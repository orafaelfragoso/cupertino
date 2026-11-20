//! `cupertino_layers_v1` (any client → server): surfaces a client hands over as layers of a parent surface, placed in
//! fixed point, blended, lifted and animated by the server on its clock.
//!
//! What a layer object asks is queued in the parent's cached state ([`LayerOps`]), so it applies with the parent's
//! commit, a synchronized parent's cached with the rest of its state. [`Cupertino::layers_committed`] applies the queue
//! to the parent's committed layers ([`ParentLayers`], in its data map): their animated properties
//! ([`LayerProperty`](cupertino_animation::LayerProperty)) and their look. An animation waits until the frame that first shows its commit is built
//! ([`present`], called by the scene) or [`START_AT_THE_LATEST`] after the commit; [`Cupertino::tick_layers`] sends
//! `finished`, `replaced` and `settled`. The content is the layer surface's own buffer, which applies on its own commit.
//! The animations a gesture-bound transaction's parts start follow the gesture's progress instead of their clock
//! ([`tracked`], [`track_gesture`]).
//! A presentation (version 2) is a layer model of a layer shell surface whose parent is itself: its offset, opacity
//! and scale apply to everything the surface draws ([`presentation`]), and it is never drawn as a layer. From version
//! 3 an xdg popup's surface may have one too, and layers of one parent may join their glass into one shape
//! ([`LayerDraw::group`]). Version 4 blurs a layer's own content by an animatable Gaussian
//! ([`LayerDraw::content_blur`], drawn by `render::content_blur`). Version 5 draws a line along a layer's glass
//! ([`GlassEdge`]), and from version 6 that line softens by the layer's content blur ([`GlassEdge::soft`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use cupertino_app_protocols::layers::MAX_CONTENT_BLUR;
use cupertino_protocols::server::layers::cupertino_layer_v1::{CupertinoLayerV1, Property};
use cupertino_protocols::server::layers::cupertino_layers_v1::CupertinoLayersV1;
use cupertino_theme::GlassStyle;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource, WEnum};
use smithay::wayland::compositor::with_states;

use crate::state::Cupertino;

mod dispatch;
mod model;
mod ops;
mod tracked;

pub use dispatch::surface_destroyed;
use model::initial;
pub use model::{BlendRole, GlassEdge, LayerModel, ParentLayers, VibrantWeight};
pub use ops::LayerOps;
use ops::{Op, Queued, Superseded};
pub use tracked::GestureProgress;

/// The role a layer's surface takes.
pub const LAYER_ROLE: &str = "cupertino_layer_v1";
/// Layers one parent may hold.
pub const MAX_PER_PARENT: usize = 64;
/// Layers one client may hold.
pub const MAX_PER_CLIENT: usize = 256;
/// Animations one client may have running or waiting.
pub const MAX_ANIMATIONS: usize = 512;
/// Model seconds after its commit by which an animation starts though no frame showed it.
pub const START_AT_THE_LATEST: f64 = 0.1;
/// The wire value of "no glass" in `set_glass`.
pub const NO_GLASS: u32 = u32::MAX;
/// The version of the global this server offers.
pub const VERSION: u32 = 6;
/// The version from which a popup may have a presentation and layers may join their glass.
pub const POPUP_VERSION: u32 = 3;
/// The version from which a layer's content blurs ([`CONTENT_BLUR`]).
pub const CONTENT_BLUR_VERSION: u32 = 4;
/// The version from which a layer's glass carries an edge (`set_glass_edge`).
pub const GLASS_EDGE_VERSION: u32 = 5;
/// The version from which the edge along a layer's glass softens by the layer's content blur ([`GlassEdge::soft`]).
pub const SOFT_EDGE_VERSION: u32 = 6;
/// Layers of one parent in one glass group: as many shapes as one joined glass holds.
pub const MAX_PER_GROUP: usize = cupertino_glass::glass::MAX_SHAPES;
/// The farthest a layer's origin, size or corner radius goes, in logical points either way. A value asked or reached
/// past it is held at it, so what the scene works out from a layer stays far inside its integer pixel range.
pub const MAX_EXTENT: f64 = 1_000_000.0;
/// The largest scale a layer or a presentation is shown at; a larger one is held at it.
pub const MAX_SCALE: f64 = 64.0;
/// The range of a timed curve's Bézier y control points; one outside is held at its end, which still lets a curve
/// overshoot its target ten times over.
pub const BEZIER_Y: (f64, f64) = (-10.0, 11.0);

/// The number of animatable properties ([`Property`]).
pub const PROPERTIES: usize = 8;
/// The index of the content blur property.
pub const CONTENT_BLUR: usize = 7;

