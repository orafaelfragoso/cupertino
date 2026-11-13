//! `cupertino_app_effects_v1` (any client → server): the global, the material backdrops of `get_backdrop`, the title bar
//! style of `set_titlebar_style`, the appearance observer of `get_appearance` ([`crate::session_v1`]) and the disabled
//! window controls of `set_disabled_controls`. The global is public: a surface of another client is a protocol error.
//! Backdrops and disabled controls live in the surface's cached state, so they apply atomically with the surface's
//! `wl_surface.commit`; the scene reads the committed (`current`) side.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use cupertino_decorations::TitlebarStyle;
use cupertino_protocols::server::app_effects::cupertino_app_effects_v1::{
    self, CupertinoAppEffectsV1, Error as EffectsError, WindowControl,
};
use cupertino_protocols::server::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1, Error, Tone,
};
use cupertino_theme::glass::{face_tone, FaceTone};
use cupertino_theme::GlassStyle;
use smithay::desktop::{layer_map_for_output, LayerMap, LayerSurface};
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::compositor::{with_states, Cacheable, SurfaceData};

use crate::menu_bar::AppMenuData;
use crate::render::material::glass::ToneReading;
use crate::state::{ClientState, Cupertino};

/// Registers the `cupertino_app_effects_v1` global.
pub struct AppEffectsV1Global {
    _global: GlobalId,
}

impl AppEffectsV1Global {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            _global: dh.create_global::<Cupertino, CupertinoAppEffectsV1, ()>(VERSION, ()),
        }
    }
}

/// The version of `cupertino_app_effects_v1` the server offers.
const VERSION: u32 = 17;
/// The version from which the backdrops of a subsurface are drawn, above the rest of its surface tree.
pub const SUBSURFACE_BACKDROPS_VERSION: u32 = 11;

/// Most backdrops one surface may hold: the notification centre puts one behind every visible card and one behind its
/// menu, so this sits well above legitimate use and the per-client limit is the bound that counts.
const MAX_BACKDROPS_PER_SURFACE: usize = 64;
/// Most backdrops one client may hold, over all its surfaces.
const MAX_BACKDROPS_PER_CLIENT: usize = 128;

/// The version of `cupertino_backdrop_v1` that has the `tone` event.
const TONE_VERSION: u32 = 7;
/// The version of `cupertino_backdrop_v1` that has the large panel glass style.
const LARGE_PANEL_VERSION: u32 = 10;
/// The version of `cupertino_backdrop_v1` that has the bar glass style.
const BAR_VERSION: u32 = 12;
/// The version of `cupertino_backdrop_v1` that has the dense and module glass styles.
const DENSE_VERSION: u32 = 13;
/// The version of `cupertino_backdrop_v1` that has the card and track glass styles.
const CARD_VERSION: u32 = 15;
/// The version of `cupertino_backdrop_v1` that has the popover glass style.
const POPOVER_VERSION: u32 = 16;
/// The version of `cupertino_backdrop_v1` that has the switcher glass style.
const SWITCHER_VERSION: u32 = 17;

/// The tone a backdrop's text is drawn with, as the wire carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireTone {
    /// The appearance's: nothing measured, or the shape is too large to follow what lies behind it.
    Appearance,
    Measured(FaceTone),
}

/// The tone of each glass backdrop, as the server last sent it: a face measured past the threshold, with its margin,
/// changes it, and only a change is sent.
#[derive(Default)]
pub struct BackdropTones {
    tracked: HashMap<u64, Tracked>,
}

struct Tracked {
    resource: CupertinoBackdropV1,
    sent: WireTone,
}

impl BackdropTones {
    fn register(&mut self, serial: u64, resource: CupertinoBackdropV1) {
        if resource.version() >= TONE_VERSION {
            self.tracked.insert(
                serial,
                Tracked {
                    resource,
                    sent: WireTone::Appearance,
                },
            );
        }
    }

    fn forget(&mut self, serial: u64) {
        self.tracked.remove(&serial);
    }

