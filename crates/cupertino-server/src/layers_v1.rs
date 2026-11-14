//! `cupertino_layers_v1` (any client → server): surfaces a client hands over as layers of a parent surface, placed in
//! fixed point, blended, lifted and animated by the server on its clock.
//!
//! What a layer object asks is queued in the parent's cached state ([`LayerOps`]), so it applies with the parent's
//! commit, a synchronized parent's cached with the rest of its state. [`Cupertino::layers_committed`] applies the queue
//! to the parent's committed layers ([`ParentLayers`], in its data map): their animated properties
//! ([`LayerProperty`]) and their look. An animation waits until the frame that first shows its commit is built
//! ([`present`], called by the scene) or [`START_AT_THE_LATEST`] after the commit; [`Cupertino::tick_layers`] sends
//! `finished`, `replaced` and `settled`. The content is the layer surface's own buffer, which applies on its own commit.
//! A presentation (version 2) is a layer model of a layer shell surface whose parent is itself: its offset, opacity
//! and scale apply to everything the surface draws ([`presentation`]), and it is never drawn as a layer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use cupertino_animation::{CubicBezier, LayerCurve, LayerProperty};
use cupertino_protocols::server::layers::cupertino_layer_v1::{
    self, Blend, CupertinoLayerV1, Elevation, Error, Highlight, Property,
};
use cupertino_protocols::server::layers::cupertino_layers_v1::{
    self, CupertinoLayersV1, Error as LayersError,
};
use cupertino_theme::GlassStyle;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::wayland::compositor::{get_role, give_role, with_states, Cacheable};

use crate::state::Cupertino;

/// The role a layer's surface takes.
pub const LAYER_ROLE: &str = "cupertino_layer_v1";
/// Layers one parent may hold.
pub const MAX_PER_PARENT: usize = 64;
/// Layers one client may hold.
pub const MAX_PER_CLIENT: usize = 256;
/// Animations one client may have running or waiting.
pub const MAX_ANIMATIONS: usize = 512;
/// Longest timed animation, in milliseconds.
pub const MAX_DURATION_MS: u32 = 10_000;
/// Range of a spring's response, in seconds.
pub const SPRING_RESPONSE: (f64, f64) = (0.01, 10.0);
/// Range of a spring's damping ratio.
pub const SPRING_DAMPING: (f64, f64) = (0.05, 4.0);
/// Range of a lens's magnification.
pub const LENS: (f64, f64) = (1.0, 4.0);
/// Model seconds after its commit by which an animation starts though no frame showed it.
pub const START_AT_THE_LATEST: f64 = 0.1;
/// The wire value of "no glass" in `set_glass`.
pub const NO_GLASS: u32 = u32::MAX;

/// The number of animatable properties ([`Property`]).
pub const PROPERTIES: usize = 7;

/// Source of layer serials: identity of a layer for the element cache.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// The index of a property on the wire.
fn property_index(p: WEnum<Property>) -> Option<usize> {
    match p {
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

/// Properties that move or reshape the layer, which reduced motion does not animate.
fn spatial(prop: usize) -> bool {
    prop != 4
}

/// Whether `v` lies in the range of property `prop`.
fn in_range(prop: usize, v: f64) -> bool {
    match prop {
        0 | 1 => v.is_finite(),
        4 => (0.0..=1.0).contains(&v),
        _ => v >= 0.0 && v.is_finite(),
    }
}

/// How a layer's content mixes with what lies behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendRole {
    #[default]
    Normal,
    /// Coverage drawn in a colour taken from the glass behind, at a weight (primary, secondary, fill).
    Vibrant(VibrantWeight),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VibrantWeight {
    Primary,
    Secondary,
    Fill,
}

/// One queued request of a layer, applied with the parent's commit.
enum Op {
    Create {
        surface: WlSurface,
        resource: CupertinoLayerV1,
        whole: bool,
    },
    Destroy,
    Set(usize, f64),
    Animate {
        serial: u32,
        prop: usize,
        to: f64,
        curve: LayerCurve,
        velocity: f64,
    },
    Cancel(usize),
    Glass(Option<(GlassStyle, f64)>),
    Blend(BlendRole),
    Raised(bool),
    PointerLit(bool),
    BlurRamp([f32; 2]),
    Masks(bool),
    Placeholder([f32; 4]),
}

/// The layer requests queued on a parent surface, by layer serial, in request order.
#[derive(Default)]
pub struct LayerOps {
    ops: Vec<(u64, Op)>,
}

impl LayerOps {
    fn animations(&self) -> usize {
        self.ops
            .iter()
            .filter(|(_, o)| matches!(o, Op::Animate { .. }))
            .count()
    }
}

impl Cacheable for LayerOps {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        std::mem::take(self)
    }

    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        into.ops.extend(self.ops);
    }
}

