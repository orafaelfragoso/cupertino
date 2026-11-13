//! CupertinoKit's [`WindowSystem`] on the transport: one [`Connection`], its windows and popups, and the renderer's
//! frames copied into the transport's buffers.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

use crate::action_grab::ActionGrabs;

use cupertino_geometry::{Rect, Size};
use cupertino_menu_model::{ActionName, Command, Entry, Menu, MenuBar};
use cupertino_modifiers::Profile;
use cupertino_protocol_app::{
    AppEffects, AppEvent, Appearance as AppearanceObserver, Backdrop as Material, BackdropId,
    BackdropTone,
};
use cupertino_ui_menus::{AppMenus, WindowMenus};
use cupertino_ui_platform::{
    ActivationRequest, Backdrop, Capabilities, ClipboardData, ClipboardKind, ClipboardToken, Color,
    ConfigureId, CursorShape, DragImage, DragProgress, DragRequest, DragToken, DropAction,
    DropAnswer, DropEvent, DropToken, GlassRegion, GlassStyle, GlassTone, KeyEvent, Keysym,
    MaterialRegion, MenuDescription, PlatformError, PlatformEvent, PopupSpec, PresentOutcome,
    RaisedLayer, ScrollBars, SystemAppearance, Timestamp, WindowEvent, WindowId, WindowRequest,
    WindowSpec, WindowSystem,
};
use cupertino_ui_render::{CpuFrame, RenderFrameId};
use cupertino_ui_text::TextInputState;
use cupertino_wayland_client::{
    BufferFormat, Capability, ClipboardOffer, Connection, DragActions, DragEvent, DragIcon,
    DragOffer, DragSource, DragSourceEvent, Error, Event, KeyboardEvent, PixelRect, Popup, ReadId,
    SurfaceId, TokenRequest, Toplevel, ToplevelConfig,
};
use rustix::time::ClockId;

use crate::convert::{self, InputTimeline};
use crate::pixels::{convert_rects, rgba_to_argb8888};
use crate::roles::{RoleEvent, SurfaceRoles};

enum Kind {
    Toplevel(Toplevel),
    Popup(Popup),
    /// A surface with a role from [`SurfaceRoles`].
    External,
}

/// What a window whose present was not ready waits for before its next frame is due.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stall {
    Configure,
    Buffer,
}

/// The most drops kept while their data is read, as the transport keeps them.
const MAX_LANDED: usize = 8;

/// A drag from another client, or this one, over a window, and the transport's offer.
struct IncomingDrop {
    token: DropToken,
    offer: DragOffer,
}

/// More rectangles than this carried between presents become one for the whole frame.
const MAX_CARRIED: usize = 64;

/// The most backdrops `cupertino_app_effects_v1` lets one surface hold. The compositor ends the connection of a client
/// that asks for more, so a request past it is refused here and the runtime paints fills instead.
const MAX_BACKDROPS_PER_SURFACE: usize = 64;
/// The most backdrops `cupertino_app_effects_v1` lets one client hold over all its surfaces.
const MAX_BACKDROPS_PER_CLIENT: usize = 128;

/// Whether a surface holding `on_surface` backdrops, of the `held` the client holds in all, stays within the
/// compositor's limits.
fn backdrops_fit(on_surface: usize, held: usize) -> bool {
    on_surface <= MAX_BACKDROPS_PER_SURFACE && held <= MAX_BACKDROPS_PER_CLIENT
}

fn too_many_backdrops() -> PlatformError {
    PlatformError::Protocol(format!(
        "at most {MAX_BACKDROPS_PER_SURFACE} backdrops per surface and {MAX_BACKDROPS_PER_CLIENT} per client"
    ))
}

struct Window {
    id: WindowId,
    surface: SurfaceId,
    kind: Kind,
    parent: Option<WindowId>,
    /// The logical size to draw at when the compositor leaves it to the window.
    size: (u32, u32),
    min_size: Option<Size>,
    max_size: Option<Size>,
    /// Damage of frames that were not presented, carried into the next present.
    carried: Vec<PixelRect>,
    /// The pixel size of the frame shown last; a frame of another size changes all of the window.
    shown: Option<(u32, u32)>,
    /// A frame callback is outstanding: the compositor will say when to draw.
    frame_pending: bool,
    /// The last present was not ready; the next frame waits for this.
    stalled: Option<Stall>,
    /// The runtime asked for a frame.
    frame_wanted: bool,
    /// Presented frames the compositor has not yet said it showed or discarded, oldest first.
    in_flight: VecDeque<RenderFrameId>,
    backdrop: Option<ShownBackdrop>,
    /// The materials under parts of the window, in the order the runtime asked for them.
    materials: Vec<ShownMaterial>,
    /// A toplevel's application id; empty for a popup.
    app_id: String,
    configured: bool,
    menus: Option<ShownMenus>,
    /// An input region to send with the next present; `Some(None)` gives the whole surface back.
    input_region: Option<Option<Vec<Rect>>>,
    /// The surface has the keyboard focus, between its `wl_keyboard.enter` and `leave`.
    keyboard: bool,
    /// A toplevel's activated state as its latest configure stated it; `None` before one, and for other surfaces.
    activated: Option<bool>,
    /// The subsurface the window's raised layer and raised materials are drawn in, once it showed one.
    layer: Option<LayerSurface>,
}

/// The subsurface of a window's raised layer ([`WindowSystem::present_layer`]): at the window's top left corner, as wide
/// as the window, over its content, and taking no input.
struct LayerSurface {
    surface: SurfaceId,
    /// Damage of layer frames that were not shown, carried into the next.
    carried: Vec<PixelRect>,
    /// The pixel size of the layer frame shown last.
    shown: Option<(u32, u32)>,
}

/// A toplevel's menus as the runtime asked for them, and their publication.
struct ShownMenus {
    description: MenuDescription,
    published: Published,
}

enum Published {
    /// The window waits for its first configure.
    Waiting,
    Shown(Box<WindowMenus>),
    /// The desktop takes no menus: no app effects at version 2, or no session bus.
    Refused,
}

/// The material under a window, shaped to the window's size with the next present.
struct ShownBackdrop {
    material: Material,
    name: String,
    radius: f64,
    /// The logical size its shape was last given.
    shaped: Option<(u32, u32)>,
}

/// A material under a part of a window, as last sent; each setter takes effect with the next present.
struct ShownMaterial {
    material: Material,
    region: MaterialRegion,
}

/// Whether the backdrops of a window's raised regions can be drawn above its content, in a subsurface: the transport
/// makes subsurfaces and the desktop draws their backdrops.
fn layers_drawn(conn: &Connection, effects: Option<&AppEffects>) -> bool {
    conn.capabilities().contains(Capability::Subsurface)
        && effects.is_some_and(AppEffects::draws_subsurface_backdrops)
}

/// Gives `backdrop` its look: glass for `glass`, or for a glass material name of the theme, on its own; otherwise the
/// material `name`.
fn set_look(
    backdrop: &Material,
    name: &str,
    glass: Option<GlassRegion>,
) -> Result<(), cupertino_protocol_app::Error> {
    let glass = glass.or_else(|| {
        GlassStyle::of_material(name).map(|style| GlassRegion {
            style,
            container: 0,
            spacing: 0.0,
        })
    });
    match glass {
        Some(g) => backdrop.set_glass(protocol_style(g.style), g.container, g.spacing),
        None => {
            backdrop.set_material(name);
            Ok(())
        }
    }
}

/// The protocol's glass style for a region's.
fn protocol_style(style: GlassStyle) -> cupertino_protocol_app::GlassStyle {
    match style {
        GlassStyle::Regular => cupertino_protocol_app::GlassStyle::Regular,
        GlassStyle::Clear => cupertino_protocol_app::GlassStyle::Clear,
        GlassStyle::LargePanel => cupertino_protocol_app::GlassStyle::LargePanel,
        GlassStyle::Bar => cupertino_protocol_app::GlassStyle::Bar,
        GlassStyle::Dense => cupertino_protocol_app::GlassStyle::Dense,
        GlassStyle::Module => cupertino_protocol_app::GlassStyle::Module,
        GlassStyle::Card => cupertino_protocol_app::GlassStyle::Card,
        GlassStyle::Track => cupertino_protocol_app::GlassStyle::Track,
        GlassStyle::Popover => cupertino_protocol_app::GlassStyle::Popover,
        GlassStyle::Switcher => cupertino_protocol_app::GlassStyle::Switcher,
        GlassStyle::Sidebar => cupertino_protocol_app::GlassStyle::Sidebar,
        GlassStyle::SubduedModule => cupertino_protocol_app::GlassStyle::SubduedModule,
        GlassStyle::Snippet => cupertino_protocol_app::GlassStyle::Snippet,
        GlassStyle::Veiled => cupertino_protocol_app::GlassStyle::Veiled,
        GlassStyle::Toolbar => cupertino_protocol_app::GlassStyle::Toolbar,
        GlassStyle::Menu => cupertino_protocol_app::GlassStyle::Menu,
    }
}