    /// Take the faces measured under small backdrops (`readings`, oldest first) and the backdrops drawn too large to
    /// follow what lies behind them or as a material other than glass (`unadapted`), and send each backdrop whose tone
    /// changed its new tone. Only the backdrops named are looked at, so a frame that measured nothing costs a lookup
    /// per backdrop drawn.
    pub fn apply(&mut self, readings: &[ToneReading], unadapted: &[u64]) {
        let mut named: Vec<u64> = readings
            .iter()
            .map(|r| r.serial)
            .chain(unadapted.iter().copied())
            .collect();
        named.sort_unstable();
        named.dedup();
        let sent = named
            .into_iter()
            .filter_map(|s| self.tracked.get(&s).map(|t| (s, t.sent)));
        for (serial, tone) in next_tones(sent, readings, unadapted) {
            let Some(t) = self.tracked.get_mut(&serial) else {
                continue;
            };
            t.sent = tone;
            if t.resource.is_alive() {
                t.resource.tone(match tone {
                    WireTone::Appearance => Tone::Appearance,
                    WireTone::Measured(FaceTone::Dark) => Tone::Dark,
                    WireTone::Measured(FaceTone::Light) => Tone::Light,
                });
            }
        }
    }

    /// The tone last sent for backdrop `serial`, for tests.
    pub fn sent(&self, serial: u64) -> Option<WireTone> {
        self.tracked.get(&serial).map(|t| t.sent)
    }
}

/// The tones that change, given the tone each backdrop was last sent (`sent`), the faces measured since (oldest
/// first) and the backdrops drawn too large, or not as glass, since: a measured face moves the tone through
/// [`face_tone`], with its margin; a backdrop drawn too large or not as glass goes back to the appearance. A
/// backdrop not in `sent` is ignored.
pub fn next_tones(
    sent: impl IntoIterator<Item = (u64, WireTone)>,
    readings: &[ToneReading],
    unadapted: &[u64],
) -> Vec<(u64, WireTone)> {
    let mut changed = Vec::new();
    for (serial, before) in sent {
        let mut tone = before;
        for r in readings.iter().filter(|r| r.serial == serial) {
            let previous = match tone {
                WireTone::Appearance => None,
                WireTone::Measured(t) => Some(t),
            };
            tone = WireTone::Measured(face_tone(r.luminance, previous));
        }
        if unadapted.contains(&serial) {
            tone = WireTone::Appearance;
        }
        if tone != before {
            changed.push((serial, tone));
        }
    }
    changed.sort_by_key(|(serial, _)| *serial);
    changed
}

/// Source of backdrop serials: identity of a backdrop and its creation order across surfaces.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Region of a backdrop in surface-local logical coordinates.
#[derive(Clone, Copy, Debug, Default)]
struct Shape {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
}

/// Material names that draw glass rather than a recipe of the design tokens: each backdrop is a glass shape of its
/// own, like `set_glass` with container 0.
pub const GLASS_MATERIALS: [(&str, GlassStyle); 10] = [
    ("Glass", GlassStyle::Regular),
    ("ClearGlass", GlassStyle::Clear),
    ("LargePanelGlass", GlassStyle::LargePanel),
    ("BarGlass", GlassStyle::Bar),
    ("DenseGlass", GlassStyle::Dense),
    ("ModuleGlass", GlassStyle::Module),
    ("CardGlass", GlassStyle::Card),
    ("TrackGlass", GlassStyle::Track),
    ("PopoverGlass", GlassStyle::Popover),
    ("SwitcherGlass", GlassStyle::Switcher),
];

/// The glass style a material name stands for.
pub fn glass_material(name: &str) -> Option<GlassStyle> {
    GLASS_MATERIALS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, style)| *style)
}

/// How a backdrop draws as glass (`set_glass`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassSpec {
    pub style: GlassStyle,
    /// Backdrops of a surface with the same nonzero container join into one shape; 0 keeps the backdrop alone.
    pub container: u32,
    /// How far the outlines of a container blend into each other, in logical points; 0 joins them with a hard edge.
    pub spacing: f64,
}

#[derive(Clone, Debug)]
struct BackdropEntry {
    serial: u64,
    /// `None` until `set_material`: the backdrop draws nothing unless it is glass.
    material: Option<String>,
    glass: Option<GlassSpec>,
    shape: Shape,
    alpha: f64,
    /// Made through an object of [`SUBSURFACE_BACKDROPS_VERSION`] or later: on a subsurface it is drawn.
    on_subsurface: bool,
}

/// The backdrops of one surface in creation order, double-buffered with the surface.
#[derive(Clone, Debug, Default)]
pub struct BackdropState {
    entries: Vec<BackdropEntry>,
}

impl Cacheable for BackdropState {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        self.clone()
    }

    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}