/// An animation asked for but not started: it starts when its commit is first shown.
#[derive(Clone, Copy, Debug)]
struct Waiting {
    serial: u32,
    prop: usize,
    to: f64,
    curve: LayerCurve,
    velocity: f64,
    since: f64,
}

/// A committed layer of a parent.
pub struct LayerModel {
    pub serial: u64,
    pub surface: WlSurface,
    /// The parent's own presentation (`get_presentation`), not a layer drawn over it.
    pub whole: bool,
    resource: CupertinoLayerV1,
    props: [LayerProperty; PROPERTIES],
    /// The serial of each property's running animation.
    running: [Option<u32>; PROPERTIES],
    waiting: Vec<Waiting>,
    /// Width and height follow the content's size until they are set.
    sized: [bool; 2],
    /// Whether the layer had an animation running or waiting at the last tick, for `settled`.
    busy: bool,
    pub glass: Option<(GlassStyle, f64)>,
    pub blend: BlendRole,
    pub raised: bool,
    pub pointer_lit: bool,
    pub blur_ramp: [f32; 2],
    pub masks: bool,
    pub placeholder: [f32; 4],
}

impl LayerModel {
    fn new(serial: u64, surface: WlSurface, resource: CupertinoLayerV1, whole: bool) -> Self {
        let mut props = [LayerProperty::new(0.0); PROPERTIES];
        props[4] = LayerProperty::new(1.0);
        props[5] = LayerProperty::new(1.0);
        Self {
            serial,
            surface,
            whole,
            resource,
            props,
            running: [None; PROPERTIES],
            waiting: Vec::new(),
            sized: [false; 2],
            busy: false,
            glass: None,
            blend: BlendRole::Normal,
            raised: false,
            pointer_lit: false,
            blur_ramp: [1.0, 1.0],
            masks: false,
            placeholder: [0.0; 4],
        }
    }