/// Whether window `i` or a popup of its chain has the keyboard focus. A role's configure has no activated state, so
/// this stands for it: the window is the one in use while keys reach it, as a toplevel stays activated while its popup
/// holds the keyboard.
fn holds_keyboard(windows: &[Window], i: usize) -> bool {
    let chain: Vec<_> = windows
        .iter()
        .map(|w| (w.id, w.parent, w.keyboard))
        .collect();
    keyboard_in_chain(&chain, windows[i].id)
}

/// Whether `root` or a window opened from it, through any number of parents, has the keyboard; each window is its
/// id, its parent and whether it has the keyboard.
fn keyboard_in_chain(windows: &[(WindowId, Option<WindowId>, bool)], root: WindowId) -> bool {
    windows.iter().any(|&(id, parent, keyboard)| {
        let (mut at, mut parent) = (id, parent);
        // each popup opens after its parent, so the chain is never longer than the windows open
        for _ in 0..windows.len() {
            if at == root {
                return keyboard;
            }
            let Some(next) = parent else {
                return false;
            };
            at = next;
            parent = windows.iter().find(|w| w.0 == next).and_then(|w| w.1);
        }
        false
    })
}

/// The window and region of the window's material that `backdrop` draws: `None` for the window's own backdrop, or
/// the index of the region under a part of it.
fn glass_region(windows: &[Window], backdrop: BackdropId) -> Option<(WindowId, Option<usize>)> {
    windows.iter().find_map(|w| {
        if w.backdrop
            .as_ref()
            .is_some_and(|b| b.material.id() == backdrop)
        {
            return Some((w.id, None));
        }
        w.materials
            .iter()
            .position(|m| m.material.id() == backdrop)
            .map(|i| (w.id, Some(i)))
    })
}

/// The toolkit's tone for the protocol's.
fn glass_tone(tone: BackdropTone) -> GlassTone {
    match tone {
        BackdropTone::Appearance => GlassTone::Appearance,
        BackdropTone::Dark => GlassTone::Dark,
        BackdropTone::Light => GlassTone::Light,
    }
}

/// Sets the flag [`WindowSystem::dispatch`] reports as [`PlatformEvent::Wake`] and wakes the connection's wait.
struct WakeHandle {
    waker: cupertino_wayland_client::Waker,
    woken: Arc<AtomicBool>,
}

impl Wake for WakeHandle {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Release);
        self.waker.wake();
    }
}

/// The Wayland window system. Physical modifier keys become logical ones through `profile`.
pub struct WaylandWindowSystem {
    conn: Connection,
    /// The time copying frames into buffers took since the runtime last asked, while it times frames.
    copy_time: Option<Duration>,
    semantics: Option<cupertino_protocol_app::Semantics>,
    semantic_surfaces: HashMap<WindowId, cupertino_protocol_app::SemanticSurface>,
    /// The desktop-control actions that came with a grab serial, while a popup of their window may still grab with it.
    action_grabs: ActionGrabs,
    profile: Profile,
    windows: Vec<Window>,
    next_window: u32,
    input_times: InputTimeline,
    selection: Option<ClipboardOffer>,
    next_clipboard: u64,
    /// Clipboard reads in progress, with the MIME type each asked for.
    reads: Vec<(ReadId, ClipboardToken, String)>,
    tokens: Vec<(TokenRequest, ActivationRequest)>,
    next_activation: u64,
    /// The drag started last, while it runs; the transport runs one at a time.
    drag: Option<(DragSource, DragToken)>,
    next_drag: u64,
    /// The drag over a window, until it leaves or drops.
    hovering: Option<IncomingDrop>,
    /// Drops being read, until the runtime finishes or refuses them, oldest first.
    landed: Vec<IncomingDrop>,
    next_drop: u64,
    /// Events produced outside a dispatch (a clipboard read, a frame that needs no wait), returned by the next one.
    queued: Vec<PlatformEvent>,
    woken: Arc<AtomicBool>,
    /// The desktop's effects and the observer of the session's appearance, on a compositor that offers them.
    effects: Option<(AppEffects, AppearanceObserver)>,
    /// The session-bus connection the windows' menus share.
    menus: AppMenus,
    appearance: SystemAppearance,
    roles: Option<Box<dyn SurfaceRoles>>,
    /// Names the configures of surfaces with a role from [`SurfaceRoles`], which carry no serial of their own.
    role_configures: u32,
}

/// Take the session's appearance or display options from `event` into `appearance`; false for any other event, and
/// for a steady caret that did not change (it follows every display options event).
fn apply_appearance(appearance: &mut SystemAppearance, event: &AppEvent) -> bool {
    match event {
        AppEvent::Appearance { dark } => appearance.dark = *dark,
        AppEvent::DisplayOptions(options) => {
            appearance.increase_contrast = options.increase_contrast;
            appearance.reduce_transparency = options.reduce_transparency;
            appearance.reduce_motion = options.reduce_motion;
        }
        AppEvent::SteadyCaret { on } => {
            return std::mem::replace(&mut appearance.steady_caret, *on) != *on;
        }
        _ => return false,
    }
    true
}

/// The compositor's chosen action; `None` for no action.
fn drop_action(action: DragActions) -> Option<DropAction> {
    if action.contains(DragActions::Move) {
        Some(DropAction::Move)
    } else if action.contains(DragActions::Copy) {
        Some(DropAction::Copy)
    } else if action.contains(DragActions::Ask) {
        Some(DropAction::Ask)
    } else {
        None
    }
}

/// The transport's actions for `actions`.
fn drag_actions<'a>(actions: impl IntoIterator<Item = &'a DropAction>) -> DragActions {
    actions
        .into_iter()
        .fold(DragActions::empty(), |all, action| {
            all | match action {
                DropAction::Copy => DragActions::Copy,
                DropAction::Move => DragActions::Move,
                DropAction::Ask => DragActions::Ask,
            }
        })
}

/// The actions in the transport's `actions`.
fn drop_actions(actions: DragActions) -> BTreeSet<DropAction> {
    [
        (DragActions::Copy, DropAction::Copy),
        (DragActions::Move, DropAction::Move),
        (DragActions::Ask, DropAction::Ask),
    ]
    .into_iter()
    .filter(|(bit, _)| actions.contains(*bit))
    .map(|(_, action)| action)
    .collect()
}

/// The transport's icon of a drag image: its RGBA pixels in `wl_shm`'s byte order.
fn drag_icon(image: &DragImage) -> Result<DragIcon, PlatformError> {
    let mut pixels = vec![0; image.pixels.len()];
    rgba_to_argb8888(&image.pixels, &mut pixels);
    DragIcon::new(
        image.width,
        image.height,
        image.scale,
        pixels,
        image.hotspot,
    )
    .ok_or_else(|| PlatformError::Protocol("a drag image the transport cannot show".into()))
}

fn platform_error(e: Error) -> PlatformError {
    match e {
        Error::Disconnected(d) => PlatformError::Disconnected(d.to_string()),
        Error::Unsupported(_) => PlatformError::Unsupported,
        e => PlatformError::Protocol(e.to_string()),
    }
}

impl WaylandWindowSystem {
    /// Connect to the compositor named by the environment.
    pub fn connect(profile: Profile) -> Result<WaylandWindowSystem, PlatformError> {
        Connection::connect()
            .map(|conn| WaylandWindowSystem::new(conn, profile))
            .map_err(platform_error)
    }

    pub fn new(mut conn: Connection, profile: Profile) -> WaylandWindowSystem {
        let effects = AppEffects::bind(&mut conn).map(|effects| {
            let observer = effects.appearance(&conn);
            (effects, observer)
        });
        let semantics = cupertino_protocol_app::Semantics::bind(&mut conn);
        let mut system = WaylandWindowSystem {
            conn,
            copy_time: None,
            semantics,
            semantic_surfaces: HashMap::new(),
            action_grabs: ActionGrabs::default(),
            profile,
            windows: Vec::new(),
            next_window: 0,
            input_times: InputTimeline::default(),
            selection: None,
            next_clipboard: 0,
            reads: Vec::new(),
            tokens: Vec::new(),
            next_activation: 0,
            drag: None,
            next_drag: 0,
            hovering: None,
            landed: Vec::new(),
            next_drop: 0,
            queued: Vec::new(),
            woken: Arc::new(AtomicBool::new(false)),
            effects,
            appearance: SystemAppearance::default(),
            roles: None,
            role_configures: 0,
            menus: AppMenus::default(),
        };
        // the compositor answers the observer at once: learn the session's appearance before the first window draws
        if system.effects.is_some() && system.conn.roundtrip().is_ok() {
            if let Ok(events) = system.conn.wait(Some(Duration::ZERO)) {
                let mut queued = Vec::new();
                for event in events {
                    system.translate(event, &mut queued);
                }
                system.queued = queued;
            }
        }
        if !system
            .effects
            .as_ref()
            .is_some_and(|(effects, _)| effects.tells_session_lock())
        {
            system.queued.push(PlatformEvent::SessionLock(false));
        }
        system
    }

