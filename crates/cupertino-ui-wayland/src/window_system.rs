//! CupertinoKit's [`WindowSystem`] on the transport: one [`Connection`], its windows and popups, and the renderer's
//! frames copied into the transport's buffers.

use std::any::Any;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

use crate::action_grab::ActionGrabs;

use crate::menus::{AppMenus, WindowMenus};
use cupertino_diagnostic_events::{self as diag, profile as timeline};
use cupertino_geometry::{Rect, Size};
use cupertino_keyboard::modifiers::Profile;
use cupertino_menu_model::{ActionName, Command, Entry, Menu, MenuBar};
use cupertino_ui_core::platform::{
    ActivationRequest, Backdrop, BlurRamp, Capabilities, ClipboardData, ClipboardKind,
    ClipboardToken, Color, ConfigureId, CursorShape, DragImage, DragProgress, DragRequest,
    DragToken, DropAction, DropAnswer, DropEvent, DropToken, GlassRegion, GlassStyle, GlassTone,
    KeyEvent, Keysym, LayerList, MaterialRegion, MenuDescription, PlatformError, PlatformEvent,
    PopupSpec, PresentOutcome, RaisedChange, RaisedLayer, RendererClass, ScrollBars, Shown,
    Submission, Submitted, SurfaceLayerLook, SystemAppearance, Timestamp, WindowEvent, WindowId,
    WindowRequest, WindowSpec, WindowSystem,
};
use cupertino_ui_core::WindowKey;
use cupertino_ui_render::render_thread;
use cupertino_ui_render::{CpuFrame, RenderFrameId};
use cupertino_ui_text::TextInputState;
use cupertino_wayland_client::app::{
    AppEffects, AppEvent, Appearance as AppearanceObserver, Backdrop as Material, BackdropId,
    BackdropTone,
};
use cupertino_wayland_client::{
    BufferFormat, Capability, ClipboardOffer, Connection, DragActions, DragEvent, DragIcon,
    DragOffer, DragSource, DragSourceEvent, Error, Event, KeyboardEvent, Popup, ReadId, SurfaceId,
    TokenRequest, Toplevel, ToplevelConfig,
};
use rustix::time::ClockId;

use crate::convert::{self, InputTimeline};
use crate::gpu_path::{GpuPath, GpuRenderers, Prepared};
use crate::gpu_present::PresentError;
use crate::hand_over::{self, Back, Drew, Failure, Of, Part, Target};
use crate::pixels::rgba_to_argb8888;
use crate::presentations::Presentations;
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

/// A frame a window submitted ([`WindowSystem::submit`]), drawn away from the UI thread, sent back before its commits
/// go out, so the window system hears of it before the compositor's frame callback and feedback for it: how each of its
/// surfaces was drawn, and the panic of a renderer that panicked, which goes on on the UI thread that submitted the
/// frame.
struct Finished {
    window: WindowId,
    backs: Vec<Back>,
    panic: Option<Box<dyn Any + Send>>,
}

/// What the thread a submitted frame was handed to sends back, in this order.
enum FromJob {
    Finished(Finished),
    /// Every commit of the window's frame went out, or none was to: the window may draw again.
    Sent(WindowId),
}

/// How often a window waiting for a GPU buffer looks at its release points.
const GPU_RELEASE_POLL: Duration = Duration::from_millis(2);

/// How long the run loop waits for the wire: the runtime's `timeout`, cut to [`GPU_RELEASE_POLL`] while a window
/// waits for a GPU buffer the compositor can release by itself ([`polls_release`]; a release point signals through an
/// eventfd the connection does not poll), and to `warm_up` while the warm-up thread is inside `Device::new` (so a hung
/// one is noticed without anything waking the loop). A window that has its buffer again leaves the runtime's own
/// timeout, so an idle process sleeps.
fn wait_timeout(
    timeout: Option<Duration>,
    gpu_stalled: bool,
    warm_up: Option<Duration>,
) -> Option<Duration> {
    let poll = gpu_stalled.then_some(GPU_RELEASE_POLL);
    [timeout, poll, warm_up].into_iter().flatten().min()
}

/// Whether a window stalled on a GPU buffer has its release points looked at every [`GPU_RELEASE_POLL`]: only while the
/// compositor can draw it and so release a buffer by itself, with an output on the connection (`outputs`) and the
/// window on one (`on_output`). With no output, or the window on none (hidden, minimized, its display gone), no frame
/// can come: its buffers come back only after something reaches the wire (an output, the window entering one, a
/// configure), which ends the wait itself, so the loop sleeps until then.
fn polls_release(outputs: bool, on_output: bool) -> bool {
    outputs && on_output
}