    /// The content's size in logical points, (0, 0) before its first buffer.
    pub fn content_size(&self) -> (f64, f64) {
        use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
        with_states(&self.surface, |states| {
            states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|d| d.lock().unwrap().view())
                .map_or((0.0, 0.0), |v| (f64::from(v.dst.w), f64::from(v.dst.h)))
        })
    }

    /// Width and height take the content's size before the first value or animation of either.
    fn fix_size(&mut self, prop: usize) {
        if prop == 2 || prop == 3 {
            let i = prop - 2;
            if !self.sized[i] {
                let (w, h) = self.content_size();
                self.props[prop].set(if i == 0 { w } else { h });
                self.sized[i] = true;
            }
        }
    }

    /// End the running animation of `prop` at `now`: `replaced` with what was shown.
    fn replace(&mut self, prop: usize, now: f64) {
        if let Some(serial) = self.running[prop].take() {
            if self.props[prop].is_animating(now) {
                let p = &self.props[prop];
                self.resource
                    .replaced(serial, p.value(now), p.velocity(now));
            } else {
                self.resource.finished(serial);
            }
        }
    }

    /// The value of `prop` shown at `now`.
    pub fn value(&self, prop: usize, now: f64) -> f64 {
        match prop {
            2 | 3 if !self.sized[prop - 2] => {
                let (w, h) = self.content_size();
                if prop == 2 {
                    w
                } else {
                    h
                }
            }
            _ => self.props[prop].value(now),
        }
    }

    /// The value `prop` rests at once its animation ends: what the client hit-tests against.
    pub fn target(&self, prop: usize) -> f64 {
        self.waiting
            .iter()
            .rev()
            .find(|w| w.prop == prop)
            .map_or_else(|| self.props[prop].target(), |w| w.to)
    }

    /// Whether an animation of the layer runs or waits at `now`.
    pub fn animating(&self, now: f64) -> bool {
        !self.waiting.is_empty() || self.props.iter().any(|p| p.is_animating(now))
    }

    /// Start the waiting animations at `now`; with `reduce_motion` the spatial ones end at their target at once.
    fn start(&mut self, now: f64, reduce_motion: bool) {
        for w in std::mem::take(&mut self.waiting) {
            self.fix_size(w.prop);
            self.replace(w.prop, now);
            if reduce_motion && spatial(w.prop) {
                self.props[w.prop].set(w.to);
            } else {
                self.props[w.prop].animate(w.to, w.curve, now, w.velocity);
            }
            self.running[w.prop] = Some(w.serial);
        }
    }

    fn apply(&mut self, op: Op, now: f64) {
        match op {
            Op::Create { .. } | Op::Destroy => {}
            Op::Set(prop, v) => {
                self.fix_size(prop);
                self.waiting.retain(|w| w.prop != prop);
                self.replace(prop, now);
                self.props[prop].set(v);
            }
            Op::Animate {
                serial,
                prop,
                to,
                curve,
                velocity,
            } => self.waiting.push(Waiting {
                serial,
                prop,
                to,
                curve,
                velocity,
                since: now,
            }),
            Op::Cancel(prop) => {
                self.waiting.retain(|w| w.prop != prop);
                let shown = self.props[prop].value(now);
                self.replace(prop, now);
                self.props[prop].set(shown);
            }
            Op::Glass(g) => self.glass = g,
            Op::Blend(b) => self.blend = b,
            Op::Raised(r) => self.raised = r,
            Op::PointerLit(l) => self.pointer_lit = l,
            Op::BlurRamp(r) => self.blur_ramp = r,
            Op::Masks(m) => self.masks = m,
            Op::Placeholder(c) => self.placeholder = c,
        }
    }
}

/// The committed layers of a parent surface in creation order, in its data map.
#[derive(Default)]
pub struct ParentLayers(pub Mutex<Vec<LayerModel>>);

/// The role data of a layer's surface: the parent it belongs to.
pub struct LayerSurfaceData {
    pub parent: WlSurface,
}

/// Per-object data of a layer: its parent and serial; `None` for an inert layer.
pub struct LayerData {
    parent: Option<WlSurface>,
    serial: u64,
    client: ClientId,
    /// A presentation: only x, y, opacity and scale, and none of the look requests.
    whole: bool,
}

/// In a layer shell surface's data map: whether it holds a presentation that is not destroyed.
#[derive(Default)]
struct HasPresentation(Mutex<bool>);

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
                .ops
                .push((self.serial, op));
        });
    }
}

/// The global and the parents that hold layers.
pub struct Layers {
    _global: GlobalId,
    parents: Vec<WlSurface>,
    /// Layers each client holds, destroyed ones not counted.
    held: HashMap<ClientId, usize>,
}

impl Layers {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            _global: dh.create_global::<Cupertino, CupertinoLayersV1, ()>(2, ()),
            parents: Vec::new(),
            held: HashMap::new(),
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