    /// Opens the windows `roles` gives a role, instead of as toplevels.
    pub fn with_roles(mut self, roles: impl SurfaceRoles + 'static) -> WaylandWindowSystem {
        self.roles = Some(Box::new(roles));
        self
    }

    /// The session's appearance and display options as the compositor last reported them; `None` on a compositor
    /// without the desktop's effects.
    pub fn appearance(&self) -> Option<SystemAppearance> {
        self.effects.as_ref().map(|_| self.appearance)
    }

    /// The transport, for an extension protocol such as the desktop's effects.
    pub fn connection(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// The surface behind `window`, for an extension protocol.
    pub fn surface(&self, window: WindowId) -> Option<SurfaceId> {
        self.windows
            .iter()
            .find(|w| w.id == window)
            .map(|w| w.surface)
    }

    /// The window system's clock: the system's monotonic clock, the one the runtime reads, so an event's time (an
    /// input's, a frame's, a presentation's) compares with the runtime's now.
    fn now(&self) -> Timestamp {
        let t = rustix::time::clock_gettime(ClockId::Monotonic);
        Timestamp(
            (t.tv_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(t.tv_nsec as u64),
        )
    }

    fn window(&self, id: WindowId) -> Result<&Window, PlatformError> {
        self.windows
            .iter()
            .find(|w| w.id == id)
            .ok_or(PlatformError::UnknownWindow(id))
    }

    fn window_mut(&mut self, id: WindowId) -> Result<&mut Window, PlatformError> {
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(PlatformError::UnknownWindow(id))
    }

    /// The window `id` hangs from: itself, or the window its popup chain opened from.
    fn root(&self, mut id: WindowId) -> WindowId {
        while let Some(parent) = self.window(id).ok().and_then(|w| w.parent) {
            id = parent;
        }
        id
    }

    /// The grab serial of a desktop-control action on the window `parent` hangs from, while a popup of `parent` may
    /// grab with it: a spent one only for a popup of a popup.
    fn action_grab(&self, parent: WindowId) -> Option<u32> {
        let root = self.root(parent);
        self.action_grabs
            .usable(root, root != parent, Instant::now())
    }

    fn toplevel(&self, id: WindowId) -> Result<Toplevel, PlatformError> {
        match self.window(id)?.kind {
            Kind::Toplevel(t) => Ok(t),
            Kind::Popup(_) | Kind::External => Err(PlatformError::Unsupported),
        }
    }

    fn add(
        &mut self,
        surface: SurfaceId,
        kind: Kind,
        parent: Option<WindowId>,
        size: (u32, u32),
    ) -> WindowId {
        self.next_window += 1;
        let id = WindowId(self.next_window);
        if let Some(publisher) = self
            .semantics
            .as_ref()
            .and_then(|s| s.surface(&self.conn, surface).ok())
        {
            self.semantic_surfaces.insert(id, publisher);
        }
        self.windows.push(Window {
            id,
            surface,
            kind,
            parent,
            size,
            min_size: None,
            max_size: None,
            carried: Vec::new(),
            shown: None,
            frame_pending: false,
            stalled: None,
            frame_wanted: false,
            in_flight: VecDeque::new(),
            backdrop: None,
            materials: Vec::new(),
            app_id: String::new(),
            configured: false,
            menus: None,
            input_region: None,
            keyboard: false,
            activated: None,
            layer: None,
        });
        id
    }

    fn set_menus(
        &mut self,
        window: WindowId,
        description: Option<MenuDescription>,
    ) -> Result<(), PlatformError> {
        self.toplevel(window)?;
        let w = self.window_mut(window)?;
        let Some(description) = description else {
            w.menus = None;
            return Ok(());
        };
        match &mut w.menus {
            Some(ShownMenus {
                description: old,
                published: Published::Shown(menus),
            }) => {
                if old.bar != description.bar {
                    menus.update(&description.bar);
                }
                set_enabled(menus, &old.disabled, &description);
                *old = description;
            }
            Some(shown) => shown.description = description,
            None => {
                w.menus = Some(ShownMenus {
                    description,
                    published: Published::Waiting,
                })
            }
        }
        self.publish_menus(window)
    }

    /// Publish the menus a configured window waits to show.
    fn publish_menus(&mut self, window: WindowId) -> Result<(), PlatformError> {
        let w = self
            .windows
            .iter()
            .find(|w| w.id == window)
            .ok_or(PlatformError::UnknownWindow(window))?;
        let Some(ShownMenus {
            description,
            published: Published::Waiting,
        }) = &w.menus
        else {
            return Ok(());
        };
        if !w.configured {
            return Ok(());
        }
        let menus = self.effects.as_ref().and_then(|(effects, _)| {
            WindowMenus::publish(
                &mut self.menus,
                &self.conn,
                effects,
                w.surface,
                &w.app_id,
                Box::new(self.profile.clone()),
                &description.bar,
            )
        });
        let published = match menus {
            Some(menus) => {
                set_enabled(&menus, &BTreeSet::new(), description);
                Published::Shown(Box::new(menus))
            }
            None => Published::Refused,
        };
        if let Some(shown) = &mut self.window_mut(window)?.menus {
            shown.published = published;
        }
        Ok(())
    }

    /// The backdrops this client holds over all its windows: their own and those under their parts.
    fn backdrops_held(&self) -> usize {
        self.windows
            .iter()
            .map(|w| w.materials.len() + usize::from(w.backdrop.is_some()))
            .sum()
    }

    fn set_backdrop(
        &mut self,
        window: WindowId,
        backdrop: Option<Backdrop>,
    ) -> Result<(), PlatformError> {
        let protocol = |e: cupertino_protocol_app::Error| PlatformError::Protocol(e.to_string());
        let surface = self.window(window)?.surface;
        let shown = match backdrop {
            None => None,
            Some(Backdrop {
                material,
                radius,
                alpha,
            }) => {
                // a fade changes only the alpha of the material already shown
                if let Some(shown) = self
                    .window_mut(window)?
                    .backdrop
                    .as_mut()
                    .filter(|b| b.name == material)
                {
                    shown.material.set_alpha(alpha).map_err(protocol)?;
                    if shown.radius != radius {
                        shown.radius = radius;
                        shown.shaped = None;
                    }
                    return Ok(());
                }
                // the backdrop it replaces is still held while the new one is made
                let w = self.window(window)?;
                if !backdrops_fit(
                    w.materials.len() + usize::from(w.backdrop.is_some()) + 1,
                    self.backdrops_held() + 1,
                ) {
                    return Err(too_many_backdrops());
                }
                let (effects, _) = self.effects.as_ref().ok_or(PlatformError::Unsupported)?;
                let wire = effects
                    .backdrop(&self.conn, surface)
                    .map_err(|e| PlatformError::Protocol(e.to_string()))?;
                set_look(&wire, &material, None).map_err(protocol)?;
                wire.set_alpha(alpha).map_err(protocol)?;
                Some(ShownBackdrop {
                    material: wire,
                    name: material,
                    radius,
                    shaped: None,
                })
            }
        };
        self.window_mut(window)?.backdrop = shown;
        Ok(())
    }

    /// Shows `regions` under the window: each kept backdrop object takes the next region and is told only what
    /// changed, new ones are made for the rest, and those left over are destroyed.
    fn set_materials(
        &mut self,
        window: WindowId,
        regions: Vec<MaterialRegion>,
    ) -> Result<(), PlatformError> {
        let protocol = |e: cupertino_protocol_app::Error| PlatformError::Protocol(e.to_string());
        let surface = self.window(window)?.surface;
        if !regions.is_empty() && self.effects.is_none() {
            return Err(PlatformError::Unsupported);
        }
        let w = self.window(window)?;
        let on_surface = regions.len() + usize::from(w.backdrop.is_some());
        let others = self.backdrops_held() - w.materials.len();
        if !backdrops_fit(on_surface, others + regions.len()) {
            // a window the runtime falls back to fills for shows no material at all
            self.window_mut(window)?.materials.clear();
            return Err(too_many_backdrops());
        }
        let mut shown = std::mem::take(&mut self.window_mut(window)?.materials);
        shown.truncate(regions.len());
        // a region moved between the content and the raised layer is made again on the other surface
        if let Some(moved) = shown
            .iter()
            .zip(&regions)
            .position(|(old, new)| old.region.raised != new.raised)
        {
            shown.truncate(moved);
        }
        let layer = if regions.iter().any(|r| r.raised) {
            Some(self.layer_surface(window)?)
        } else {
            None
        };
        let result = (|| {
            for (i, region) in regions.into_iter().enumerate() {
                if let Some(old) = shown.get_mut(i) {
                    if old.region.material != region.material || old.region.glass != region.glass {
                        set_look(&old.material, &region.material, region.glass)
                            .map_err(protocol)?;
                    }
                    if old.region.alpha != region.alpha {
                        old.material.set_alpha(region.alpha).map_err(protocol)?;
                    }
                    if old.region.rect != region.rect || old.region.radius != region.radius {
                        old.material
                            .set_shape(&region.rect, region.radius)
                            .map_err(protocol)?;
                    }
                    old.region = region;
                    continue;
                }
                let (effects, _) = self.effects.as_ref().ok_or(PlatformError::Unsupported)?;
                let on = match (region.raised, layer) {
                    (true, Some(layer)) => layer,
                    _ => surface,
                };
                let material = effects
                    .backdrop(&self.conn, on)
                    .map_err(|e| PlatformError::Protocol(e.to_string()))?;
                set_look(&material, &region.material, region.glass).map_err(protocol)?;
                material.set_alpha(region.alpha).map_err(protocol)?;
                material
                    .set_shape(&region.rect, region.radius)
                    .map_err(protocol)?;
                shown.push(ShownMaterial { material, region });
            }
            Ok(())
        })();
        // a window the runtime falls back to fills for shows no material at all
        if result.is_err() {
            shown.clear();
        }
        self.window_mut(window)?.materials = shown;
        result
    }

    /// The subsurface of `window`'s raised layer, made the first time it is needed. Its shapes and frames are in the
    /// window's coordinates, as it lies at the window's top left corner.
    fn layer_surface(&mut self, window: WindowId) -> Result<SurfaceId, PlatformError> {
        if !layers_drawn(&self.conn, self.effects.as_ref().map(|(e, _)| e)) {
            return Err(PlatformError::Unsupported);
        }
        let w = self.window(window)?;
        if let Some(layer) = &w.layer {
            return Ok(layer.surface);
        }
        let surface = self.conn.subsurface(w.surface).map_err(platform_error)?;
        self.window_mut(window)?.layer = Some(LayerSurface {
            surface,
            carried: Vec::new(),
            shown: None,
        });
        Ok(surface)
    }

    /// The window at `i` may draw again; whether the frame it asked for meanwhile is due now.
    fn resume(&mut self, i: usize) -> bool {
        let w = &mut self.windows[i];
        w.stalled = None;
        !w.frame_pending && std::mem::take(&mut w.frame_wanted)
    }

    fn send_size_limits(&mut self, window: WindowId) -> Result<Result<(), Error>, PlatformError> {
        let t = self.toplevel(window)?;
        let w = self.window(window)?;
        let (min, max) = (
            w.min_size.map(convert::whole),
            w.max_size.map(convert::whole),
        );
        Ok(self.conn.set_size_limits(t, min, max))
    }

    /// Keep `damage` for the next present.
    fn carry(
        &mut self,
        window: WindowId,
        damage: Vec<PixelRect>,
        frame: &CpuFrame,
    ) -> Result<(), PlatformError> {
        let w = self.window_mut(window)?;
        w.carried.extend(damage);
        bound(&mut w.carried, frame.width, frame.height);
        Ok(())
    }

    /// When a presented frame turned visible, `time_ns` on the compositor's presentation clock, on the window system's
    /// clock; now when that clock cannot be read.
    fn presented_time(&self, time_ns: u64) -> Timestamp {
        let now = self.now().0;
        let clock_now = self
            .conn
            .presentation_clock()
            .and_then(|id| ClockId::try_from(id as i32).ok())
            .map(|id| {
                let t = rustix::time::clock_gettime(id);
                (t.tv_sec as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(t.tv_nsec as u64)
            });
        Timestamp(clock_now.map_or(now, |clock_now| {
            convert::on_window_clock(time_ns, clock_now, now)
        }))
    }

    fn input_time(&mut self, millis: u32) -> Timestamp {
        let now = self.now().0;
        Timestamp(self.input_times.map(millis, now))
    }

    /// The window system's events for one transport event; none for a surface that is not a window here.
    fn translate(&mut self, event: Event, out: &mut Vec<PlatformEvent>) {
        if matches!(&event, Event::Extension(e) if e.downcast_ref::<cupertino_protocol_app::SemanticWake>().is_some())
        {
            out.push(PlatformEvent::Wake);
            return;
        }
        let on = |windows: &[Window], surface: SurfaceId| {
            windows.iter().position(|w| w.surface == surface)
        };
        let mut due = false;
        let (i, event) = match event {
            Event::Configure { window, configure } => {
                let Some(i) = on(&self.windows, window.surface()) else {
                    return;
                };
                if let Some(size) = configure.size {
                    self.windows[i].size = size;
                }
                if !std::mem::replace(&mut self.windows[i].configured, true) {
                    let _ = self.publish_menus(self.windows[i].id);
                }
                due = self.resume(i);
                let configure = convert::configure(&configure);
                self.windows[i].activated = Some(configure.states.activated);
                (i, WindowEvent::Configure(configure))
            }
            Event::PopupConfigure { popup, configure } => {
                let Some(i) = on(&self.windows, popup.surface()) else {
                    return;
                };
                self.windows[i].size = (configure.width, configure.height);
                due = self.resume(i);
                out.push(PlatformEvent::Window {
                    window: self.windows[i].id,
                    event: WindowEvent::PopupPlaced(convert::popup_placement(&configure)),
                });
                (
                    i,
                    WindowEvent::Configure(convert::popup_configure(&configure)),
                )
            }
            Event::Close(window) => match on(&self.windows, window.surface()) {
                Some(i) => (i, WindowEvent::CloseRequested),
                None => return,
            },
            Event::PopupDone(popup) => match on(&self.windows, popup.surface()) {
                Some(i) => (i, WindowEvent::PopupDone),
                None => return,
            },
            Event::Frame(callback) => {
                let Some(i) = on(&self.windows, callback.surface) else {
                    return;
                };
                self.windows[i].frame_pending = false;
                if !self.resume(i) {
                    return;
                }
                // animations step to the time of the frame the compositor showed, not to when it was read
                let time = Timestamp(convert::frame_time(callback.time, self.now().0));
                (i, WindowEvent::Frame(time))
            }
            Event::ScaleChanged { surface, scale } => match on(&self.windows, surface) {
                Some(i) => (i, WindowEvent::Scale(scale.factor())),
                None => return,
            },
            Event::Pointer {
                surface,
                time,
                event,
            } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let serial = self.conn.input_serial().unwrap_or(0);
                let time = self.input_time(time);
                let mods = convert::mods(self.conn.modifiers(), &self.profile);
                (
                    i,
                    WindowEvent::Pointer(convert::pointer(event, serial, mods), time),
                )
            }
            Event::Touch {
                surface,
                time,
                event,
            } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let time = self.input_time(time);
                (i, WindowEvent::Touch(convert::touch(event), time))
            }
            Event::Keyboard {
                surface,
                time,
                event,
            } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                if let (KeyboardEvent::Enter | KeyboardEvent::Leave, Some(roles)) =
                    (&event, self.roles.as_mut())
                {
                    roles.keyboard_focus(surface, matches!(event, KeyboardEvent::Enter));
                }
                match event {
                    KeyboardEvent::Enter => self.windows[i].keyboard = true,
                    KeyboardEvent::Leave => self.windows[i].keyboard = false,
                    KeyboardEvent::Key { .. } => {}
                }
                let event = match event {
                    KeyboardEvent::Enter => WindowEvent::Focus(true),
                    KeyboardEvent::Leave => WindowEvent::Focus(false),
                    KeyboardEvent::Key {
                        code,
                        pressed,
                        repeat,
                        keysym,
                        character,
                        mods,
                    } => WindowEvent::Key(
                        KeyEvent {
                            code,
                            keysym: Keysym(keysym),
                            text: character,
                            pressed,
                            repeat,
                            mods: convert::mods(mods, &self.profile),
                        },
                        self.input_time(time),
                    ),
                };
                (i, event)
            }
            Event::TextInput { surface, event } => match on(&self.windows, surface) {
                Some(i) => (i, WindowEvent::Ime(convert::ime(event))),
                None => return,
            },
            Event::Selection(offer) => {
                let mimes = offer.as_ref().map(|o| o.mime_types().to_vec());
                self.selection = offer;
                out.push(PlatformEvent::ClipboardOffered {
                    kind: ClipboardKind::Clipboard,
                    mimes,
                });
                return;
            }
            Event::Received { id, data } => {
                if let Some(at) = self.reads.iter().position(|(r, _, _)| *r == id) {
                    let (_, token, mime) = self.reads.swap_remove(at);
                    let data = data.map(|bytes| ClipboardData { mime, bytes });
                    out.push(PlatformEvent::Clipboard { token, data });
                }
                return;
            }
            Event::ActivationToken { request, token } => {
                if let Some(at) = self.tokens.iter().position(|(r, _)| *r == request) {
                    let (_, request) = self.tokens.swap_remove(at);
                    out.push(PlatformEvent::ActivationToken {
                        request,
                        token: Some(token),
                    });
                }
                return;
            }
            Event::Extension(extension) => {
                if let Some(role) = self.roles.as_mut().and_then(|r| r.event(&extension)) {
                    let (surface, event) = match role {
                        RoleEvent::Configure { surface, size } => (surface, Some(size)),
                        RoleEvent::Closed(surface) => (surface, None),
                    };
                    let Some(i) = on(&self.windows, surface) else {
                        return;
                    };
                    match event {
                        Some(size) => {
                            self.role_configures = self.role_configures.wrapping_add(1);
                            let configure = convert::role_configure(
                                self.role_configures,
                                size,
                                holds_keyboard(&self.windows, i),
                            );
                            self.windows[i].size = size;
                            due = self.resume(i);
                            (i, WindowEvent::Configure(configure))
                        }
                        None => (i, WindowEvent::CloseRequested),
                    }
                } else {
                    match extension.downcast_ref::<AppEvent>() {
                        Some(AppEvent::SessionLock { locked }) => {
                            out.push(PlatformEvent::SessionLock(*locked));
                        }
                        Some(AppEvent::ScrollBars { always }) => {
                            out.push(PlatformEvent::ScrollBars(if *always {
                                ScrollBars::Always
                            } else {
                                ScrollBars::WhenScrolling
                            }));
                        }
                        Some(event) if apply_appearance(&mut self.appearance, event) => {
                            out.push(PlatformEvent::Appearance(self.appearance));
                        }
                        Some(AppEvent::BackdropTone { backdrop, tone }) => {
                            if let Some((window, region)) = glass_region(&self.windows, *backdrop) {
                                out.push(PlatformEvent::Window {
                                    window,
                                    event: WindowEvent::GlassTone {
                                        region,
                                        tone: glass_tone(*tone),
                                    },
                                });
                            }
                        }
                        Some(event) => {
                            for w in &mut self.windows {
                                if let Some(ShownMenus {
                                    published: Published::Shown(menus),
                                    ..
                                }) = &mut w.menus
                                {
                                    if menus.handle(event) {
                                        break;
                                    }
                                }
                            }
                        }
                        None => {}
                    }
                    return;
                }
            }
            Event::Presented(presented) => {
                let Some(i) = on(&self.windows, presented.surface) else {
                    return;
                };
                let Some(frame) = self.windows[i].in_flight.pop_front() else {
                    return;
                };
                let time = self.presented_time(presented.time_ns);
                (i, WindowEvent::Presented { frame, time })
            }
            Event::Discarded(surface) => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let Some(frame) = self.windows[i].in_flight.pop_front() else {
                    return;
                };
                (i, WindowEvent::Discarded { frame })
            }
            Event::OutputsChanged => {
                if let Some(roles) = self.roles.as_mut() {
                    roles.outputs_changed(&self.conn);
                }
                return;
            }
            Event::DragSource { source, event } => {
                let Some((_, token)) = self.drag.filter(|(s, _)| *s == source) else {
                    return;
                };
                let progress = match event {
                    DragSourceEvent::Target(mime) => DragProgress::Target(mime),
                    DragSourceEvent::Action(action) => DragProgress::Action(drop_action(action)),
                    DragSourceEvent::Dropped => DragProgress::Dropped,
                    DragSourceEvent::Finished => DragProgress::Finished,
                    DragSourceEvent::Cancelled => DragProgress::Cancelled,
                };
                if matches!(progress, DragProgress::Finished | DragProgress::Cancelled) {
                    self.drag = None;
                }
                out.push(PlatformEvent::Drag { token, progress });
                return;
            }
            Event::Drag { surface, event } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let mods = convert::mods(self.conn.modifiers(), &self.profile);
                let event = match event {
                    DragEvent::Enter { offer, point } => {
                        self.next_drop += 1;
                        let token = DropToken(self.next_drop);
                        let event = DropEvent::Enter {
                            token,
                            position: point,
                            mimes: offer.mime_types().to_vec(),
                            actions: drop_actions(offer.source_actions()),
                            mods,
                        };
                        self.hovering = Some(IncomingDrop { token, offer });
                        event
                    }
                    DragEvent::Motion { point, .. } => DropEvent::Motion {
                        position: point,
                        mods,
                    },
                    // after the drop the choice is the window's own, told by finish_drop
                    DragEvent::Action(_) if self.hovering.is_none() => return,
                    DragEvent::Action(action) => DropEvent::Action(drop_action(action)),
                    DragEvent::Leave => {
                        self.hovering = None;
                        DropEvent::Leave
                    }
                    DragEvent::Drop => {
                        let Some(drop) = self.hovering.take() else {
                            return;
                        };
                        if self.landed.len() >= MAX_LANDED {
                            self.landed.remove(0);
                        }
                        self.landed.push(drop);
                        DropEvent::Drop
                    }
                };
                (i, WindowEvent::Drop(event))
            }
            Event::PopupRepositioned { .. } => return,
        };
        let window = self.windows[i].id;
        out.push(PlatformEvent::Window { window, event });
        if due {
            out.push(PlatformEvent::Window {
                window,
                event: WindowEvent::Frame(self.now()),
            });
        }
    }
}