/// Source of layer serials: identity of a layer for the element cache.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// The index of a property on the wire, for a layer object of `version`.
fn property_index(p: WEnum<Property>, version: u32) -> Option<usize> {
    match p {
        WEnum::Value(Property::ContentBlur) => {
            (version >= CONTENT_BLUR_VERSION).then_some(CONTENT_BLUR)
        }
        WEnum::Value(p) => Some(match p {
            Property::X => 0,
            Property::Y => 1,
            Property::Width => 2,
            Property::Height => 3,
            Property::Opacity => 4,
            Property::Scale => 5,
            Property::CornerRadius => 6,
            _ => return None,
        }),
        WEnum::Unknown(_) => None,
    }
}

/// Properties a presentation has: x, y, opacity and scale.
fn presented(prop: usize) -> bool {
    matches!(prop, 0 | 1 | 4 | 5)
}

/// Properties that move, reshape or blur the layer, which reduced motion does not animate.
fn spatial(prop: usize) -> bool {
    prop != 4
}

/// `v` held inside the range property `prop` is shown in: its protocol range, with origins and sizes held within
/// [`MAX_EXTENT`] and the scale within [`MAX_SCALE`]; not a number is 0.
pub fn held(prop: usize, v: f64) -> f64 {
    if v.is_nan() {
        return 0.0;
    }
    match prop {
        0 | 1 => v.clamp(-MAX_EXTENT, MAX_EXTENT),
        4 => v.clamp(0.0, 1.0),
        5 => v.clamp(0.0, MAX_SCALE),
        CONTENT_BLUR => v.clamp(0.0, MAX_CONTENT_BLUR),
        _ => v.clamp(0.0, MAX_EXTENT),
    }
}

/// Whether `v` lies in the range of property `prop`.
fn in_range(prop: usize, v: f64) -> bool {
    match prop {
        0 | 1 => v.is_finite(),
        4 => (0.0..=1.0).contains(&v),
        CONTENT_BLUR => (0.0..=MAX_CONTENT_BLUR).contains(&v),
        _ => v >= 0.0 && v.is_finite(),
    }
}

/// The role data of a layer's surface: the parent it belongs to.
pub struct LayerSurfaceData {
    pub parent: WlSurface,
}

/// Per-object data of a layer: its parent and serial; `None` for an inert layer.
pub struct LayerData {
    parent: Option<WlSurface>,
    /// The layer's own surface, which may become a layer again once this object is destroyed.
    surface: Option<WlSurface>,
    serial: u64,
    client: ClientId,
    /// A presentation: only x, y, opacity and scale, and none of the look requests.
    whole: bool,
}

impl LayerData {
    fn queue(&self, op: Op) {
        let Some(parent) = self.parent.as_ref().filter(|p| p.is_alive()) else {
            return;
        };
        with_states(parent, |states| {
            states
                .cached_state
                .get::<LayerOps>()
                .pending()
                .push(self.serial, op);
        });
    }

    /// The layers of the parent in glass group `group` once its pending requests apply, this layer left out.
    fn in_group(&self, group: u32) -> usize {
        let Some(parent) = self.parent.as_ref().filter(|p| p.is_alive()) else {
            return 0;
        };
        let mut groups: Vec<(u64, u32)> = with_parent(parent, |layers| {
            layers.iter().map(|l| (l.serial, l.group)).collect()
        });
        with_states(parent, |states| {
            for (serial, op) in &states.cached_state.get::<LayerOps>().pending().ops {
                match op {
                    Op::Create { .. } => groups.push((*serial, 0)),
                    Op::Destroy => groups.retain(|(s, _)| s != serial),
                    Op::Group(g, _) => {
                        for (_, was) in groups.iter_mut().filter(|(s, _)| s == serial) {
                            *was = *g;
                        }
                    }
                    _ => {}
                }
            }
        });
        groups
            .iter()
            .filter(|&&(s, g)| s != self.serial && g == group)
            .count()
    }
}

/// The global and the parents that hold layers.
pub struct Layers {
    _global: GlobalId,
    parents: Vec<WlSurface>,
    /// Layers each client holds, destroyed ones not counted.
    held: HashMap<ClientId, usize>,
    /// Animations each client has queued and not yet applied ([`Queued`]).
    queued: HashMap<ClientId, Arc<AtomicUsize>>,
    /// Queued animations a later request made moot before they applied, answered `replaced` with the next tick.
    superseded: Superseded,
}