/// A committed backdrop, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct BackdropArea {
    /// Identity for the element cache.
    pub key: u64,
    pub material: String,
    pub area: Rectangle<f64, Logical>,
    pub radius: f64,
    pub alpha: f64,
    /// Drawn as glass: by `set_glass`, or by a glass material name ([`GLASS_MATERIALS`]).
    pub glass: Option<GlassSpec>,
}

/// Committed, visible backdrops of `surface` (set material or glass, non-empty shape) in creation order,
/// placed relative to `origin`, the logical position of the surface's local origin.
pub fn surface_backdrops(surface: &WlSurface, origin: Point<f64, Logical>) -> Vec<BackdropArea> {
    with_states(surface, |states| committed_backdrops(states, origin, false))
}

/// Committed, visible backdrops of the subsurface `surface` that it draws ([`SUBSURFACE_BACKDROPS_VERSION`]), as
/// [`surface_backdrops`] places them.
pub fn subsurface_backdrops(surface: &WlSurface, origin: Point<f64, Logical>) -> Vec<BackdropArea> {
    with_states(surface, |states| committed_backdrops(states, origin, true))
}

/// The committed backdrops of a surface's `states`, those a subsurface draws only with `subsurface`.
fn committed_backdrops(
    states: &SurfaceData,
    origin: Point<f64, Logical>,
    subsurface: bool,
) -> Vec<BackdropArea> {
    states
        .cached_state
        .get::<BackdropState>()
        .current()
        .entries
        .iter()
        .filter(|e| !subsurface || e.on_subsurface)
        .filter_map(|e| {
            let glass = e.glass.or_else(|| {
                e.material
                    .as_deref()
                    .and_then(glass_material)
                    .map(|style| GlassSpec {
                        style,
                        container: 0,
                        spacing: 0.0,
                    })
            });
            let material = match (&e.material, glass) {
                (Some(m), _) => m.clone(),
                (None, Some(_)) => String::new(),
                (None, None) => return None,
            };
            let s = e.shape;
            (s.width > 0.0 && s.height > 0.0).then(|| BackdropArea {
                key: e.serial,
                material,
                glass,
                area: Rectangle::new(origin + Point::from((s.x, s.y)), (s.width, s.height).into()),
                radius: s.radius,
                alpha: e.alpha,
            })
        })
        .collect()
}

/// One part of a surface tree drawn with the backdrops of its subsurfaces, front to back ([`raised_parts`]).
#[derive(Clone, Debug, PartialEq)]
pub enum TreePart<S> {
    /// Surfaces of the tree drawn together, front to back.
    Surfaces(Vec<S>),
    /// The backdrops of a subsurface, below the part holding its subtree and above the rest: the subsurface and its
    /// rectangle, in logical points from the tree's origin.
    Backdrops {
        surface: S,
        rect: Rectangle<f64, Logical>,
    },
}

/// A surface tree walked front to back: each drawn surface, and the end of the subtree of each subsurface that draws
/// backdrops of its own, which comes right after the last (backmost) surface of that subtree.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeStep<S> {
    Surface(S),
    End {
        surface: S,
        rect: Rectangle<f64, Logical>,
    },
}

/// The parts of a walk (`steps`, front to back): the surfaces between the ends of raised subtrees, and after each such
/// end the subsurface's backdrops. `None` when no subsurface draws backdrops, so the tree draws as one.
pub fn split_tree<S>(steps: Vec<TreeStep<S>>) -> Option<Vec<TreePart<S>>> {
    if !steps.iter().any(|s| matches!(s, TreeStep::End { .. })) {
        return None;
    }
    let mut parts = Vec::new();
    let mut run = Vec::new();
    for step in steps {
        match step {
            TreeStep::Surface(s) => run.push(s),
            TreeStep::End { surface, rect } => {
                if !run.is_empty() {
                    parts.push(TreePart::Surfaces(std::mem::take(&mut run)));
                }
                parts.push(TreePart::Backdrops { surface, rect });
            }
        }
    }
    if !run.is_empty() {
        parts.push(TreePart::Surfaces(run));
    }
    Some(parts)
}