impl WindowSystem for WaylandWindowSystem {
    fn semantics_active(&self, window: WindowId) -> bool {
        self.semantic_surfaces
            .get(&window)
            .is_some_and(|s| s.active())
    }
    fn publish_semantics(
        &mut self,
        window: WindowId,
        snapshot: &cupertino_ui_platform::semantics::Snapshot,
    ) {
        if let Some(s) = self.semantic_surfaces.get(&window) {
            s.publish(snapshot);
        }
    }
    fn semantic_requests(
        &mut self,
    ) -> Vec<(WindowId, cupertino_ui_platform::semantics::ActionCall)> {
        let requests: Vec<_> = self
            .semantic_surfaces
            .iter()
            .flat_map(|(w, s)| s.requests().into_iter().map(|r| (*w, r)))
            .collect();
        requests
            .into_iter()
            .map(|(window, request)| {
                if let Some(serial) = request.grab_serial {
                    let root = self.root(window);
                    self.action_grabs
                        .record(window, root, request.call.id, serial);
                }
                (window, request.call)
            })
            .collect()
    }
    fn semantic_finished(
        &mut self,
        window: WindowId,
        call: &cupertino_ui_platform::semantics::ActionCall,
        outcome: cupertino_ui_platform::semantics::Outcome,
        revision: u64,
    ) {
        // answered as applied, the action's work may still show a popup a moment later; any other answer ends the grab
        let applied = outcome == cupertino_ui_platform::semantics::Outcome::Applied;
        self.action_grabs
            .answered(window, call.id, applied, Instant::now());
        if let Some(s) = self.semantic_surfaces.get(&window) {
            s.complete(call, outcome, revision);
        }
    }