impl Layers {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            _global: dh.create_global::<Cupertino, CupertinoLayersV1, ()>(VERSION, ()),
            parents: Vec::new(),
            held: HashMap::new(),
            queued: HashMap::new(),
            superseded: Superseded::default(),
        }
    }

    /// Whether a layer animates or waits to start: frames are drawn while it does.
    pub fn wants_tick(&self, now: f64) -> bool {
        self.parents.iter().any(|p| {
            p.is_alive()
                && with_parent(p, |layers| {
                    layers.iter().any(|l| l.animating(now) || l.busy)
                })
        })
    }

    /// Whether every layer and presentation of every client shows its target values at `now`: what desktop control reads
    /// of the clients' layouts is what is shown.
    pub fn settled(&self, now: f64) -> bool {
        self.parents
            .iter()
            .all(|p| !p.is_alive() || settled(p, now))
    }

    /// Animations the client has running or waiting, those queued and not yet applied included, wherever they wait.
    fn animations(&self, client: &ClientId) -> usize {
        let queued = self
            .queued
            .get(client)
            .map_or(0, |n| n.load(Ordering::Relaxed));
        queued
            + self
                .parents
                .iter()
                .filter(|p| p.is_alive() && p.client().is_some_and(|c| c.id() == *client))
                .map(|p| {
                    with_parent(p, |layers| {
                        layers
                            .iter()
                            .map(|l| l.waiting.len() + l.running.iter().flatten().count())
                            .sum::<usize>()
                    })
                })
                .sum::<usize>()
    }

    /// A count of `client`'s animation of `layer`, held while it is queued.
    fn queue_animation(&mut self, client: &ClientId, layer: &CupertinoLayerV1) -> Queued {
        let count = self.queued.entry(client.clone()).or_default();
        Queued::new(count, layer, &self.superseded)
    }
}

/// Run `f` on the committed layers of `parent`.
pub fn with_parent<R>(parent: &WlSurface, f: impl FnOnce(&mut Vec<LayerModel>) -> R) -> R {
    with_states(parent, |states| {
        let layers = states
            .data_map
            .get_or_insert_threadsafe(ParentLayers::default);
        let mut guard = layers.0.lock().unwrap();
        f(&mut guard)
    })
}

/// A layer as the scene draws it at one time: its shown values and look.
#[derive(Clone, Debug)]
pub struct LayerDraw {
    pub serial: u64,
    pub surface: WlSurface,
    /// Origin and size of the bounds in the parent's logical points.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub opacity: f64,
    pub scale: f64,
    pub radius: f64,
    pub content: (f64, f64),
    pub glass: Option<(GlassStyle, f64)>,
    pub blend: BlendRole,
    pub raised: bool,
    pub pointer_lit: bool,
    pub blur_ramp: [f32; 2],
    pub masks: bool,
    pub placeholder: [f32; 4],
    /// The glass group, 0 for the layer's glass alone, and how far the group's shapes blend, in points.
    pub group: u32,
    pub spacing: f64,
    /// The line along the outside of its glass, or of its group's joined glass when it is the group's first layer.
    pub edge: GlassEdge,
    /// The standard deviation of the blur of the layer's content, in the layer's points; 0 draws it sharp.
    pub content_blur: f64,
}

/// The layers of `parent` as shown at `now`, back to front (flat ones in creation order, then raised ones), starting
/// the animations whose commit this frame shows first.
pub fn present(parent: &WlSurface, now: f64, reduce_motion: bool) -> Vec<LayerDraw> {
    with_parent(parent, |layers| {
        let mut out: Vec<LayerDraw> = layers
            .iter_mut()
            .filter(|l| l.surface.is_alive() && !l.whole)
            .map(|l| {
                l.start(now, reduce_motion);
                LayerDraw {
                    serial: l.serial,
                    surface: l.surface.clone(),
                    x: l.value(0, now),
                    y: l.value(1, now),
                    width: l.value(2, now),
                    height: l.value(3, now),
                    opacity: l.value(4, now).clamp(0.0, 1.0),
                    scale: l.value(5, now).max(0.0),
                    radius: l.value(6, now).max(0.0),
                    content: l.content_size(),
                    glass: l.glass,
                    blend: l.blend,
                    raised: l.raised,
                    pointer_lit: l.pointer_lit,
                    blur_ramp: l.blur_ramp,
                    masks: l.masks,
                    placeholder: l.placeholder,
                    group: l.group,
                    spacing: l.spacing,
                    edge: l.edge,
                    content_blur: l.value(CONTENT_BLUR, now).clamp(0.0, MAX_CONTENT_BLUR),
                }
            })
            .collect();
        out.sort_by_key(|l| l.raised);
        out
    })
}

/// What a surface's presentation shows at one time: everything the surface draws is moved by `x` and `y` (points),
/// scaled by `scale` about the surface's centre and faded by `opacity`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presented {
    pub x: f64,
    pub y: f64,
    pub opacity: f64,
    pub scale: f64,
}

/// The presentation of `surface` as shown at `now`, starting the animations whose commit this frame shows first;
/// `None` when the surface has none.
pub fn presentation(surface: &WlSurface, now: f64, reduce_motion: bool) -> Option<Presented> {
    with_parent(surface, |layers| {
        layers.iter_mut().find(|l| l.whole).map(|l| {
            l.start(now, reduce_motion);
            Presented {
                x: l.value(0, now),
                y: l.value(1, now),
                opacity: l.value(4, now).clamp(0.0, 1.0),
                scale: l.value(5, now).max(0.0),
            }
        })
    })
}