/// The surface tree of `root` split around the backdrops its subsurfaces draw ([`SUBSURFACE_BACKDROPS_VERSION`]):
/// each such subsurface's backdrops lie directly below it and the subsurfaces placed below it, above the rest of the
/// tree. The walk is the one the renderer draws the tree in (only surfaces with a buffer, their children only then).
/// `None` when no subsurface draws backdrops.
pub fn raised_parts(root: &WlSurface) -> Option<Vec<TreePart<WlSurface>>> {
    use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
    use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
    let view = |states: &SurfaceData| {
        states
            .data_map
            .get::<RendererSurfaceStateUserData>()
            .and_then(|d| d.lock().unwrap().view())
    };
    // the processor and the post-filter both add to the walk
    let steps = std::cell::RefCell::new(Vec::new());
    with_surface_tree_downward(
        root,
        Point::<f64, Logical>::from((0.0, 0.0)),
        |_, states, at| match view(states) {
            Some(v) => TraversalAction::DoChildren(*at + v.offset.to_f64()),
            None => TraversalAction::SkipChildren,
        },
        |surface, states, _| {
            if view(states).is_some() {
                steps.borrow_mut().push(TreeStep::Surface(surface.clone()));
            }
        },
        |surface, states, at| {
            if surface == root {
                return true;
            }
            let Some(v) = view(states) else {
                return true;
            };
            let raised = !committed_backdrops(states, (0.0, 0.0).into(), true).is_empty();
            if raised {
                steps.borrow_mut().push(TreeStep::End {
                    surface: surface.clone(),
                    rect: Rectangle::new(*at + v.offset.to_f64(), v.dst.to_f64()),
                });
            }
            true
        },
    );
    split_tree(steps.into_inner())
}

/// Backdrops of a layer surface, in output-local logical coordinates.
pub fn layer_surface_backdrops(map: &LayerMap, layer: &LayerSurface) -> Vec<BackdropArea> {
    let Some(geo) = map.layer_geometry(layer) else {
        return Vec::new();
    };
    surface_backdrops(layer.wl_surface(), geo.loc.to_f64())
}

/// Backdrops of every layer surface of `output`, in output-local logical coordinates.
pub fn layer_backdrops(output: &Output) -> Vec<BackdropArea> {
    let map = layer_map_for_output(output);
    map.layers()
        .flat_map(|l| layer_surface_backdrops(&map, l))
        .collect()
}

/// Per-object data of a backdrop: the surface it belongs to and its identity there. The backdrop of a surface that
/// belongs to another client has no surface and does nothing.
pub struct BackdropData {
    surface: Option<WlSurface>,
    serial: u64,
    /// The client's count of backdrops, given back when this one is destroyed; `None` for an inert backdrop.
    held: Option<Arc<AtomicUsize>>,
}

impl BackdropData {
    /// Edit the backdrop in the surface's pending state; inert once the surface is gone.
    fn edit(&self, f: impl FnOnce(&mut BackdropEntry)) {
        let Some(surface) = self.surface.as_ref().filter(|s| s.is_alive()) else {
            return;
        };
        with_states(surface, |states| {
            let mut cache = states.cached_state.get::<BackdropState>();
            let entry = cache
                .pending()
                .entries
                .iter_mut()
                .find(|e| e.serial == self.serial);
            f(entry.expect(
                "a backdrop stays in its surface's pending state while its resource is alive",
            ));
        });
    }
}

impl GlobalDispatch<CupertinoAppEffectsV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoAppEffectsV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

/// Whether `surface` was created by `client`.
fn owned_by(surface: &WlSurface, client: &Client) -> bool {
    surface.client().is_some_and(|c| c.id() == client.id())
}