    fn capabilities(&self) -> Capabilities {
        let has = |c| self.conn.capabilities().contains(c);
        Capabilities {
            fractional_scale: has(Capability::FractionalScale),
            presentation_time: has(Capability::Presentation),
            text_input: has(Capability::TextInput),
            touch: false,
            cursor_shape: has(Capability::CursorShape),
            primary_selection: false,
            drag_and_drop: has(Capability::Clipboard) && has(Capability::Seat),
            server_decorations: has(Capability::ServerDecoration),
            activation: has(Capability::Activation),
            app_effects: self.effects.is_some(),
            raised_layer: layers_drawn(&self.conn, self.effects.as_ref().map(|(e, _)| e)),
        }
    }

    fn create_window(&mut self, spec: &WindowSpec) -> Result<WindowId, PlatformError> {
        if let Some(surface) = self
            .roles
            .as_mut()
            .and_then(|roles| roles.create(&mut self.conn, spec))
        {
            let surface = surface.map_err(platform_error)?;
            let id = self.add(surface, Kind::External, None, convert::whole(spec.size));
            self.window_mut(id)?.input_region = spec.input_region.clone().map(Some);
            return Ok(id);
        }
        let toplevel = self
            .conn
            .toplevel(&ToplevelConfig {
                app_id: spec.app_id.clone(),
                title: spec.title.clone(),
                min_size: spec.min_size.map(convert::whole),
                max_size: spec.max_size.map(convert::whole),
                decorations: convert::decorations(spec.decorations),
                foreign_parent: spec.foreign_parent.clone(),
            })
            .map_err(platform_error)?;
        // the style goes with the surface's first commit, before any buffer
        if let (Some(style), Some((effects, _))) = (
            convert::window_style(spec.decorations),
            self.effects.as_ref(),
        ) {
            effects
                .set_window_style(&self.conn, toplevel.surface(), style)
                .map_err(|e| PlatformError::Protocol(e.to_string()))?;
        }
        let id = self.add(
            toplevel.surface(),
            Kind::Toplevel(toplevel),
            None,
            convert::whole(spec.size),
        );
        let w = self.window_mut(id)?;
        (w.min_size, w.max_size) = (spec.min_size, spec.max_size);
        w.app_id = spec.app_id.clone();
        w.input_region = spec.input_region.clone().map(Some);
        Ok(id)
    }

    fn can_grab(&self, parent: WindowId) -> bool {
        self.conn.input_serial().is_some() || self.action_grab(parent).is_some()
    }