/// Whether every layer of `parent` shows its target values: what desktop control reads is what is shown.
pub fn settled(parent: &WlSurface, now: f64) -> bool {
    with_parent(parent, |layers| layers.iter().all(|l| !l.animating(now)))
}

/// The target value of property `prop` of the layer `serial` of `parent`, what the client hit-tests against.
pub fn target(parent: &WlSurface, serial: u64, prop: usize) -> Option<f64> {
    with_parent(parent, |layers| {
        layers
            .iter()
            .find(|l| l.serial == serial)
            .map(|l| l.target(prop))
    })
}

/// The animations waiting to start on the layers of `parent`, its presentation's included, are driven by the gesture of
/// `progress` once they start: the commits of a gesture-bound transaction's part just applied.
pub fn track_gesture(parent: &WlSurface, progress: &GestureProgress) {
    if parent.is_alive() {
        with_parent(parent, |layers| {
            for l in layers.iter_mut() {
                l.track(progress);
            }
        });
    }
}

/// The layer requests queued on `parent` for its next commit.
pub fn queued(parent: &WlSurface) -> usize {
    with_states(parent, |s| {
        s.cached_state.get::<LayerOps>().pending().ops.len()
    })
}

impl Cupertino {
    /// Apply the layer requests the commit of `surface` carries, and redraw a layer surface's parent.
    pub fn layers_committed(&mut self, surface: &WlSurface) {
        let ops = with_states(surface, |states| {
            std::mem::take(&mut states.cached_state.get::<LayerOps>().current().ops)
        });
        if !ops.is_empty() {
            let now = self.now();
            with_parent(surface, |layers| {
                for (serial, op) in ops {
                    match op {
                        Op::Create {
                            surface,
                            resource,
                            whole,
                        } => layers.push(LayerModel::new(serial, surface, resource, whole)),
                        Op::Destroy => layers.retain(|l| l.serial != serial),
                        op => {
                            if let Some(l) = layers.iter_mut().find(|l| l.serial == serial) {
                                l.apply(op, now);
                            }
                        }
                    }
                }
            });
            if !self.layers.parents.contains(surface) {
                self.layers.parents.push(surface.clone());
            }
        }
    }

    /// The parent a layer surface draws in, for a commit of the layer's content.
    pub fn layer_parent(surface: &WlSurface) -> Option<WlSurface> {
        with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .map(|d| d.parent.clone())
        })
    }

    /// Start animations no frame showed in time, and send `finished`, `settled` and the `replaced` of animations
    /// superseded before they applied.
    pub fn tick_layers(&mut self, now: f64) {
        self.answer_superseded(now);
        self.layers.parents.retain(|p| p.is_alive());
        let mut redraw = false;
        let reduce_motion = self.display_options().reduce_motion;
        for parent in &self.layers.parents {
            with_parent(parent, |layers| {
                for l in layers.iter_mut() {
                    if l.waiting
                        .iter()
                        .any(|w| now >= w.since + START_AT_THE_LATEST)
                    {
                        l.start(now, reduce_motion);
                        redraw = true;
                    }
                    l.follow_gestures(now);
                    for prop in 0..PROPERTIES {
                        if l.running[prop].is_some()
                            && l.tracked[prop].is_none()
                            && !l.props[prop].is_animating(now)
                        {
                            let serial = l.running[prop].take().unwrap();
                            l.resource.finished(serial);
                        }
                    }
                    let busy = l.animating(now) || l.running.iter().any(Option::is_some);
                    if l.busy && !busy {
                        l.resource.settled();
                    }
                    l.busy = busy;
                }
            });
        }
        if redraw {
            self.request_redraw();
        }
    }

    /// Answer each animation a later request superseded before it applied `replaced`, with what its layer shows at
    /// `now` (where a layer not yet applied starts), the start of the motion that follows.
    fn answer_superseded(&mut self, now: f64) {
        let superseded = std::mem::take(&mut *self.layers.superseded.lock().unwrap());
        for (layer, serial, prop) in superseded {
            let Some(data) = layer.data::<LayerData>().filter(|_| layer.is_alive()) else {
                continue;
            };
            let shown = data
                .parent
                .as_ref()
                .filter(|p| p.is_alive())
                .and_then(|parent| {
                    with_parent(parent, |layers| {
                        layers
                            .iter()
                            .find(|l| l.serial == data.serial)
                            .map(|l| (l.value(prop, now), l.velocity(prop, now)))
                    })
                });
            let (value, velocity) = shown.unwrap_or((initial(prop), 0.0));
            layer.replaced(serial, value, velocity);
        }
    }
}