    /// Animations the client has running or waiting, those queued on its parents included.
    fn animations(&self, client: &ClientId) -> usize {
        self.parents
            .iter()
            .filter(|p| p.is_alive() && p.client().is_some_and(|c| c.id() == *client))
            .map(|p| {
                let queued = with_states(p, |s| {
                    s.cached_state.get::<LayerOps>().pending().animations()
                });
                queued
                    + with_parent(p, |layers| {
                        layers
                            .iter()
                            .map(|l| l.waiting.len() + l.running.iter().flatten().count())
                            .sum::<usize>()
                    })
            })
            .sum()
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

    /// Start animations no frame showed in time, and send `finished` and `settled`.
    pub fn tick_layers(&mut self, now: f64) {
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
                    for prop in 0..PROPERTIES {
                        if l.running[prop].is_some() && !l.props[prop].is_animating(now) {
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
}

impl GlobalDispatch<CupertinoLayersV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoLayersV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

fn owned_by(surface: &WlSurface, client: &Client) -> bool {
    surface.is_alive() && surface.client().is_some_and(|c| c.id() == client.id())
}

impl Dispatch<CupertinoLayersV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        global: &CupertinoLayersV1,
        request: cupertino_layers_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_layers_v1::Request::GetLayer {
                id,
                surface,
                parent,
            } => {
                let inert = LayerData {
                    parent: None,
                    serial: 0,
                    client: client.id(),
                    whole: false,
                };
                if !owned_by(&surface, client) || !owned_by(&parent, client) {
                    global.post_error(
                        LayersError::InvalidSurface,
                        "the surface or the parent belongs to another client or is gone",
                    );
                    init.init(id, inert);
                    return;
                }
                let parent_is_layer = get_role(&parent) == Some(LAYER_ROLE);
                if surface == parent || parent_is_layer || give_role(&surface, LAYER_ROLE).is_err()
                {
                    // a surface that already is a layer of this parent may become one again
                    let again = get_role(&surface) == Some(LAYER_ROLE)
                        && Cupertino::layer_parent(&surface).as_ref() == Some(&parent);
                    if !again || surface == parent || parent_is_layer {
                        global.post_error(
                            LayersError::Role,
                            "the surface has a role, is the parent, or the parent is a layer",
                        );
                        init.init(id, inert);
                        return;
                    }
                }
                let on_parent = with_parent(&parent, |l| l.len())
                    + with_states(&parent, |s| {
                        s.cached_state
                            .get::<LayerOps>()
                            .pending()
                            .ops
                            .iter()
                            .filter(|(_, o)| matches!(o, Op::Create { .. }))
                            .count()
                    });
                let held = state.layers.held.entry(client.id()).or_default();
                if on_parent >= MAX_PER_PARENT || *held >= MAX_PER_CLIENT {
                    global.post_error(
                        LayersError::TooManyLayers,
                        format!(
                            "a parent holds at most {MAX_PER_PARENT} layers and a client {MAX_PER_CLIENT}"
                        ),
                    );
                    init.init(id, inert);
                    return;
                }
                *held += 1;
                with_states(&surface, |s| {
                    s.data_map
                        .insert_if_missing_threadsafe(|| LayerSurfaceData {
                            parent: parent.clone(),
                        })
                });
                let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
                let data = LayerData {
                    parent: Some(parent.clone()),
                    serial,
                    client: client.id(),
                    whole: false,
                };
                let resource = init.init(id, data);
                with_states(&parent, |s| {
                    s.cached_state.get::<LayerOps>().pending().ops.push((
                        serial,
                        Op::Create {
                            surface: surface.clone(),
                            resource,
                            whole: false,
                        },
                    ))
                });
                if !state.layers.parents.contains(&parent) {
                    state.layers.parents.push(parent);
                }
            }
            cupertino_layers_v1::Request::GetPresentation { id, surface } => {
                let inert = LayerData {
                    parent: None,
                    serial: 0,
                    client: client.id(),
                    whole: true,
                };
                if !owned_by(&surface, client) {
                    global.post_error(
                        LayersError::InvalidSurface,
                        "the surface belongs to another client or is gone",
                    );
                    init.init(id, inert);
                    return;
                }
                let layer_shell = get_role(&surface)
                    == Some(smithay::wayland::shell::wlr_layer::LAYER_SURFACE_ROLE);
                let taken = with_states(&surface, |s| {
                    let held = s
                        .data_map
                        .get_or_insert_threadsafe(HasPresentation::default);
                    let mut held = held.0.lock().unwrap();
                    let taken = *held;
                    if layer_shell {
                        *held = true;
                    }
                    taken
                });
                if !layer_shell || taken {
                    global.post_error(
                        LayersError::Role,
                        "the surface is not a layer shell surface or already has a presentation",
                    );
                    init.init(id, inert);
                    return;
                }
                *state.layers.held.entry(client.id()).or_default() += 1;
                let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
                let data = LayerData {
                    parent: Some(surface.clone()),
                    serial,
                    client: client.id(),
                    whole: true,
                };
                let resource = init.init(id, data);
                with_states(&surface, |s| {
                    s.cached_state.get::<LayerOps>().pending().ops.push((
                        serial,
                        Op::Create {
                            surface: surface.clone(),
                            resource,
                            whole: true,
                        },
                    ))
                });
                if !state.layers.parents.contains(&surface) {
                    state.layers.parents.push(surface);
                }
            }
            cupertino_layers_v1::Request::Destroy => {}
            _ => unreachable!("cupertino_layers_v1 version 2 has no other request"),
        }
    }
}

/// The curve of `animate_timed`, or the reason it is refused.
fn timed(duration: u32, x1: f64, y1: f64, x2: f64, y2: f64) -> Option<LayerCurve> {
    let unit = 0.0..=1.0;
    (duration <= MAX_DURATION_MS && unit.contains(&x1) && unit.contains(&x2)).then(|| {
        LayerCurve::Timed {
            duration: f64::from(duration) / 1000.0,
            bezier: CubicBezier::new(x1, y1, x2, y2),
        }
    })
}

/// The curve of `animate_spring`, or the reason it is refused.
fn spring(response: f64, damping: f64) -> Option<LayerCurve> {
    ((SPRING_RESPONSE.0..=SPRING_RESPONSE.1).contains(&response)
        && (SPRING_DAMPING.0..=SPRING_DAMPING.1).contains(&damping))
    .then_some(LayerCurve::Spring { response, damping })
}

impl Dispatch<CupertinoLayerV1, LayerData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        layer: &CupertinoLayerV1,
        request: cupertino_layer_v1::Request,
        data: &LayerData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use cupertino_layer_v1::Request as R;
        let bad_property = |what: &str| {
            layer.post_error(Error::InvalidProperty, format!("unknown {what}"));
        };
        let bad_value = |what: &str| layer.post_error(Error::InvalidValue, what.to_string());
        let animation = |state: &mut Cupertino,
                         serial: u32,
                         property: WEnum<Property>,
                         to: f64,
                         curve: Option<LayerCurve>,
                         velocity: f64| {
            let Some(prop) = property_index(property).filter(|&p| !data.whole || presented(p))
            else {
                return bad_property("property");
            };
            if !in_range(prop, to) {
                return bad_value("the target is outside the property's range");
            }
            let Some(curve) = curve else {
                return layer.post_error(
                    Error::InvalidCurve,
                    "a duration over 10 s, Bézier x outside 0 to 1, or a spring outside its ranges",
                );
            };
            if state.layers.animations(&data.client) >= MAX_ANIMATIONS {
                return layer.post_error(
                    Error::TooManyAnimations,
                    format!("a client has at most {MAX_ANIMATIONS} animations"),
                );
            }
            data.queue(Op::Animate {
                serial,
                prop,
                to,
                curve,
                velocity,
            });
        };
        match request {
            R::Destroy => {}
            R::SetValue { property, value } => {
                match property_index(property).filter(|&p| !data.whole || presented(p)) {
                    None => bad_property("property"),
                    Some(prop) if !in_range(prop, value) => {
                        bad_value("the value is outside the property's range")
                    }
                    Some(prop) => data.queue(Op::Set(prop, value)),
                }
            }
            R::AnimateTimed {
                serial,
                property,
                value,
                duration,
                x1,
                y1,
                x2,
                y2,
            } => animation(
                state,
                serial,
                property,
                value,
                timed(duration, x1, y1, x2, y2),
                0.0,
            ),
            R::AnimateSpring {
                serial,
                property,
                value,
                response,
                damping,
                velocity,
            } => animation(
                state,
                serial,
                property,
                value,
                spring(response, damping),
                velocity,
            ),
            R::Cancel { property } => {
                match property_index(property).filter(|&p| !data.whole || presented(p)) {
                    Some(prop) => data.queue(Op::Cancel(prop)),
                    None => bad_property("property"),
                }
            }
            _ if data.whole => bad_property("request on a presentation"),
            R::SetGlass { style, lens } => {
                if style == NO_GLASS {
                    data.queue(Op::Glass(None));
                    return;
                }
                let Some((_, style)) = crate::app_effects_v1::GLASS_MATERIALS.get(style as usize)
                else {
                    return bad_property("glass style");
                };
                if !(LENS.0..=LENS.1).contains(&lens) {
                    return bad_value("a lens below 1 or above 4");
                }
                data.queue(Op::Glass(Some((*style, lens))));
            }
            R::SetBlend { blend } => {
                let role = match blend {
                    WEnum::Value(Blend::Normal) => BlendRole::Normal,
                    WEnum::Value(Blend::Vibrant) => BlendRole::Vibrant(VibrantWeight::Primary),
                    WEnum::Value(Blend::VibrantSecondary) => {
                        BlendRole::Vibrant(VibrantWeight::Secondary)
                    }
                    WEnum::Value(Blend::VibrantFill) => BlendRole::Vibrant(VibrantWeight::Fill),
                    _ => return bad_property("blend role"),
                };
                data.queue(Op::Blend(role));
            }
            R::SetElevation { elevation } => match elevation {
                WEnum::Value(Elevation::Flat) => data.queue(Op::Raised(false)),
                WEnum::Value(Elevation::Raised) => data.queue(Op::Raised(true)),
                _ => bad_property("elevation"),
            },
            R::SetHighlight { highlight } => match highlight {
                WEnum::Value(Highlight::Fixed) => data.queue(Op::PointerLit(false)),
                WEnum::Value(Highlight::Pointer) => data.queue(Op::PointerLit(true)),
                _ => bad_property("highlight"),
            },
            R::SetBlurRamp { top, bottom } => {
                if !(0.0..=1.0).contains(&top) || !(0.0..=1.0).contains(&bottom) {
                    return bad_value("a blur weight outside 0 to 1");
                }
                data.queue(Op::BlurRamp([top as f32, bottom as f32]));
            }
            R::SetMasksToBounds { masks } => data.queue(Op::Masks(masks != 0)),
            R::SetPlaceholder {
                red,
                green,
                blue,
                alpha,
            } => {
                let c = [red, green, blue, alpha];
                if c.iter().any(|v| !(0.0..=1.0).contains(v)) {
                    return bad_value("a placeholder channel outside 0 to 1");
                }
                data.queue(Op::Placeholder(c.map(|v| v as f32)));
            }
            _ => unreachable!("cupertino_layer_v1 version 2 has no other request"),
        }
    }

    fn destroyed(state: &mut Self, client: ClientId, _: &CupertinoLayerV1, data: &LayerData) {
        if let Some(surface) = data.parent.as_ref().filter(|s| data.whole && s.is_alive()) {
            with_states(surface, |s| {
                if let Some(held) = s.data_map.get::<HasPresentation>() {
                    *held.0.lock().unwrap() = false;
                }
            });
        }
        if data.parent.is_some() {
            if let Some(n) = state.layers.held.get_mut(&client) {
                *n = n.saturating_sub(1);
            }
            data.queue(Op::Destroy);
        }
    }
}