    fn create_popup(
        &mut self,
        parent: WindowId,
        spec: &PopupSpec,
    ) -> Result<WindowId, PlatformError> {
        let surface = self.window(parent)?.surface;
        let placement = convert::placement(spec);
        // a menu a desktop-control action shows grabs with the serial the action came with, not a stale or missing one
        let action = self.action_grab(parent).filter(|_| spec.grab);
        let popup = match action {
            Some(serial) => self
                .conn
                .popup_with_grab_serial(surface, &placement, serial),
            None => self.conn.popup(surface, &placement),
        }
        .map_err(platform_error)?;
        // the compositor takes it once, for the menu that opens from a window; the menu's own popups grow its grab
        if let Some(serial) =
            action.filter(|_| self.window(parent).is_ok_and(|w| w.parent.is_none()))
        {
            self.action_grabs.spend(serial);
        }
        Ok(self.add(
            popup.surface(),
            Kind::Popup(popup),
            Some(parent),
            convert::whole(spec.size),
        ))
    }

    fn destroy(&mut self, window: WindowId) {
        self.semantic_surfaces.remove(&window);
        self.action_grabs.forget(window);
        let Ok(surface) = self.window(window).map(|w| w.surface) else {
            return;
        };
        // the announcement goes before its surface
        if let Ok(w) = self.window_mut(window) {
            w.menus = None;
        }
        // the transport destroys a window's popups with it
        let _ = self.conn.destroy(surface);
        if let Some(roles) = self.roles.as_mut() {
            roles.destroyed(surface);
        }
        let mut gone = vec![window];
        while let Some(child) = self
            .windows
            .iter()
            .find(|w| w.parent.is_some_and(|p| gone.contains(&p)) && !gone.contains(&w.id))
        {
            gone.push(child.id);
        }
        self.windows.retain(|w| !gone.contains(&w.id));
    }

    fn request(&mut self, window: WindowId, request: WindowRequest) -> Result<(), PlatformError> {
        let result = match request {
            WindowRequest::Backdrop(backdrop) => return self.set_backdrop(window, backdrop),
            WindowRequest::Materials(regions) => return self.set_materials(window, regions),
            WindowRequest::Menus(menus) => return self.set_menus(window, menus),
            WindowRequest::SetInputRegion(region) => {
                self.window_mut(window)?.input_region = Some(region);
                return Ok(());
            }
            WindowRequest::SetTitle(title) => {
                let t = self.toplevel(window)?;
                self.conn.set_title(t, &title)
            }
            WindowRequest::SetMinSize(size) => {
                self.window_mut(window)?.min_size = size;
                self.send_size_limits(window)?
            }
            WindowRequest::SetMaxSize(size) => {
                self.window_mut(window)?.max_size = size;
                self.send_size_limits(window)?
            }
            WindowRequest::Move => {
                let t = self.toplevel(window)?;
                self.conn.start_move(t)
            }
            WindowRequest::Resize(edge) => {
                let t = self.toplevel(window)?;
                self.conn.start_resize(t, convert::resize_edge(edge))
            }
            WindowRequest::ShowWindowMenu(at) => {
                let t = self.toplevel(window)?;
                self.conn.show_window_menu(t, at)
            }
            WindowRequest::Maximize(on) => {
                let t = self.toplevel(window)?;
                self.conn.set_maximized(t, on)
            }
            WindowRequest::Fullscreen(on) => {
                let t = self.toplevel(window)?;
                self.conn.set_fullscreen(t, on)
            }
            WindowRequest::Minimize => {
                let t = self.toplevel(window)?;
                self.conn.minimize(t)
            }
            WindowRequest::Activate(None) => {
                let t = self.toplevel(window)?;
                self.conn.activate(t)
            }
            WindowRequest::Activate(Some(token)) => {
                let t = self.toplevel(window)?;
                self.conn.activate_with_token(t, &token)
            }
            WindowRequest::Reposition(spec) => {
                let Kind::Popup(p) = self.window(window)?.kind else {
                    return Err(PlatformError::Unsupported);
                };
                self.window_mut(window)?.size = convert::whole(spec.size);
                self.conn
                    .reposition_popup(p, &convert::placement(&spec))
                    .map(drop)
            }
        };
        result.map_err(platform_error)
    }

    fn set_text_input(&mut self, window: WindowId, state: Option<&TextInputState>) {
        let Ok(surface) = self.window(window).map(|w| w.surface) else {
            return;
        };
        let state = state.map(convert::text_input);
        // without the protocol the field types through key events alone
        let _ = self.conn.set_text_input(surface, state.as_ref());
    }

    fn set_cursor(&mut self, _window: WindowId, shape: Option<CursorShape>) {
        // before the pointer enters the compositor's cursor stays
        let _ = match shape {
            Some(shape) => self.conn.set_cursor(convert::cursor(shape)),
            None => self.conn.hide_cursor(),
        };
    }

    /// The drag answers the button press that started the implicit grab still held in `window`'s client (the first of
    /// the buttons down); one the transport refuses (no button held) fails here, and one the compositor ignores ends as [`DragProgress::Cancelled`]. A drag started while
    /// another runs replaces it, which ends as cancelled. An image the transport cannot show is left out.
    fn start_drag(
        &mut self,
        window: WindowId,
        request: DragRequest,
    ) -> Result<DragToken, PlatformError> {
        let surface = self.window(window)?.surface;
        // an image the transport cannot show (a side over its limit at this scale) leaves the drag without one
        let icon = request.image.as_ref().and_then(|image| {
            drag_icon(image)
                .inspect_err(|e| {
                    eprintln!("cupertino-ui-wayland: the drag goes without its image: {e:?}")
                })
                .ok()
        });
        let actions = drag_actions(&request.actions);
        let contents = request
            .data
            .into_iter()
            .map(|d| (d.mime, d.bytes))
            .collect();
        let source = self
            .conn
            .start_drag_with_icon(surface, contents, actions, icon.as_ref())
            .map_err(platform_error)?;
        self.next_drag += 1;
        let token = DragToken(self.next_drag);
        if let Some((_, replaced)) = self.drag.replace((source, token)) {
            self.queued.push(PlatformEvent::Drag {
                token: replaced,
                progress: DragProgress::Cancelled,
            });
        }
        Ok(token)
    }

    /// Accepts the drag in the answer's type, or refuses it, and names the actions and the preferred one; the
    /// compositor's choice arrives as [`DropEvent::Action`] (a compositor older than drag actions copies).
    fn answer_drop(&mut self, token: DropToken, answer: &DropAnswer) -> Result<(), PlatformError> {
        let offer = match &self.hovering {
            Some(drop) if drop.token == token => drop.offer.clone(),
            _ => return Err(PlatformError::Protocol("no such drag over a window".into())),
        };
        self.conn
            .accept_drag(&offer, answer.mime.as_deref())
            .map_err(platform_error)?;
        let preferred = drag_actions(answer.preferred.as_ref());
        self.conn
            .set_drag_actions(&offer, drag_actions(&answer.actions), preferred)
            .map_err(platform_error)?;
        self.conn.flush().map_err(platform_error)
    }

    /// The read runs in the transport's loop like a clipboard read, with its limits.
    fn read_drop(&mut self, token: DropToken, mime: &str) -> Result<ClipboardToken, PlatformError> {
        let offer = self
            .landed
            .iter()
            .find(|d| d.token == token)
            .map(|d| d.offer.clone())
            .ok_or_else(|| PlatformError::Protocol("no such drop".into()))?;
        let id = self
            .conn
            .receive_drop(&offer, mime)
            .map_err(platform_error)?;
        self.next_clipboard += 1;
        let read = ClipboardToken(self.next_clipboard);
        self.reads.push((id, read, mime.to_owned()));
        Ok(read)
    }

    /// The source is told the action the drop was taken with just before it finishes, whatever was chosen during the
    /// drag (an ask, or a move where the window copied), so a source never removes data the window only copied. One
    /// taken as an ask copies; one taken with an action the source does not allow is refused, and the source keeps its
    /// data.
    fn finish_drop(&mut self, token: DropToken, action: Option<DropAction>) {
        let Some(at) = self.landed.iter().position(|d| d.token == token) else {
            return;
        };
        let drop = self.landed.remove(at);
        let last = action.map(|action| match action {
            DropAction::Ask => DropAction::Copy,
            other => other,
        });
        let allowed =
            last.filter(|last| drop.offer.source_actions().contains(drag_actions([last])));
        // a drop whose surface or offer is gone is already over
        let _ = match allowed {
            Some(last) => {
                // a compositor tells the source only a change of action, so this costs nothing when it agrees
                let bits = drag_actions([&last]);
                let _ = self.conn.set_drag_actions(&drop.offer, bits, bits);
                self.conn.finish_drag(&drop.offer)
            }
            None => self.conn.refuse_drop(&drop.offer),
        };
    }

    fn set_clipboard(&mut self, kind: ClipboardKind, data: ClipboardData) {
        if kind == ClipboardKind::Clipboard {
            let _ = self.conn.set_selection(vec![(data.mime, data.bytes)]);
        }
    }