impl Dispatch<CupertinoAppEffectsV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        effects: &CupertinoAppEffectsV1,
        request: cupertino_app_effects_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_app_effects_v1::Request::GetBackdrop { id, surface } => {
                let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
                if !owned_by(&surface, client) {
                    init.init(
                        id,
                        BackdropData {
                            surface: None,
                            serial,
                            held: None,
                        },
                    );
                    effects.post_error(
                        EffectsError::InvalidSurface,
                        "the surface belongs to another client",
                    );
                    return;
                }
                // a client without `ClientState` (none is created in this crate) is not counted: it gets a counter of its own each time
                let held = client
                    .get_data::<ClientState>()
                    .map(|data| data.backdrops.clone())
                    .unwrap_or_default();
                let on_surface = with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<BackdropState>()
                        .pending()
                        .entries
                        .len()
                });
                // the count is taken before the check, so a refused request cannot leave it raised
                if held.fetch_add(1, Ordering::SeqCst) >= MAX_BACKDROPS_PER_CLIENT
                    || on_surface >= MAX_BACKDROPS_PER_SURFACE
                {
                    held.fetch_sub(1, Ordering::SeqCst);
                    init.init(
                        id,
                        BackdropData {
                            surface: None,
                            serial,
                            held: None,
                        },
                    );
                    effects.post_error(
                        EffectsError::TooManyBackdrops,
                        format!(
                            "at most {MAX_BACKDROPS_PER_SURFACE} backdrops per surface and {MAX_BACKDROPS_PER_CLIENT} per client"
                        ),
                    );
                    return;
                }
                with_states(&surface, |states| {
                    states
                        .cached_state
                        .get::<BackdropState>()
                        .pending()
                        .entries
                        .push(BackdropEntry {
                            serial,
                            material: None,
                            glass: None,
                            shape: Shape::default(),
                            alpha: 1.0,
                            on_subsurface: effects.version() >= SUBSURFACE_BACKDROPS_VERSION,
                        });
                });
                let resource = init.init(
                    id,
                    BackdropData {
                        surface: Some(surface),
                        serial,
                        held: Some(held),
                    },
                );
                state.backdrop_tones.register(serial, resource);
            }
            cupertino_app_effects_v1::Request::SetTitlebarStyle { surface, style } => {
                if owned_by(&surface, client) {
                    state.set_titlebar_style(effects, &surface, style)
                } else {
                    effects.post_error(
                        EffectsError::InvalidSurface,
                        "the surface belongs to another client",
                    );
                }
            }
            cupertino_app_effects_v1::Request::GetAppearance { id } => {
                state.create_appearance_observer(id, init)
            }
            cupertino_app_effects_v1::Request::GetMenu { id, surface } => {
                let refusal = if !owned_by(&surface, client) {
                    Some((
                        EffectsError::InvalidSurface,
                        "the surface belongs to another client",
                    ))
                } else if state.menu_bars.has_app_menu(&surface) {
                    Some((
                        EffectsError::MenuExists,
                        "the surface already has a menu object",
                    ))
                } else {
                    None
                };
                if let Some((error, message)) = refusal {
                    init.init(id, AppMenuData { surface: None });
                    effects.post_error(error, message);
                    return;
                }
                let menu = init.init(
                    id,
                    AppMenuData {
                        surface: Some(surface.clone()),
                    },
                );
                state.menu_bars.add_app_menu(menu, surface);
            }
            cupertino_app_effects_v1::Request::SetDisabledControls { surface, disabled } => {
                if owned_by(&surface, client) {
                    state.set_disabled_controls(effects, &surface, disabled.into())
                } else {
                    effects.post_error(
                        EffectsError::InvalidSurface,
                        "the surface belongs to another client",
                    );
                }
            }
            // `Destroy` needs nothing: objects created through the global stay valid
            _ => {}
        }
    }
}

impl Cupertino {
    /// `set_titlebar_style`: record the style of a window that has not mapped and size its insets for it.
    /// A window still on the client-side decoration mode keeps no insets; its style applies if it switches.
    fn set_titlebar_style(
        &mut self,
        effects: &CupertinoAppEffectsV1,
        surface: &WlSurface,
        style: WEnum<cupertino_app_effects_v1::TitlebarStyle>,
    ) {
        let style = match style {
            WEnum::Value(cupertino_app_effects_v1::TitlebarStyle::Standard) => {
                TitlebarStyle::Standard
            }
            WEnum::Value(cupertino_app_effects_v1::TitlebarStyle::Unified) => {
                TitlebarStyle::Unified
            }
            WEnum::Value(cupertino_app_effects_v1::TitlebarStyle::UnifiedCompact)
                if effects.version() >= STYLES_VERSION =>
            {
                TitlebarStyle::UnifiedCompact
            }
            WEnum::Value(cupertino_app_effects_v1::TitlebarStyle::Sidebar)
                if effects.version() >= STYLES_VERSION =>
            {
                TitlebarStyle::Sidebar
            }
            other => {
                effects.post_error(
                    EffectsError::Window,
                    format!("{other:?} is not a title bar style"),
                );
                return;
            }
        };
        let Some(i) = self
            .windows
            .iter()
            .position(|w| w.has_surface(surface))
            .filter(|i| !self.windows[*i].mapped)
        else {
            effects.post_error(
                EffectsError::Window,
                "the surface is not an application window that is still unmapped",
            );
            return;
        };
        let insets = self.decoration_insets(style);
        let e = &mut self.windows[i];
        e.titlebar = style;
        if e.server_side {
            e.decor_insets = insets;
        }
    }