/// Forget the layers of a destroyed layer surface at once, and a client's count with its last layer.
pub fn surface_destroyed(state: &mut Cupertino, surface: &WlSurface) {
    if let Some(parent) = Cupertino::layer_parent(surface).filter(|p| p.is_alive()) {
        with_parent(&parent, |layers| layers.retain(|l| l.surface != *surface));
    }
    state.layers.held.retain(|_, n| *n > 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_follow_the_protocol() {
        assert!(in_range(0, -40.5) && in_range(1, 1e6));
        assert!(in_range(4, 0.0) && in_range(4, 1.0) && !in_range(4, 1.01));
        assert!(!in_range(2, -1.0) && !in_range(5, -0.1) && in_range(6, 0.0));
        assert!(timed(10_000, 0.0, -2.0, 1.0, 3.0).is_some());
        assert!(timed(10_001, 0.0, 0.0, 1.0, 1.0).is_none());
        assert!(timed(300, -0.1, 0.0, 1.0, 1.0).is_none());
        assert!(spring(0.3, 0.8).is_some());
        assert!(spring(0.001, 0.8).is_none() && spring(0.3, 5.0).is_none());
        assert!(!spatial(4) && spatial(0) && spatial(5));
        assert!(presented(0) && presented(1) && presented(4) && presented(5));
        assert!(!presented(2) && !presented(3) && !presented(6));
    }
}