/// The most drops kept while their data is read, as the transport keeps them.
const MAX_LANDED: usize = 8;

/// A drag from another client, or this one, over a window, and the transport's offer.
struct IncomingDrop {
    token: DropToken,
    offer: DragOffer,
}

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
    key: Option<WindowKey>,
    surface: SurfaceId,
    kind: Kind,
    parent: Option<WindowId>,
    /// The logical size to draw at when the compositor leaves it to the window.
    size: (u32, u32),
    min_size: Option<Size>,
    max_size: Option<Size>,
    /// The pixel size of the frame shown last; a frame of another size changes all of the window.
    shown: Option<(u32, u32)>,
    /// A frame callback is outstanding: the compositor will say when to draw.
    frame_pending: bool,
    /// A frame [`WindowSystem::submit`] took is not back yet, or its commits are not all sent: it commits with a frame
    /// callback, which paces the next, and nothing the next frame sends from the UI thread may go out before them.
    away: bool,
    /// How the frame away was shown, once it is back: the runtime hears it once the frame's commits are sent.
    drawn: Option<Shown>,
    /// A commit without a buffer was asked for while a frame was away, sent once the frame's commits are.
    state_owed: bool,
    /// The last present was not ready; the next frame waits for this.
    stalled: Option<Stall>,
    /// The surface layers whose present found no free buffer: a window stalled on a buffer waits for theirs too.
    busy_layers: Vec<SurfaceId>,
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
    /// The compositor leaves the window's size to it (no configure yet, or the toplevel's last one carried no size):
    /// `size` follows what the runtime asks ([`WindowRequest::SetSize`]). A popup's or role's configure always sizes it.
    free_size: bool,
    menus: Option<ShownMenus>,
    /// An input region to send with the next present; `Some(None)` gives the whole surface back.
    input_region: Option<Option<Vec<Rect>>>,
    /// An opaque region to send with the next present of a frame.
    opaque_region: Option<Vec<Rect>>,
    /// The surface has the keyboard focus, between its `wl_keyboard.enter` and `leave`.
    keyboard: bool,
    /// A toplevel's activated state as its latest configure stated it; `None` before one, and for other surfaces.
    activated: Option<bool>,
    /// The subsurface the window's raised layer and raised materials are drawn in, once it showed one.
    layer: Option<LayerSurface>,
}