    /// `set_disabled_controls`: keep the controls an application window disables in its surface's pending state, for
    /// the surface's next commit.
    fn set_disabled_controls(
        &self,
        effects: &CupertinoAppEffectsV1,
        surface: &WlSurface,
        bits: u32,
    ) {
        if !self.windows.iter().any(|w| w.has_surface(surface)) {
            effects.post_error(
                EffectsError::Window,
                "the surface is not an application window",
            );
            return;
        }
        let undefined = bits & !defined_controls(effects.version()).bits();
        if undefined != 0 {
            effects.post_error(
                EffectsError::InvalidControls,
                format!(
                    "{undefined:#x} names no window control of version {}",
                    effects.version()
                ),
            );
            return;
        }
        let disabled = WindowControl::from_bits_truncate(bits);
        with_states(surface, |states| {
            *states.cached_state.get::<DisabledControls>().pending() = DisabledControls {
                minimize: disabled.contains(WindowControl::Minimize),
                zoom: disabled.contains(WindowControl::Zoom),
            };
        });
    }
}

/// The version that adds the compact unified and the sidebar title bar styles.
const STYLES_VERSION: u32 = 6;

/// Each window control with the version of the global that defines it.
const WINDOW_CONTROLS: [(WindowControl, u32); 2] =
    [(WindowControl::Minimize, 4), (WindowControl::Zoom, 4)];

/// The window controls a `cupertino_app_effects_v1` of `version` may name.
fn defined_controls(version: u32) -> WindowControl {
    WINDOW_CONTROLS
        .iter()
        .filter(|(_, since)| version >= *since)
        .fold(WindowControl::empty(), |defined, (control, _)| {
            defined | *control
        })
}

/// The window controls a client disabled with `set_disabled_controls`, double-buffered with its surface. The commit
/// that applies them to the window reads them with [`disabled_controls`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisabledControls {
    pub minimize: bool,
    pub zoom: bool,
}

impl From<DisabledControls> for cupertino_wm::DisabledControls {
    fn from(d: DisabledControls) -> Self {
        Self {
            minimize: d.minimize,
            zoom: d.zoom,
        }
    }
}

impl Cacheable for DisabledControls {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}

/// The disabled controls `surface` has committed; none for a surface that never set them.
pub fn disabled_controls(surface: &WlSurface) -> DisabledControls {
    with_states(surface, |states| {
        *states.cached_state.get::<DisabledControls>().current()
    })
}

/// The recipe drawn in place of a name that is not a recipe.
const FALLBACK_MATERIAL: &str = "Menu";

/// Log, once per name, that a client asked for a material recipe that does not exist.
fn warn_unknown_material(name: &str) {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // a client could send a new name for every request: stop keeping them past a few
    if seen.len() < 64 && seen.insert(name.to_owned()) {
        tracing::warn!(
            material = name,
            fallback = FALLBACK_MATERIAL,
            "unknown material recipe, using the fallback"
        );
    }
}

/// Set the region of a backdrop, in surface-local logical points; a negative or non-finite value is a protocol error.
fn set_shape(resource: &CupertinoBackdropV1, data: &BackdropData, rect: [f64; 4], radius: f64) {
    let [x, y, width, height] = rect;
    if !rect.iter().chain([&radius]).all(|v| v.is_finite())
        || width < 0.0
        || height < 0.0
        || radius < 0.0
    {
        resource.post_error(
            Error::InvalidShape,
            format!("negative or non-finite size or radius: {width}x{height}, radius {radius}"),
        );
        return;
    }
    data.edit(|e| {
        e.shape = Shape {
            x,
            y,
            width,
            height,
            radius,
        }
    });
}