    /// The read runs in the transport's loop while dispatch goes on; its data arrives with the dispatch it ends in (the
    /// transport gives up on a source silent for a second, or after ten seconds). One that can't start answers `None`
    /// with the next dispatch.
    fn read_clipboard(&mut self, kind: ClipboardKind, mime: &str) -> ClipboardToken {
        self.next_clipboard += 1;
        let token = ClipboardToken(self.next_clipboard);
        let started = match (&self.selection, kind) {
            (Some(offer), ClipboardKind::Clipboard) => {
                let offer = offer.clone();
                self.conn.receive_async(&offer, mime).ok()
            }
            _ => None,
        };
        match started {
            Some(id) => self.reads.push((id, token, mime.to_owned())),
            None => self
                .queued
                .push(PlatformEvent::Clipboard { token, data: None }),
        }
        token
    }

    fn request_activation_token(
        &mut self,
        window: WindowId,
        app_id: Option<&str>,
    ) -> ActivationRequest {
        self.next_activation += 1;
        let request = ActivationRequest(self.next_activation);
        let surface = self.window(window).map(|w| w.surface).ok();
        let asked =
            surface.and_then(|surface| self.conn.request_activation_token(surface, app_id).ok());
        match asked {
            Some(wire) => self.tokens.push((wire, request)),
            None => self.queued.push(PlatformEvent::ActivationToken {
                request,
                token: None,
            }),
        }
        request
    }

    /// With no frame callback outstanding and no present waiting for a configure or a free buffer nothing paces the
    /// window, so the frame is due at once.
    fn activated(&self, window: WindowId) -> Option<bool> {
        self.window(window).ok().and_then(|w| w.activated)
    }

    fn time_copies(&mut self, on: bool) {
        self.copy_time = on.then_some(Duration::ZERO);
    }

    fn take_copy_time(&mut self) -> Duration {
        self.copy_time
            .as_mut()
            .map_or(Duration::ZERO, std::mem::take)
    }

    fn request_frame(&mut self, window: WindowId) {
        let now = self.now();
        let Ok(w) = self.window_mut(window) else {
            return;
        };
        if w.frame_pending || w.stalled.is_some() {
            w.frame_wanted = true;
        } else {
            let id = w.id;
            self.queued.push(PlatformEvent::Window {
                window: id,
                event: WindowEvent::Frame(now),
            });
        }
    }

    /// The transport acknowledges the latest configure with every frame, so `configure` needs no request of its own; a
    /// frame drawn at another size than the latest configure asks is not ready.
    fn present(
        &mut self,
        window: WindowId,
        frame: &CpuFrame,
        _configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        let w = self.window_mut(window)?;
        let (surface, size) = (w.surface, w.size);
        if let Some(b) = w.backdrop.as_mut().filter(|b| b.shaped != Some(size)) {
            let bounds = Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1));
            if b.material.set_shape(&bounds, b.radius).is_ok() {
                b.shaped = Some(size);
            }
        }
        let resized = w.shown != Some((frame.width, frame.height));
        let mut damage = std::mem::take(&mut w.carried);
        damage.extend(frame.damage.iter().map(|r| PixelRect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }));
        if let Some(region) = w.input_region.take() {
            self.conn
                .set_input_region(surface, region.as_deref())
                .map_err(platform_error)?;
        }
        let copy_time = self.copy_time.as_mut();
        match copy_and_present(
            &mut self.conn,
            surface,
            size,
            frame,
            &damage,
            resized,
            copy_time,
        )? {
            Ok(()) => {
                let feedback = self.conn.capabilities().contains(Capability::Presentation);
                let w = self.window_mut(window)?;
                if feedback {
                    w.in_flight.push_back(frame.id);
                }
                w.frame_pending = true;
                w.shown = Some((frame.width, frame.height));
                Ok(PresentOutcome::Presented)
            }
            Err(stall) => {
                self.carry(window, damage, frame)?;
                self.window_mut(window)?.stalled = stall;
                Ok(PresentOutcome::NotReady)
            }
        }
    }

    /// The layer's subsurface commits now, synchronized: what it shows applies with the window's next commit.
    fn present_layer(
        &mut self,
        window: WindowId,
        layer: Option<RaisedLayer<'_>>,
    ) -> Result<PresentOutcome, PlatformError> {
        let Some(layer) = layer else {
            if let Some(shown) = self.window_mut(window)?.layer.as_mut() {
                if shown.shown.take().is_some() {
                    shown.carried.clear();
                    let surface = shown.surface;
                    self.conn
                        .unmap_subsurface(surface)
                        .map_err(platform_error)?;
                }
            }
            return Ok(PresentOutcome::Presented);
        };
        let surface = self.layer_surface(window)?;
        let width = self.window(window)?.size.0;
        let size = (width, layer.height);
        self.conn
            .place_subsurface(surface, (0, 0), size)
            .map_err(platform_error)?;
        let frame = layer.frame;
        let state = self.window_mut(window)?.layer.as_mut().expect("made above");
        let resized = state.shown != Some((frame.width, frame.height));
        let mut damage = std::mem::take(&mut state.carried);
        damage.extend(frame.damage.iter().map(|r| PixelRect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }));
        let copy_time = self.copy_time.as_mut();
        match copy_and_present(
            &mut self.conn,
            surface,
            size,
            frame,
            &damage,
            resized,
            copy_time,
        )? {
            Ok(()) => {
                let state = self.window_mut(window)?.layer.as_mut().expect("made above");
                state.shown = Some((frame.width, frame.height));
                Ok(PresentOutcome::Presented)
            }
            Err(_) => {
                let state = self.window_mut(window)?.layer.as_mut().expect("made above");
                state.carried.extend(damage);
                bound(&mut state.carried, frame.width, frame.height);
                Ok(PresentOutcome::NotReady)
            }
        }
    }

    /// One pixel of `color` the compositor stretches over the window ([`Connection::present_solid`]), with the input
    /// region waiting to be sent; refused where the compositor has no one-pixel buffers. The next frame presented is
    /// whole.
    fn present_solid(
        &mut self,
        window: WindowId,
        color: Color,
        _configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        if !self.conn.capabilities().contains(Capability::SolidColor) {
            return Err(PlatformError::Unsupported);
        }
        let w = self.window_mut(window)?;
        let (surface, size) = (w.surface, w.size);
        if let Some(b) = w.backdrop.as_mut().filter(|b| b.shaped != Some(size)) {
            let bounds = Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1));
            if b.material.set_shape(&bounds, b.radius).is_ok() {
                b.shaped = Some(size);
            }
        }
        if let Some(region) = w.input_region.take() {
            self.conn
                .set_input_region(surface, region.as_deref())
                .map_err(platform_error)?;
        }
        match self.conn.present_solid(surface, size, color.to_array()) {
            Ok(()) => {
                let w = self.window_mut(window)?;
                w.frame_pending = true;
                w.shown = None;
                w.carried.clear();
                Ok(PresentOutcome::Presented)
            }
            Err(Error::NotConfigured) => {
                self.window_mut(window)?.stalled = Some(Stall::Configure);
                Ok(PresentOutcome::NotReady)
            }
            Err(Error::Unsupported(_)) => Err(PlatformError::Unsupported),
            Err(e) => Err(platform_error(e)),
        }
    }

    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError> {
        if let Some(roles) = self.roles.as_mut() {
            roles.prepare(&mut self.conn);
        }
        let ready = !self.queued.is_empty() || self.woken.load(Ordering::Acquire);
        events.append(&mut self.queued);
        let wire = self
            .conn
            .wait(if ready { Some(Duration::ZERO) } else { timeout })
            .map_err(platform_error)?;
        for event in wire {
            self.translate(event, events);
        }
        for w in &self.windows {
            if let Some(ShownMenus {
                published: Published::Shown(menus),
                ..
            }) = &w.menus
            {
                events.extend(menus.activations().map(|a| PlatformEvent::Window {
                    window: w.id,
                    event: WindowEvent::MenuActivated(a.action),
                }));
            }
        }
        for i in 0..self.windows.len() {
            let w = &self.windows[i];
            if w.stalled == Some(Stall::Buffer)
                && matches!(self.conn.buffer_free(w.surface), Ok(true))
                && self.resume(i)
            {
                events.push(PlatformEvent::Window {
                    window: self.windows[i].id,
                    event: WindowEvent::Frame(self.now()),
                });
            }
        }
        if self.woken.swap(false, Ordering::AcqRel) {
            events.push(PlatformEvent::Wake);
        }
        Ok(())
    }

    fn waker(&self) -> Waker {
        Waker::from(Arc::new(WakeHandle {
            waker: self.conn.waker(),
            woken: Arc::clone(&self.woken),
        }))
    }
}

/// Bring the published items from `was` disabled to those `now` disables, leaving out what its bar does not show.
fn set_enabled(menus: &WindowMenus, was: &BTreeSet<ActionName>, now: &MenuDescription) {
    let shown = actions(&now.bar);
    for action in was.union(&now.disabled).filter(|a| shown.contains(a)) {
        menus.set_enabled(action, !now.disabled.contains(action));
    }
}