/// The subsurface of a window's raised layer ([`Submission::raised`]): at the window's top left corner, as wide as the
/// window, over its content, and taking no input.
struct LayerSurface {
    surface: SurfaceId,
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
    /// The glass drawn in place of the material, if any.
    glass: Option<GlassRegion>,
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

/// Varies the blur of `backdrop` from its top to its bottom by `ramp`, each weight taken from 0 to 1 (one that is not a
/// number as 1); a compositor without blur ramps is sent nothing and blurs evenly.
fn set_ramp(
    backdrop: &Material,
    ramp: BlurRamp,
) -> Result<(), cupertino_wayland_client::app::Error> {
    let weight = |w: f64| if w.is_nan() { 1.0 } else { w.clamp(0.0, 1.0) };
    backdrop.set_blur_ramp(weight(ramp.top), weight(ramp.bottom))
}

/// Gives `backdrop` its look: glass for `glass`, or for a glass material name of the theme, on its own; otherwise the
/// material `name`.
fn set_look(
    backdrop: &Material,
    name: &str,
    glass: Option<GlassRegion>,
) -> Result<(), cupertino_wayland_client::app::Error> {
    let glass =
        glass.or_else(|| GlassStyle::of_material(name).map(|s| GlassRegion::of_style(s, 0, 0.0)));
    let Some(g) = glass else {
        backdrop.set_material(name);
        return Ok(());
    };
    // a compositor without variants draws the style, untinted
    backdrop.set_glass_tint(g.tint.map(f64::from))?;
    match g.variant.filter(|_| backdrop.takes_glass_variants()) {
        Some(variant) => backdrop.set_glass_variant(
            crate::glass_wire::protocol_variant(variant),
            g.container,
            g.spacing,
        ),
        None => backdrop.set_glass(protocol_style(g.style), g.container, g.spacing),
    }
}

/// The protocol's glass style for a region's.
pub(crate) fn protocol_style(style: GlassStyle) -> cupertino_wayland_client::app::GlassStyle {
    match style {
        GlassStyle::Regular => cupertino_wayland_client::app::GlassStyle::Regular,
        GlassStyle::Clear => cupertino_wayland_client::app::GlassStyle::Clear,
        GlassStyle::LargePanel => cupertino_wayland_client::app::GlassStyle::LargePanel,
        GlassStyle::Bar => cupertino_wayland_client::app::GlassStyle::Bar,
        GlassStyle::Dense => cupertino_wayland_client::app::GlassStyle::Dense,
        GlassStyle::Module => cupertino_wayland_client::app::GlassStyle::Module,
        GlassStyle::Card => cupertino_wayland_client::app::GlassStyle::Card,
        GlassStyle::Track => cupertino_wayland_client::app::GlassStyle::Track,
        GlassStyle::Popover => cupertino_wayland_client::app::GlassStyle::Popover,
        GlassStyle::Switcher => cupertino_wayland_client::app::GlassStyle::Switcher,
        GlassStyle::Sidebar => cupertino_wayland_client::app::GlassStyle::Sidebar,
        GlassStyle::SubduedModule => cupertino_wayland_client::app::GlassStyle::SubduedModule,
        GlassStyle::Snippet => cupertino_wayland_client::app::GlassStyle::Snippet,
        GlassStyle::Veiled => cupertino_wayland_client::app::GlassStyle::Veiled,
        GlassStyle::Toolbar => cupertino_wayland_client::app::GlassStyle::Toolbar,
        GlassStyle::Menu => cupertino_wayland_client::app::GlassStyle::Menu,
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
    /// Frames drawn away from the UI thread ([`WindowSystem::submit`]) come back here, read on each dispatch.
    finished: (Sender<FromJob>, Receiver<FromJob>),
    semantics: Option<cupertino_wayland_client::app::Semantics>,
    semantic_surfaces: HashMap<WindowId, cupertino_wayland_client::app::SemanticSurface>,
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
    /// Taken on the next opening attempt, whether it succeeds or not. Only a backend with role extensions needs it.
    opening_key: Option<WindowKey>,
    /// The process's renderer and, while it draws on the GPU, every surface's GPU buffers.
    gpu: GpuPath,
    /// The windows' presentations the compositor animates ([`WindowRequest::Present`]).
    presentations: Presentations,
    /// The windows' surface layers ([`Submission::surface_layers`]).
    surface_layers: crate::surface_layers::SurfaceLayers,
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

/// Switches the frame timeline `CUPERTINO_PROFILE` asks for on, once for the process, and starts the thread that writes
/// it: the first window system of a process does, whether it connected itself or was handed its connection, and a
/// later one finds the subscriber installed and does nothing. Each span carries the name of the thread that ran it, so
/// the trace tells the components of one process apart.
fn start_timeline() {
    if timeline::init() {
        let _ = std::thread::Builder::new()
            .name(timeline::WRITER_THREAD.into())
            .spawn(timeline::write_traces);
    }
}

fn platform_error(e: Error) -> PlatformError {
    match e {
        Error::Disconnected(d) => PlatformError::Disconnected(d.to_string()),
        Error::Unsupported(_) => PlatformError::Unsupported,
        e => PlatformError::Protocol(e.to_string()),
    }
}

impl WaylandWindowSystem {
    /// Connect to the compositor named by the environment, for a process that draws on the CPU
    /// ([`RendererClass::Cpu`]).
    pub fn connect(profile: Profile) -> Result<WaylandWindowSystem, PlatformError> {
        WaylandWindowSystem::connect_as(profile, RendererClass::Cpu)
    }

    /// Connect to the compositor named by the environment, for a process of `class`: the class decides whether its
    /// windows draw on the CPU or the GPU, or the size and repaint rule does, and `CUPERTINO_RENDERER=cpu|gpu`
    /// overrides it.
    pub fn connect_as(
        profile: Profile,
        class: RendererClass,
    ) -> Result<WaylandWindowSystem, PlatformError> {
        Connection::connect()
            .map(|conn| WaylandWindowSystem::new_as(conn, profile, class))
            .map_err(platform_error)
    }

    /// The window system on `conn`, for a process that draws on the CPU ([`RendererClass::Cpu`]).
    pub fn new(conn: Connection, profile: Profile) -> WaylandWindowSystem {
        WaylandWindowSystem::new_as(conn, profile, RendererClass::Cpu)
    }

    /// The window system on `conn`, for a process of `class` ([`WaylandWindowSystem::connect_as`]).
    pub fn new_as(
        mut conn: Connection,
        profile: Profile,
        class: RendererClass,
    ) -> WaylandWindowSystem {
        start_timeline();
        let effects = AppEffects::bind(&mut conn).map(|effects| {
            let observer = effects.appearance(&conn);
            (effects, observer)
        });
        let semantics = cupertino_wayland_client::app::Semantics::bind(&mut conn);
        let presentations = Presentations::bind(&mut conn);
        let gpu = GpuPath::start(&mut conn, class);
        let mut system = WaylandWindowSystem {
            conn,
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
            finished: mpsc::channel(),
            effects,
            appearance: SystemAppearance::default(),
            roles: None,
            role_configures: 0,
            opening_key: None,
            menus: AppMenus::default(),
            gpu,
            presentations,
            surface_layers: crate::surface_layers::SurfaceLayers::default(),
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

    /// The process's GPU path: whether it draws on the GPU, and what each surface's GPU buffers did.
    pub fn gpu(&self) -> &GpuPath {
        &self.gpu
    }

    /// Wait up to `timeout` for the GPU device and programs made on the warm-up thread; whether the process draws on
    /// the GPU now. The window system takes them by itself as they come (its next dispatch); this is for tests and
    /// tools that need the switch at a known moment.
    pub fn wait_for_gpu(&mut self, timeout: Duration) -> bool {
        self.gpu.poll(&mut self.conn, Some(timeout));
        let events = self.gpu.take_events();
        self.renderer_changed(&events);
        self.queued.extend(events);
        self.gpu.on()
    }

    /// Test hook: the GPU device's next status check reads `loss` instead of the driver's own, as if it had been reset
    /// or purged; the next frame finds it.
    pub fn inject_gpu_loss(&self, loss: cupertino_gpu::Loss) {
        self.gpu.inject_loss(loss);
    }

    /// After a change of renderer every window's next frame is whole.
    fn renderer_changed(&mut self, events: &[PlatformEvent]) {
        if events
            .iter()
            .any(|e| matches!(e, PlatformEvent::RendererChanged(_)))
        {
            for w in &mut self.windows {
                w.shown = None;
            }
        }
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

    /// What makes the windows' renderers on the process's GPU once it draws there: a renderer factory holds it
    /// (`cupertino-kit`'s does).
    pub fn gpu_renderers(&self) -> GpuRenderers {
        self.gpu.renderers()
    }

    /// The renderer this window system draws with now, and why: the CPU until the rule decided otherwise.
    pub fn renderer(&self) -> crate::renderer::Choice {
        self.gpu.choice()
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
        key: Option<WindowKey>,
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
            key,
            surface,
            kind,
            parent,
            size,
            min_size: None,
            max_size: None,
            shown: None,
            frame_pending: false,
            away: false,
            drawn: None,
            state_owed: false,
            stalled: None,
            busy_layers: Vec::new(),
            frame_wanted: false,
            in_flight: VecDeque::new(),
            backdrop: None,
            materials: Vec::new(),
            app_id: String::new(),
            configured: false,
            free_size: true,
            menus: None,
            input_region: None,
            opaque_region: None,
            keyboard: false,
            activated: None,
            layer: None,
        });
        id
    }

    fn opening_role(&mut self) -> Option<WindowKey> {
        let key = self.opening_key.take();
        if let Some(roles) = &mut self.roles {
            roles.opening(key.as_ref());
        }
        key
    }

    /// For a frame, the role extension sends its state after the buffer preflight, before the worker may commit.
    fn role_commit(&mut self, window: WindowId, new_frame: bool) {
        let Some(roles) = &mut self.roles else { return };
        let Some(w) = self.windows.iter().find(|w| w.id == window) else {
            return;
        };
        if let Some(key) = &w.key {
            let layers = self.surface_layers.shown_mut(window);
            roles.before_commit(
                &mut self.conn,
                key,
                w.surface,
                &mut layers.iter().map(|s| (s.key, s.surface)),
                new_frame,
            );
        }
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
        let protocol =
            |e: cupertino_wayland_client::app::Error| PlatformError::Protocol(e.to_string());
        let surface = self.window(window)?.surface;
        let shown = match backdrop {
            None => None,
            Some(Backdrop {
                material,
                radius,
                alpha,
                glass,
            }) => {
                // a fade changes only the alpha of the material already shown
                if let Some(shown) = self
                    .window_mut(window)?
                    .backdrop
                    .as_mut()
                    .filter(|b| b.name == material && b.glass == glass)
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
                set_look(&wire, &material, glass).map_err(protocol)?;
                wire.set_alpha(alpha).map_err(protocol)?;
                Some(ShownBackdrop {
                    material: wire,
                    name: material,
                    glass,
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
        let protocol =
            |e: cupertino_wayland_client::app::Error| PlatformError::Protocol(e.to_string());
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
                    if old.region.blur_ramp != region.blur_ramp {
                        set_ramp(&old.material, region.blur_ramp).map_err(protocol)?;
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
                if region.blur_ramp != BlurRamp::EVEN {
                    set_ramp(&material, region.blur_ramp).map_err(protocol)?;
                }
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

    /// Hand `changes` to the presentation of `window`, a surface with a role from [`SurfaceRoles`] or, where the
    /// compositor presents popups, a popup; a window that showed a frame commits them without a buffer
    /// ([`Self::commit_state`]), one that did not with its first frame.
    fn present_changes(
        &mut self,
        window: WindowId,
        changes: &[cupertino_ui_core::platform::PresentChange],
    ) -> Result<(), PlatformError> {
        let w = self.window(window)?;
        let popup = matches!(w.kind, Kind::Popup(_)) && self.presentations.popups();
        if !matches!(w.kind, Kind::External) && !popup {
            return Err(PlatformError::Unsupported);
        }
        let (surface, shown) = (w.surface, w.shown.is_some());
        self.presentations
            .change(&self.conn, window, surface, changes)?;
        if shown {
            // an input region asked for meanwhile goes with the commit: a closing popup takes no input as it fades
            if let Some(region) = self.window_mut(window)?.input_region.take() {
                self.conn
                    .set_input_region(surface, region.as_deref())
                    .map_err(platform_error)?;
            }
            self.commit_state(window)?;
        }
        Ok(())
    }

    /// Commits `window` without a buffer, so what was sent for it applies at once, or, while a frame of it is away,
    /// once that frame's commits are sent ([`Self::sent`]): sent before them, it would apply what the UI thread sent
    /// with that frame (its regions, its layers' values, its raised layer hidden) ahead of the frame.
    fn commit_state(&mut self, window: WindowId) -> Result<(), PlatformError> {
        let w = self.window_mut(window)?;
        if w.away {
            w.state_owed = true;
            return Ok(());
        }
        let surface = w.surface;
        self.role_commit(window, false);
        self.conn.commit_state(surface).map_err(platform_error)
    }

    /// Whether what `window` shows is its layout: no animation of its presentation or its surface layers runs.
    pub fn presentation_settled(&self, window: WindowId) -> bool {
        self.presentations.settled(window) && self.surface_layers.settled(window)
    }

    /// Whether windows may have surface layers: the layers global at the version that blurs content.
    fn surface_layers_drawn(&self) -> bool {
        self.presentations
            .layers()
            .is_some_and(cupertino_wayland_client::app::Layers::blurs_content)
    }

    /// Sends `changes` to `window`'s surface layer `key` and commits the window without a buffer when it shows a frame
    /// ([`Self::commit_state`]), so they apply; a layer not made yet takes them with its first values.
    fn change_surface_layer(
        &mut self,
        window: WindowId,
        key: u64,
        changes: Vec<cupertino_ui_core::platform::LayerChange>,
    ) -> Result<(), PlatformError> {
        if !self.surface_layers_drawn() {
            return Err(PlatformError::Unsupported);
        }
        let shown = self.window(window)?.shown.is_some();
        if self.surface_layers.change(window, key, changes)? && shown {
            self.commit_state(window)?;
        }
        Ok(())
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
            shown: None,
        });
        Ok(surface)
    }

    /// Hides `window`'s raised layer with the window's next commit.
    fn hide_layer(&mut self, window: WindowId) -> Result<(), PlatformError> {
        if let Some(shown) = self.window_mut(window)?.layer.as_mut() {
            if shown.shown.take().is_some() {
                let surface = shown.surface;
                self.conn
                    .unmap_subsurface(surface)
                    .map_err(platform_error)?;
            }
        }
        Ok(())
    }

    /// Makes `window`'s surface layers `wanted` (each key with its rect and look), in their order, their values sent
    /// for the window's next commit; the surfaces of layers no longer wanted go.
    fn sync_surface_layers(
        &mut self,
        window: WindowId,
        wanted: &[(u64, Rect, SurfaceLayerLook)],
    ) -> Result<(), PlatformError> {
        if !self.surface_layers_drawn() {
            return if wanted.is_empty() {
                Ok(())
            } else {
                Err(PlatformError::Unsupported)
            };
        }
        let parent = self.window(window)?.surface;
        let global = self
            .presentations
            .layers()
            .ok_or(PlatformError::Unsupported)?;
        let gone = self
            .surface_layers
            .sync(&mut self.conn, global, window, parent, wanted)?;
        self.window_mut(window)?
            .busy_layers
            .retain(|s| !gone.contains(s));
        for s in gone {
            self.gpu.forget(&mut self.conn, s);
            let _ = self.conn.destroy(s);
        }
        Ok(())
    }

    /// The pixel size of the frame `window`'s surface `of` shows.
    fn shown_size(&mut self, window: WindowId, of: Of) -> Option<(u32, u32)> {
        match of {
            Of::Window => self.window(window).ok()?.shown,
            Of::Raised => self.window(window).ok()?.layer.as_ref()?.shown,
            Of::Layer(key) => {
                self.surface_layers
                    .shown_mut(window)
                    .iter()
                    .find(|s| s.key == key)?
                    .shown
            }
        }
    }

    /// A buffer of `surface` for a frame of `pixels` drawn for `preferred` (points), `resized` when the compositor takes
    /// all of it as changed: a free GPU buffer where the surface draws on the GPU, else a shared-memory buffer detached
    /// with its commit, in the renderer's byte order where the compositor takes it. Without one, what the window waits
    /// for: a free buffer, its configure, or nothing for a frame of another size than the surface asks for.
    fn prepare_target(
        &mut self,
        surface: SurfaceId,
        preferred: (u32, u32),
        pixels: (u32, u32),
        resized: bool,
    ) -> Result<Result<Target, Option<Stall>>, PlatformError> {
        if self.gpu.draws(surface) {
            let prepared = self
                .gpu
                .prepare(&mut self.conn, surface, preferred, pixels, resized)
                .map_err(platform_error)?;
            match prepared {
                Prepared::Gpu(draw) => return Ok(Ok(Target::Gpu(*draw))),
                Prepared::Busy => return Ok(Err(Some(Stall::Buffer))),
                Prepared::Resized => return Ok(Err(None)),
                // the surface fell back to shared memory, or the device was lost
                Prepared::Shm => {}
            }
        }
        let format = if self.conn.offers(BufferFormat::Abgr8888) {
            BufferFormat::Abgr8888
        } else {
            BufferFormat::Argb8888
        };
        let leased = tracing::trace_span!("wl.lease")
            .in_scope(|| self.conn.lease(surface, preferred, format));
        match leased {
            Ok(lease) => {
                let layout = lease.layout();
                if (layout.width(), layout.height()) != pixels {
                    return Ok(Err(None));
                }
                let (buffer, commit) = lease.detach().map_err(platform_error)?;
                Ok(Ok(Target::Shm(buffer, commit)))
            }
            Err(Error::NoFreeBuffer) => Ok(Err(Some(Stall::Buffer))),
            Err(Error::NotConfigured) => Ok(Err(Some(Stall::Configure))),
            Err(e) => Err(platform_error(e)),
        }
    }

    /// A buffer [`Self::prepare_target`] gave, given back undrawn: a GPU surface's buffers go, allocated again for its
    /// next frame; a shared-memory buffer's commit goes unsent.
    fn release_target(&mut self, surface: SurfaceId, target: Target) {
        match target {
            Target::Gpu(draw) => {
                drop(draw);
                let _ = self
                    .gpu
                    .finish(&mut self.conn, surface, Err(PresentError::Resized));
            }
            Target::Shm(buffer, commit) => {
                drop(commit);
                self.conn.buffer_unused(buffer);
            }
        }
    }

    /// Whether `surface` has a buffer to present a frame into now: one of its GPU buffers where it draws on the GPU,
    /// else one of its shared-memory buffers.
    fn surface_can_present(&mut self, surface: SurfaceId) -> bool {
        match self.gpu.buffer_free(&self.conn, surface) {
            Some(free) => free,
            None => matches!(self.conn.buffer_free(surface), Ok(true)),
        }
    }
}

mod present;
mod trait_impl;
use trait_impl::set_enabled;