impl Dispatch<CupertinoBackdropV1, BackdropData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoBackdropV1,
        request: cupertino_backdrop_v1::Request,
        data: &BackdropData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            cupertino_backdrop_v1::Request::SetMaterial { name } => {
                let (appearance, reduce_transparency) = (
                    state.appearance(),
                    state.display_options().reduce_transparency,
                );
                let name = if glass_material(&name).is_none()
                    && !state
                        .render
                        .as_ref()
                        .is_some_and(|r| r.recipe(&name, appearance, reduce_transparency).is_some())
                {
                    // an unknown name is no reason to drop the client: say so once and draw the stand-in recipe
                    warn_unknown_material(&name);
                    FALLBACK_MATERIAL.to_owned()
                } else {
                    name
                };
                // the last of set_material and set_glass wins
                data.edit(|e| {
                    e.material = Some(name);
                    e.glass = None;
                });
            }
            cupertino_backdrop_v1::Request::SetShape {
                x,
                y,
                width,
                height,
                radius,
            } => {
                set_shape(
                    resource,
                    data,
                    [
                        f64::from(x),
                        f64::from(y),
                        f64::from(width),
                        f64::from(height),
                    ],
                    radius,
                );
            }
            cupertino_backdrop_v1::Request::SetShapeFixed {
                x,
                y,
                width,
                height,
                radius,
            } => set_shape(resource, data, [x, y, width, height], radius),
            cupertino_backdrop_v1::Request::SetAlpha { alpha } => {
                if !(0.0..=1.0).contains(&alpha) {
                    resource.post_error(
                        Error::InvalidAlpha,
                        format!("alpha {alpha} is outside 0 to 1"),
                    );
                    return;
                }
                data.edit(|e| e.alpha = alpha);
            }
            cupertino_backdrop_v1::Request::SetGlass {
                style,
                container,
                spacing,
            } => {
                let style = match style {
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Regular) => {
                        Some(GlassStyle::Regular)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Clear) => {
                        Some(GlassStyle::Clear)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::LargePanel)
                        if resource.version() >= LARGE_PANEL_VERSION =>
                    {
                        Some(GlassStyle::LargePanel)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Bar)
                        if resource.version() >= BAR_VERSION =>
                    {
                        Some(GlassStyle::Bar)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Dense)
                        if resource.version() >= DENSE_VERSION =>
                    {
                        Some(GlassStyle::Dense)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Module)
                        if resource.version() >= DENSE_VERSION =>
                    {
                        Some(GlassStyle::Module)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Card)
                        if resource.version() >= CARD_VERSION =>
                    {
                        Some(GlassStyle::Card)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Track)
                        if resource.version() >= CARD_VERSION =>
                    {
                        Some(GlassStyle::Track)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Popover)
                        if resource.version() >= POPOVER_VERSION =>
                    {
                        Some(GlassStyle::Popover)
                    }
                    WEnum::Value(cupertino_backdrop_v1::GlassStyle::Switcher)
                        if resource.version() >= SWITCHER_VERSION =>
                    {
                        Some(GlassStyle::Switcher)
                    }
                    _ => None,
                };
                let (Some(style), true) = (style, spacing >= 0.0 && spacing.is_finite()) else {
                    resource.post_error(
                        Error::InvalidGlass,
                        format!("unknown glass style or negative spacing {spacing}"),
                    );
                    return;
                };
                data.edit(|e| {
                    e.glass = Some(GlassSpec {
                        style,
                        container,
                        spacing,
                    })
                });
            }
            // `Destroy` is handled in `destroyed`
            _ => {}
        }
    }

    /// The backdrop leaves its surface's pending state, so it disappears with the next commit.
    fn destroyed(state: &mut Self, _: ClientId, _: &CupertinoBackdropV1, data: &BackdropData) {
        state.backdrop_tones.forget(data.serial);
        if let Some(held) = &data.held {
            held.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(surface) = data.surface.as_ref().filter(|s| s.is_alive()) {
            with_states(surface, |states| {
                states
                    .cached_state
                    .get::<BackdropState>()
                    .pending()
                    .entries
                    .retain(|e| e.serial != data.serial)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_theme::glass::{FACE_TONE_HYSTERESIS, FACE_TONE_THRESHOLD};

    fn rect(y: f64) -> Rectangle<f64, Logical> {
        Rectangle::new((0.0, y).into(), (100.0, 10.0).into())
    }

    fn surfaces(names: &[&'static str]) -> TreePart<&'static str> {
        TreePart::Surfaces(names.to_vec())
    }

    #[test]
    fn a_tree_whose_subsurfaces_draw_no_backdrops_stays_whole() {
        let steps = vec![TreeStep::Surface("sub"), TreeStep::Surface("root")];
        assert_eq!(split_tree(steps), None);
        assert_eq!(split_tree::<&str>(Vec::new()), None);
    }

    #[test]
    fn a_subsurface_s_backdrops_lie_below_its_subtree_and_above_the_rest() {
        // front to back: a child over the toolbar, the toolbar, a child under it, its end, then the window and a
        // subsurface placed below the window
        let steps = vec![
            TreeStep::Surface("glyphs"),
            TreeStep::Surface("toolbar"),
            TreeStep::Surface("under-toolbar"),
            TreeStep::End {
                surface: "toolbar",
                rect: rect(0.0),
            },
            TreeStep::Surface("root"),
            TreeStep::Surface("below-root"),
        ];
        assert_eq!(
            split_tree(steps),
            Some(vec![
                surfaces(&["glyphs", "toolbar", "under-toolbar"]),
                TreePart::Backdrops {
                    surface: "toolbar",
                    rect: rect(0.0),
                },
                surfaces(&["root", "below-root"]),
            ])
        );
    }

    #[test]
    fn nested_and_sibling_subsurfaces_each_get_their_backdrops_in_place() {
        // a popover subsurface over the toolbar, the toolbar holding a nested subsurface with backdrops of its own,
        // then the window
        let steps = vec![
            TreeStep::Surface("popover"),
            TreeStep::End {
                surface: "popover",
                rect: rect(60.0),
            },
            TreeStep::Surface("button"),
            TreeStep::End {
                surface: "button",
                rect: rect(10.0),
            },
            TreeStep::Surface("toolbar"),
            TreeStep::End {
                surface: "toolbar",
                rect: rect(0.0),
            },
            TreeStep::Surface("root"),
        ];
        let parts = split_tree(steps).unwrap();
        assert_eq!(
            parts,
            vec![
                surfaces(&["popover"]),
                TreePart::Backdrops {
                    surface: "popover",
                    rect: rect(60.0),
                },
                surfaces(&["button"]),
                TreePart::Backdrops {
                    surface: "button",
                    rect: rect(10.0),
                },
                surfaces(&["toolbar"]),
                TreePart::Backdrops {
                    surface: "toolbar",
                    rect: rect(0.0),
                },
                surfaces(&["root"]),
            ]
        );
        // the toolbar is the last surface of the tree: nothing follows its backdrops
        let last = split_tree(vec![
            TreeStep::Surface("root"),
            TreeStep::Surface("toolbar"),
            TreeStep::End {
                surface: "toolbar",
                rect: rect(0.0),
            },
        ])
        .unwrap();
        assert!(matches!(last.last(), Some(TreePart::Backdrops { .. })));
    }

    fn read(serial: u64, luminance: f32) -> ToneReading {
        ToneReading { serial, luminance }
    }

    #[test]
    fn a_tone_is_sent_once_when_the_face_crosses_the_threshold_and_not_while_it_wobbles() {
        let (t, h) = (FACE_TONE_THRESHOLD, FACE_TONE_HYSTERESIS);
        let dark = WireTone::Measured(FaceTone::Dark);
        let light = WireTone::Measured(FaceTone::Light);
        // the first measurement of a backdrop sends its tone
        assert_eq!(
            next_tones([(1, WireTone::Appearance)], &[read(1, 0.05)], &[]),
            vec![(1, dark)]
        );
        // the same tone again sends nothing: a face that changes without crossing costs no event
        assert!(next_tones([(1, dark)], &[read(1, 0.02), read(1, 0.1)], &[]).is_empty());
        // a face moving about the threshold, inside the margin, sends nothing either
        let wobble: Vec<ToneReading> = (0..50)
            .map(|i| read(1, t + (i as f32 * 0.9).sin() * h * 0.9))
            .collect();
        assert!(next_tones([(1, dark)], &wobble, &[]).is_empty());
        assert!(next_tones([(1, light)], &wobble, &[]).is_empty());
        // past the margin it flips, and the last of several readings decides
        assert_eq!(
            next_tones([(1, dark)], &[read(1, t + 2.0 * h)], &[]),
            vec![(1, light)]
        );
        assert_eq!(
            next_tones([(1, dark)], &[read(1, 0.9), read(1, 0.01)], &[]),
            Vec::new()
        );
        // only the backdrop measured changes, and one the server does not track is ignored
        assert_eq!(
            next_tones([(1, dark), (2, dark)], &[read(2, 0.9), read(9, 0.9)], &[]),
            vec![(2, light)]
        );
    }

    #[test]
    fn a_backdrop_drawn_too_large_goes_back_to_the_appearance_once() {
        let light = WireTone::Measured(FaceTone::Light);
        assert_eq!(
            next_tones([(4, light)], &[], &[4]),
            vec![(4, WireTone::Appearance)]
        );
        // one that never had a tone sends nothing
        assert!(next_tones([(4, WireTone::Appearance)], &[], &[4]).is_empty());
    }
}