/// The actions of the items `bar` publishes.
fn actions(bar: &MenuBar) -> BTreeSet<ActionName> {
    fn walk(menu: &Menu, out: &mut BTreeSet<ActionName>) {
        for entry in menu.sections.iter().flatten() {
            match entry {
                Entry::Item(item) => {
                    if let Command::Action(action) = &item.command {
                        out.insert(action.clone());
                    }
                }
                Entry::Submenu(menu) => walk(menu, out),
            }
        }
    }
    let mut out = BTreeSet::new();
    for menu in std::iter::once(&bar.app).chain(&bar.menus) {
        walk(menu, &mut out);
    }
    out
}

/// Copy `damage` of `frame`, and whatever the buffer missed (all of it when that is unknown), into a buffer of `surface`
/// and present it, telling the compositor that `damage` changed, or everything when the window was `resized`. When it
/// is not presented, what the window waits for: a free buffer or its first configure; nothing for a frame drawn at
/// another size than the surface asks for, which a frame at that size replaces.
fn copy_and_present(
    conn: &mut Connection,
    surface: SurfaceId,
    size: (u32, u32),
    frame: &CpuFrame,
    damage: &[PixelRect],
    resized: bool,
    copy_time: Option<&mut Duration>,
) -> Result<Result<(), Option<Stall>>, PlatformError> {
    let mut lease = match conn.lease(surface, size, BufferFormat::Argb8888) {
        Ok(lease) => lease,
        Err(Error::NoFreeBuffer) => return Ok(Err(Some(Stall::Buffer))),
        Err(Error::NotConfigured) => return Ok(Err(Some(Stall::Configure))),
        Err(e) => return Err(platform_error(e)),
    };
    let layout = lease.layout();
    if (layout.width(), layout.height()) != (frame.width, frame.height) {
        return Ok(Err(None));
    }
    let inside = |rects: &mut dyn Iterator<Item = &PixelRect>| -> Vec<PixelRect> {
        rects
            .filter_map(|r| clip(*r, frame.width, frame.height))
            .collect()
    };
    let changed = inside(&mut damage.iter());
    let copy = lease
        .missed()
        .map(|missed| inside(&mut changed.iter().chain(missed)));
    let whole = [PixelRect {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
    }];
    let copying = copy_time.is_some().then(Instant::now);
    convert_rects(
        &frame.pixels,
        frame.stride as usize,
        lease.pixels_mut(),
        layout.stride(),
        copy.as_deref().unwrap_or(&whole),
    );
    if let (Some(total), Some(since)) = (copy_time, copying) {
        *total += since.elapsed();
    }
    lease
        .present_damage((!resized).then_some(&changed[..]))
        .map_err(platform_error)?;
    Ok(Ok(()))
}

/// Replace more than [`MAX_CARRIED`] rectangles by one for the whole `width` × `height` frame.
fn bound(carried: &mut Vec<PixelRect>, width: u32, height: u32) {
    if carried.len() > MAX_CARRIED {
        *carried = vec![PixelRect {
            x: 0,
            y: 0,
            width,
            height,
        }];
    }
}

/// `r` cut to a `width` × `height` buffer; `None` when nothing is left.
fn clip(r: PixelRect, width: u32, height: u32) -> Option<PixelRect> {
    let (x1, y1) = (
        r.x.saturating_add(r.width).min(width),
        r.y.saturating_add(r.height).min(height),
    );
    (r.x < x1 && r.y < y1).then(|| PixelRect {
        x: r.x,
        y: r.y,
        width: x1 - r.x,
        height: y1 - r.y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_and_a_track_ask_for_the_protocol_s_card_and_track_glass() {
        use cupertino_protocol_app::GlassStyle as Wire;
        assert_eq!(protocol_style(GlassStyle::Card), Wire::Card);
        assert_eq!(protocol_style(GlassStyle::Track), Wire::Track);
        assert_eq!(protocol_style(GlassStyle::Popover), Wire::Popover);
        assert_eq!(protocol_style(GlassStyle::Switcher), Wire::Switcher);
        assert_eq!(protocol_style(GlassStyle::Sidebar), Wire::Sidebar);
        assert_eq!(
            protocol_style(GlassStyle::SubduedModule),
            Wire::SubduedModule
        );
        assert_eq!(protocol_style(GlassStyle::Snippet), Wire::Snippet);
        assert_eq!(protocol_style(GlassStyle::Veiled), Wire::Veiled);
        assert_eq!(protocol_style(GlassStyle::Toolbar), Wire::Toolbar);
        assert_eq!(protocol_style(GlassStyle::Module), Wire::Module);
        // a material named for them stands for that glass
        assert_eq!(GlassStyle::of_material("CardGlass"), Some(GlassStyle::Card));
        assert_eq!(
            GlassStyle::of_material("TrackGlass"),
            Some(GlassStyle::Track)
        );
    }

    #[test]
    fn the_menu_s_material_name_asks_for_menu_glass() {
        use cupertino_protocol_app::GlassStyle as Wire;
        let style = GlassStyle::of_material("MenuGlass").unwrap();
        assert_eq!(protocol_style(style), Wire::Menu);
    }

    #[test]
    fn the_sidebar_s_material_name_asks_for_sidebar_glass() {
        use cupertino_protocol_app::GlassStyle as Wire;
        let style = GlassStyle::of_material("SidebarGlass").unwrap();
        assert_eq!(protocol_style(style), Wire::Sidebar);
    }

    #[test]
    fn the_switcher_s_material_name_asks_for_switcher_glass() {
        use cupertino_protocol_app::GlassStyle as Wire;
        let style = GlassStyle::of_material("SwitcherGlass").unwrap();
        assert_eq!(protocol_style(style), Wire::Switcher);
    }

    #[test]
    fn the_session_appearance_and_display_options_make_one_system_appearance() {
        use cupertino_defaults::accessibility::DisplayOptions;
        let mut appearance = SystemAppearance::default();
        assert!(apply_appearance(
            &mut appearance,
            &AppEvent::Appearance { dark: true }
        ));
        let options = DisplayOptions {
            reduce_motion: true,
            reduce_transparency: true,
            increase_contrast: true,
            differentiate_without_color: true,
        };
        assert!(apply_appearance(
            &mut appearance,
            &AppEvent::DisplayOptions(options)
        ));
        assert_eq!(
            appearance,
            SystemAppearance {
                dark: true,
                increase_contrast: true,
                reduce_transparency: true,
                reduce_motion: true,
                steady_caret: false,
            }
        );
        assert!(
            !apply_appearance(&mut appearance, &AppEvent::SteadyCaret { on: false }),
            "the caret was already blinking"
        );
        assert!(apply_appearance(
            &mut appearance,
            &AppEvent::SteadyCaret { on: true }
        ));
        assert!(appearance.steady_caret);
        assert!(apply_appearance(
            &mut appearance,
            &AppEvent::Appearance { dark: false }
        ));
        assert!(!appearance.dark);
        assert!(
            appearance.increase_contrast,
            "a change of appearance keeps the display options"
        );
        let before = appearance;
        assert!(!apply_appearance(
            &mut appearance,
            &AppEvent::ScrollCount {
                vertical: true,
                count: 1
            }
        ));
        assert_eq!(appearance, before);
    }

    fn r(x: u32) -> PixelRect {
        PixelRect {
            x,
            y: 0,
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn carried_damage_beyond_the_limit_becomes_the_whole_frame() {
        let mut carried: Vec<PixelRect> = (0..MAX_CARRIED as u32).map(r).collect();
        bound(&mut carried, 200, 100);
        assert_eq!(carried.len(), MAX_CARRIED);
        carried.push(r(99));
        bound(&mut carried, 200, 100);
        assert_eq!(
            carried,
            [PixelRect {
                x: 0,
                y: 0,
                width: 200,
                height: 100
            }]
        );
    }

    #[test]
    fn damage_is_cut_to_the_frame() {
        let wide = PixelRect {
            x: 190,
            y: 95,
            width: 50,
            height: 50,
        };
        assert_eq!(
            clip(wide, 200, 100),
            Some(PixelRect {
                x: 190,
                y: 95,
                width: 10,
                height: 5
            })
        );
        assert_eq!(clip(r(200), 200, 100), None);
    }

    #[test]
    fn a_window_holds_the_keyboard_while_it_or_a_popup_of_its_chain_has_it() {
        let (bar, menu, submenu, other) = (WindowId(1), WindowId(2), WindowId(3), WindowId(4));
        let open = |keyboard: WindowId| {
            [
                (bar, None, bar == keyboard),
                (menu, Some(bar), menu == keyboard),
                (submenu, Some(menu), submenu == keyboard),
                (other, None, other == keyboard),
            ]
        };
        assert!(keyboard_in_chain(&open(bar), bar));
        assert!(keyboard_in_chain(&open(submenu), bar));
        assert!(keyboard_in_chain(&open(submenu), menu));
        assert!(!keyboard_in_chain(&open(menu), submenu));
        assert!(!keyboard_in_chain(&open(other), bar));
        assert!(!keyboard_in_chain(&open(WindowId(9)), bar));
    }
}
