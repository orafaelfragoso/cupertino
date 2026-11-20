//! The runtime: one UI thread that owns the model, the node tree and every window. A step takes the window system's
//! events, routes them, runs ready tasks, feeds messages to `update`, rebuilds the view when the model changed,
//! and then, per window, lays out, rebuilds semantics and focus, and paints and presents when the compositor is
//! ready for a frame. Time comes only from the injected clock and the window system's events.

use crate::command::CommandRegistry;
use crate::cx::{Effects, Status, WindowGesture};
use crate::damage::DamageTracker;
use crate::focus::{Focus, FocusChange};
use crate::frame_timing::{handed_over, lap, mark, Phase};
use crate::layout::{
    layout, layout_cached, Constraints, LayoutCache, LayoutDirection, LayoutOptions, LayoutResult,
    LayoutStats,
};
use crate::layout_view::{placements, placements_kept, KeptPlacements, TreeView};
use crate::paint::{raised_nodes, same_environment, PaintInput, Painter};
use crate::platform::{
    ActivationRequest, Backdrop, ClipboardData, ClipboardKind, ClipboardToken, ConfigureId,
    ConstraintAdjust, DragImage, DragProgress, DragRequest, DragToken, DropAction, DropToken,
    GlassTone, KeyEvent, Keysym, LayerList, Lent, MaterialRegion, MenuDescription, Mods,
    MouseButton, PlatformError, PlatformEvent, PointerEvent, PopupEdge, PopupSpec, PresentOutcome,
    RaisedChange, RaisedLayer, SharedRenderer, Shown, Submission, Submitted, SurfaceLayerLook,
    SystemAppearance, Timestamp, TouchEvent, WindowEvent, WindowId, WindowRequest, WindowSpec,
    WindowSystem,
};
use crate::pointer::Pointer;
use crate::route::{hit_test, Geometry, Route};
use crate::scroll::{geometry_events, reveal_events};
use crate::semantics::{
    build_kept, composing, diff, dismissible, keeps_editing, protected, text_input, KeptSemantics,
};
use crate::task::Executor;
use crate::tree::Tree;
use crate::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, Application, Clock, Command,
    Drag, DragHandler, DragPreview, Element, Environment, Event, FocusTarget, FrameSnapshot, Icons,
    Key, Menus, NodeId, NodeSnapshot, OwnedWindow, Pending, PopupAnchor, Propagation, StepStatus,
    TokenHandler, UiError, Window, WindowBackground, WindowKey,
};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_menu_model::ActionName;
use cupertino_theme::ui::{Color, Surface, ThemeTokens};
use cupertino_ui_render::{
    damage_rects, CpuFrame, DamageRegion, DisplayList, PaintResources, PixelRect, RenderError,
    RenderRequest, UiRenderer,
};
use cupertino_ui_text::TextSystem;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

const TAB: Keysym = Keysym(0xff09);
const ESCAPE: Keysym = Keysym(0xff1b);
/// How many frames in a row a window holds back while its scroll containers tell the model a new geometry.
const HELD_PAINTS: u8 = 4;
const LEFT_TAB: Keysym = Keysym(0xfe20);

pub type RendererFactory = Box<dyn FnMut() -> Box<dyn UiRenderer<Frame = CpuFrame> + Send>>;
pub type WindowErrorHandler = Box<dyn FnMut(WindowId, &UiError)>;
/// Makes the theme for the session's appearance and display options.
pub type ThemeForAppearance = Box<dyn FnMut(&SystemAppearance) -> Arc<ThemeTokens>>;

/// Asked each time the window system wakes the loop: the theme to draw with when what a person chose changed since it
/// was last asked, with [`crate::set_personal`] already told the other choices.
pub type PersonalRefresh = Box<dyn FnMut() -> Option<Arc<ThemeTokens>>>;

/// Called on each turn of the loop, after the window system was read: the application's runner keeps up the work it holds
/// outside the runtime (the processes it started, reaped as they end).
pub type EachTurn = Box<dyn FnMut()>;

/// How a runtime is set up besides its window system.
pub struct RuntimeOptions {
    pub theme: Arc<ThemeTokens>,
    pub text: Arc<TextSystem>,
    /// The system's symbols ([`PlaceholderIcons`](crate::PlaceholderIcons) without asset packs).
    pub icons: Arc<dyn Icons>,
    pub direction: LayoutDirection,
    /// The most nodes the tree holds; more is a [`UiError::NodeLimit`].
    pub node_limit: usize,
    /// Makes the renderer of each new window.
    pub renderers: RendererFactory,
    pub accessibility: Option<Box<dyn AccessibilityAdapter>>,
    pub clock: Arc<dyn Clock>,
    /// Hears about a window that could not be laid out, rendered or presented, and about a popup the window system would
    /// not open (against its parent). The window keeps running and draws whole again after its next configure; the
    /// popup stays closed; the application does not end.
    pub on_window_error: Option<WindowErrorHandler>,
}

/// A held key, by its physical code, which its release repeats even when a modifier changed the symbol in between; by
/// its symbol when the window system gives no code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeldKey {
    Code(u32),
    Symbol(Keysym),
}

impl HeldKey {
    fn of(key: &KeyEvent) -> HeldKey {
        match key.code {
            0 => HeldKey::Symbol(key.keysym),
            code => HeldKey::Code(code),
        }
    }
}

impl<M> WindowState<M> {
    /// Whether the window may draw a frame now: configured, the compositor ready for one, and no frame of it away
    /// ([`WindowSystem::submit`]). A window that may not waits, and the loop with it, for what lets it.
    fn may_draw(&self) -> bool {
        self.configured && self.frame_ready && self.in_flight.is_none()
    }

    /// Lets the renderers of the window and its layers go of the layers they keep, as a window nobody sees does.
    fn drop_layers(&mut self) {
        held(&self.renderer).drop_layers();
        if let Some(layer) = &self.layer.renderer {
            held(layer).drop_layers();
        }
        self.surface_layers.drop_kept();
    }

    /// Something in the window changed: lay it out and paint it again.
    fn invalidate(&mut self) {
        self.dirty = true;
        self.prepared = false;
        self.laid_out = None;
    }

    /// The next frame repaints the whole window, its raised layer too.
    fn invalidate_damage(&mut self) {
        self.damage.invalidate();
        self.layer.damage.invalidate();
        self.surface_layers.invalidate();
    }

    /// The window just became active or inactive: when a window already drawn became active, its colours cross over
    /// [`ACTIVATION_NS`] (at once with Reduce Motion). A window that opens active shows the active colours from its
    /// first frame.
    fn begin_activation(&mut self, now: Timestamp) {
        self.activated_at =
            (self.active && self.committed && !self.env.theme.preferences.reduce_motion)
                .then_some(now);
    }

    /// How far the colours of the window have crossed to the active ones at `now`, 0 to 1; ends the crossing when done.
    fn activation_progress(&mut self, now: Timestamp) -> f32 {
        let Some(start) = self.activated_at else {
            return 1.0;
        };
        let elapsed = now.0.saturating_sub(start.0);
        if elapsed >= ACTIVATION_NS {
            self.activated_at = None;
            return 1.0;
        }
        // eased out: it moves fastest at the start
        let t = elapsed as f32 / ACTIVATION_NS as f32;
        1.0 - (1.0 - t) * (1.0 - t)
    }

    /// Something in the window changed how it paints but no node's size or place: paint it again, and build its
    /// semantics and geometry again from the layout it has.
    fn repaint(&mut self) {
        self.dirty = true;
        self.prepared = false;
    }
}

/// A window's raised layer ([`RaisedLayer`]): what it last showed and how to draw the next.
#[derive(Default)]
struct LayerState {
    damage: DamageTracker,
    /// Made on the first frame that shows a layer.
    renderer: Option<SharedRenderer>,
    list: Arc<DisplayList>,
    /// The height in whole points of the layer the window system shows, while it shows one.
    shown: Option<u32>,
}

struct WindowState<M> {
    key: WindowKey,
    id: WindowId,
    spec: WindowSpec,
    shortcuts: CommandRegistry<M>,
    on_close: Option<M>,
    background: WindowBackground,
    /// The material the window system shows under the window, as last asked for.
    backdrop: Option<Backdrop>,
    /// The materials the window system shows under nodes, as last asked for.
    materials: Vec<MaterialRegion>,
    /// The window system refused materials under nodes: the nodes paint their fills.
    materials_refused: bool,
    /// The size and colour the window system shows the window in, with no frame, while its background is
    /// [`WindowBackground::Solid`]; `None` once a frame is shown, or before anything is.
    solid_shown: Option<(Size, Color)>,
    /// The window system cannot show a colour without a frame: a solid window is drawn.
    solid_refused: bool,
    /// The menus the window system was last asked to publish.
    menus: Option<MenuDescription>,
    on_menu: Option<Box<dyn Fn(ActionName) -> M>>,
    root: Option<NodeId>,
    env: Environment,
    size: Size,
    configured: bool,
    /// The compositor leaves the window's size to it: no configure yet, or the last one carried no size. The window
    /// then takes its spec's size, and the window system is told when that changes.
    free_size: bool,
    ack: Option<ConfigureId>,
    active: bool,
    /// A popup opened from this window has the keyboard, or had it as it closed and the window has not heard since
    /// where the keyboard went: the window still draws as active, as a menu takes the keyboard while the window it
    /// opened from stays the one in use. A popup that loses the keyboard while still open clears it.
    keyboard_in_popup: bool,
    /// Whether the window's pixels were last painted active (`active || keyboard_in_popup`); `None` before its first
    /// paint. The keyboard moving between the window and its popups paints the window again only when this differs
    /// as it is drawn: a menu or a submenu that takes the keyboard while the window stays active repaints nothing.
    painted_active: Option<bool>,
    /// The last configure said the window is maximized; a zoom asks for the other state.
    maximized: bool,
    /// The last configure said the window is fullscreen; a drag region neither moves nor zooms it.
    fullscreen: bool,
    /// The compositor is ready for a frame.
    frame_ready: bool,
    /// A frame of the window has been committed; the compositor has not necessarily shown it yet.
    committed: bool,
    /// A frame was drawn that the window system was not ready to show; it keeps the frame's damage, and the next
    /// present shows it even when nothing changed since.
    owed: bool,
    /// The compositor answered the frame callback of a committed frame, so the window has been shown
    /// ([`Command::OnPresented`]).
    presented: bool,
    /// Something changed since the last presented frame.
    dirty: bool,
    /// Layout and semantics are up to date with the tree.
    prepared: bool,
    /// The last layout, while it is up to date with the tree and the window's size.
    laid_out: Option<LayoutResult<NodeId>>,
    focus: Focus,
    pointer: Pointer,
    touches: HashMap<i32, Vec<NodeId>>,
    /// Each key held down and the node that took its press; it gets [`Event::Cancel`] if the window deactivates first.
    held: Vec<(HeldKey, NodeId)>,
    /// The Escape a dismissible node took, whose release then goes to no one.
    dismissed: Option<HeldKey>,
    animate: BTreeSet<NodeId>,
    /// The nodes of [`WindowState::animate`] that asked from their paint: what they draw moves by itself, so they
    /// paint again at the frame whether or not they handle [`Event::AnimationFrame`].
    paint_animate: BTreeSet<NodeId>,
    /// The nodes that asked for the window's idle time ([`Widget::idle`]), each to be called once.
    idle: BTreeSet<NodeId>,
    /// When the window's frames came, to tell when the next is due.
    frame_clock: idle::FrameClock,
    /// When each node asked to be woken, one time per node; ordered so that nodes due together wake in a fixed order.
    wakes: BTreeMap<NodeId, Timestamp>,
    text_input: Option<NodeId>,
    geometry: Geometry,
    accessibility: BTreeMap<NodeId, AccessibilityNode>,
    /// The node of `accessibility` assistive technology is told holds the focus.
    accessibility_focus: Option<NodeId>,
    semantic_revision: u64,
    semantic_snapshot: Option<cupertino_app_protocols::semantics::Snapshot>,
    /// When the tree was last handed to the window system as a new snapshot.
    semantic_sent: Option<Timestamp>,
    /// When a change of the tree held back by the publishing interval may go out.
    semantic_due: Option<Timestamp>,
    /// The adapter holds this window's whole tree, so a diff is enough.
    accessible: bool,
    focusable: Vec<NodeId>,
    /// Roving groups and their items, in reading order, from the last semantics pass.
    roving_groups: Vec<NodeId>,
    roving_items: Vec<NodeId>,
    /// The item a roving group asked to focus, moved to at the next frame if it is a Tab stop by then.
    focus_request: Option<FocusRequest>,
    /// The node keyboard or programmatic focus moved to, for its scroll containers to show at the next layout.
    reveal: Option<(NodeId, Option<Rect>)>,
    /// Each scroll container's viewport and content as it last heard them.
    scroll_heard: HashMap<NodeId, (Size, Size)>,
    /// Frames held back in a row, so the model could take a scroll geometry first.
    held_paints: u8,
    layout: LayoutStats,
    /// What earlier layouts measured, so an unchanged subtree is not measured again; good for the environment it was
    /// measured in only.
    layout_cache: LayoutCache<NodeId>,
    measured_in: Option<Environment>,
    /// The kept subtrees' placements, good for the environment the layout cache is.
    placements: KeptPlacements,
    /// The kept subtrees' semantics, good for the environment the layout cache is.
    semantics: KeptSemantics,
    painter: Painter,
    damage: DamageTracker,
    /// The raised nodes ([`Widget::raised`]) of the last layout with their rects, in window coordinates.
    raised: Vec<(NodeId, Rect)>,
    /// Those of them that cover what lies under them ([`Widget::obscures`]).
    obscuring: Vec<(NodeId, Rect)>,
    /// The raised layer the window system shows over the window's content.
    layer: LayerState,
    /// The marked subtrees the window system shows in surfaces of their own ([`Widget::surface_layer`]).
    surface_layers: surface_layers::SurfaceLayers,
    display_list: Arc<DisplayList>,
    /// The images the display list draws.
    resources: Arc<PaintResources>,
    renderer: SharedRenderer,
    /// The frame the window system took ([`WindowSystem::submit`]) and has not said yet how it was shown.
    in_flight: Option<InFlight>,
    /// The renderers of the window and its layers let go of the layers they keep once the frame in flight is shown:
    /// the UI thread never waits for a renderer while a frame of it is away, since the renderer may wait for the
    /// render thread that draws the frame.
    layers_owed: bool,
    /// The error of the last frame, until one renders and presents; animations wait for that.
    failed: Option<UiError>,
    popup: Option<PopupState<M>>,
    /// When the pointer press that opened this popup went down, if a button was held as it opened.
    opening_press: Option<Timestamp>,
    /// The touch that opened this popup, if one was held as it opened: its id and when it went down.
    opening_touch: Option<(i32, Timestamp)>,
    /// Where the compositor last placed each open popup of this window, in its coordinates; a popup asked to move is
    /// not here until it is placed anew.
    popups: Vec<(WindowKey, Rect)>,
    /// A drag from any application over the window.
    drop: Option<drops::DropState>,
    /// The popup closed and is fading out; it takes no input and the view no longer holds it.
    fade: Option<fade::Fade>,
    /// The point of this popover it grows out of as it opens and shrinks into as it closes
    /// ([`Widget::grows_from_anchor`]), in its coordinates; set as its first frame is drawn.
    grows_from: Option<Point>,
    /// What [`Self::grows_from`] was last found from for a popover that grows out of a node of its parent: the parent's
    /// layout count, where the popover was placed and where it was asked to be. It is found again only when one changes.
    grow_followed: Option<(u64, Rect, PopupSpec)>,
    /// How many times the window was laid out: its nodes' places change only then.
    layouts: u64,
    /// The popover is growing to its size.
    grow: Option<morph::Grow>,
    /// When the window last became active, while its colours still cross from the inactive ones.
    activated_at: Option<Timestamp>,
    /// The opaque region the window system was last asked to show the window with ([`WindowRequest::SetOpaqueRegion`]).
    opaque: Vec<Rect>,
    /// The clipboard holds text, as the window's controls are told.
    clipboard_text: bool,
    /// A control of the window has a popup of its own open.
    owned_popup: bool,
}

/// A frame the window system took ([`WindowSystem::submit`]): what the window shows once it says the frame was shown.
struct InFlight {
    /// The configure the frame answers.
    ack: Option<ConfigureId>,
    /// The height of the raised layer shown with it, `None` for none, where the frame changes the layer.
    raised: Option<Option<u32>>,
    /// The surface layers shown with it, each key with its rect and look, where the frame changes them.
    surface_layers: Option<Vec<(u64, Rect, SurfaceLayerLook)>>,
}

/// How long the colours of a window that becomes active take to cross from the inactive ones, in nanoseconds.
const ACTIVATION_NS: u64 = 120_000_000;

/// What tells of a popup the compositor dismissed: the application, by its message, or the runtime itself, for a popup
/// a control opened for itself.
#[derive(Clone)]
pub(super) enum Dismiss<M> {
    Message(M),
    Owned,
}

/// A popup to open or keep: where, and what its dismissal tells.
struct Declared<M> {
    parent: WindowKey,
    anchor: PopupAnchor,
    grab: bool,
    on_dismiss: Dismiss<M>,
}

/// The popup a control opened for itself ([`EventCx::open_popup`](crate::EventCx::open_popup)).
struct Owned<M> {
    /// The window the control is in, and the control.
    parent: WindowKey,
    node: NodeId,
    build: crate::OwnedWindows<M>,
    /// The keys of its windows at the last view.
    keys: Vec<WindowKey>,
}

/// The kinds of text the clipboard is read as, the first it offers.
const TEXT_MIMES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];

/// `bytes` as text: invalid sequences replaced, a character cut at the end dropped.
fn clipboard_text(bytes: &[u8]) -> String {
    let whole = match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => &bytes[..e.valid_up_to()],
        _ => bytes,
    };
    String::from_utf8_lossy(whole).into_owned()
}

struct PopupState<M> {
    parent: WindowKey,
    anchor: PopupAnchor,
    /// What the content measured at the last view.
    size: Size,
    on_dismiss: Dismiss<M>,
    /// Where the popup was last placed, so it moves only when its anchor or size changes.
    placed: PopupSpec,
    /// Wanted a grab and opened without one ([`WindowSystem::can_grab`]): the runtime closes it as the grab would have.
    ungrabbed: bool,
}

/// The touch held down first ([`Runtime`]'s `touch`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeldTouch {
    /// The window it went down in, which hears all of it.
    window: WindowId,
    id: i32,
    /// When it went down, on the runtime's clock.
    start: Timestamp,
    /// Where it is, in that window's coordinates.
    at: Point,
}

/// Runs an application on a window system.
pub struct Runtime<A: Application, W: WindowSystem> {
    model: A::Model,
    system: W,
    options: RuntimeOptions,
    tree: Tree<A::Message>,
    windows: Vec<WindowState<A::Message>>,
    messages: VecDeque<A::Message>,
    executor: Executor<A::Message>,
    events: Vec<PlatformEvent>,
    view_dirty: bool,
    /// Parents whose popups wait for their first layout to find the anchor.
    awaiting_layout: Vec<WindowKey>,
    /// Parents that got an anchor node in the same view as the popup that stands on it, with whether the view has been
    /// asked again since: the anchor has no place until the parent lays out, so the popup is declared once more then.
    awaiting_anchor: Vec<(WindowKey, bool)>,
    /// Whether the last press was a key or an action assistive technology or desktop control asked for: a popup opened
    /// by it shows its focus.
    keyboard: bool,
    /// When the pointer button held down went down, on the runtime's clock, until it comes up: a popup opened meanwhile
    /// was opened by that press.
    press: Option<Timestamp>,
    /// The touch held down first, until it lifts or is cancelled: a popup opened meanwhile was opened by it. Another
    /// finger put down meanwhile opens nothing.
    touch: Option<HeldTouch>,
    /// A popup opened while that touch was held, since the view was last built.
    touch_opened: bool,
    exit: bool,
    theme_for: Option<ThemeForAppearance>,
    /// The theme in the light and the dark appearance with the session's other options, for subtrees pinned to one
    /// ([`PaintCx::pin_appearance`](crate::PaintCx::pin_appearance)), and the theme they were made from.
    pinned: [Option<Arc<ThemeTokens>>; 2],
    pinned_from: Option<Arc<ThemeTokens>>,
    /// Some window pinned a subtree to an appearance: the pinned themes follow the session from then on.
    pins_seen: bool,
    personal_refresh: Option<PersonalRefresh>,
    each_turn: Option<EachTurn>,
    /// Follows each scroll event to the screen, when asked ([`Runtime::with_scroll_timing`]).
    scroll_timing: Option<crate::scroll_timing::ScrollTimings>,
    /// Times every frame phase by phase, when asked ([`Runtime::with_frame_timing`]).
    frame_timing: Option<crate::frame_timing::FrameTimings>,
    /// What each activation token asked for and not yet given turns into.
    tokens: HashMap<ActivationRequest, TokenHandler<A::Message>>,
    /// What each step of a running drag turns into.
    drags: HashMap<DragToken, DragHandler<A::Message>>,
    /// The drops whose data is being read, by read.
    landed: HashMap<ClipboardToken, drops::Landed>,
    /// Drops handed to their targets, finished once the turn has handled what they became: with the action agreed, or
    /// what the application said it did.
    finishing: Vec<(DropToken, Option<DropAction>)>,
    /// Drops the application keeps open until it finishes them ([`Command::HoldDrop`]).
    held: Vec<DropToken>,
    /// The messages to send once a window has presented its first frame ([`Command::OnPresented`]).
    on_presented: Vec<(WindowKey, A::Message)>,
    /// Requests for windows the view that follows opens ([`Command::Window`]), sent as each is made.
    to_open: Vec<(WindowKey, WindowRequest)>,
    /// The messages of [`Command::After`], by when they are due and then in the order they were asked for.
    timers: BTreeMap<(Timestamp, u64), A::Message>,
    /// The order of the next [`Command::After`].
    next_timer: u64,
    semantic_finishing: Vec<(
        WindowId,
        cupertino_app_protocols::semantics::ActionCall,
        cupertino_app_protocols::semantics::Outcome,
    )>,
    /// Popups the window system refused, kept closed until the view stops declaring them.
    refused_popups: Vec<WindowKey>,
    /// The kind of text the clipboard offers, when it holds text.
    clipboard_text: Option<String>,
    /// The clipboard reads under way for a paste, each with the window and the node that asked.
    pastes: HashMap<ClipboardToken, (WindowKey, NodeId)>,
    /// The popup a control opened for itself, while it is open.
    owned: Option<Owned<A::Message>>,
    /// The window whose control's popup the compositor dismissed among the events of this turn: it hears where that
    /// left the keyboard once they are all handled ([`Runtime::keyboard_after_dismissal`]).
    owned_dismissed: Option<WindowKey>,
}

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    pub fn new(system: W, options: RuntimeOptions) -> Self {
        Self::start(system, options, A::init())
    }

    /// [`Runtime::new`] from `model` and the `commands` to run first, instead of [`Application::init`]'s: for an
    /// application that starts from what its caller read, such as its options and its channels to a service.
    pub fn start(
        system: W,
        options: RuntimeOptions,
        (model, commands): (A::Model, Vec<Command<A::Message>>),
    ) -> Self {
        let executor = Executor::new(system.waker());
        // a runtime starts without the session's word, whatever one before it on this thread heard
        crate::scroll::set_session_scroll_bars(None);
        let mut runtime = Self {
            model,
            tree: Tree::new(options.node_limit),
            system,
            options,
            windows: Vec::new(),
            messages: VecDeque::new(),
            executor,
            events: Vec::new(),
            view_dirty: true,
            awaiting_layout: Vec::new(),
            awaiting_anchor: Vec::new(),
            keyboard: false,
            press: None,
            touch: None,
            touch_opened: false,
            exit: false,
            theme_for: None,
            pinned: [None, None],
            pinned_from: None,
            pins_seen: false,
            personal_refresh: None,
            each_turn: None,
            scroll_timing: None,
            frame_timing: None,
            tokens: HashMap::new(),
            drags: HashMap::new(),
            landed: HashMap::new(),
            finishing: Vec::new(),
            held: Vec::new(),
            on_presented: Vec::new(),
            to_open: Vec::new(),
            timers: BTreeMap::new(),
            next_timer: 0,
            semantic_finishing: Vec::new(),
            refused_popups: Vec::new(),
            clipboard_text: None,
            pastes: HashMap::new(),
            owned_dismissed: None,
            owned: None,
        };
        for command in commands {
            runtime.command(command);
        }
        runtime
    }

    /// Follow every pointer scroll event to the screen and give `sink` its timing once the compositor showed the frame
    /// that drew it, or dropped it ([`ScrollTiming`](crate::ScrollTiming)). Without it the runtime keeps no timing.
    pub fn with_scroll_timing(mut self, sink: crate::ScrollTimingSink) -> Self {
        self.scroll_timing = Some(crate::scroll_timing::ScrollTimings::new(sink));
        self
    }

    /// Time every turn that presents a frame, phase by phase, and give `sink` the timings
    /// ([`FrameTiming`](crate::FrameTiming)); the window system times its copies into buffers from then on. Without it
    /// the runtime reads no clock for it.
    pub fn with_frame_timing(mut self, sink: crate::FrameTimingSink) -> Self {
        self.system.time_copies(true);
        self.frame_timing = Some(crate::frame_timing::FrameTimings::new(sink));
        self
    }

    /// Whether the runtime times its frames ([`Runtime::with_frame_timing`]).
    pub fn times_frames(&self) -> bool {
        self.frame_timing.is_some()
    }

    /// Follow the session's appearance: `theme_for` makes the theme each time the window system reports it.
    pub fn with_theme_for(mut self, theme_for: ThemeForAppearance) -> Self {
        self.set_theme_for(theme_for);
        self
    }

    /// Follow what a person chose: `refresh` is asked each time the loop wakes.
    pub fn with_personal_refresh(mut self, refresh: PersonalRefresh) -> Self {
        self.personal_refresh = Some(refresh);
        self
    }

    /// Call `each_turn` on each turn of the loop, after the window system was read.
    pub fn with_each_turn(mut self, each_turn: EachTurn) -> Self {
        self.set_each_turn(each_turn);
        self
    }

    /// [`Runtime::with_each_turn`] on a runtime already made.
    pub fn set_each_turn(&mut self, each_turn: EachTurn) {
        self.each_turn = Some(each_turn);
    }

    /// The theme every window is drawn with.
    pub fn theme(&self) -> &Arc<ThemeTokens> {
        &self.options.theme
    }

    /// Draw every window with `theme` from now on. Text is measured and laid out again; focus and input-method
    /// compositions stay. A theme equal to the current one changes nothing.
    /// [`Runtime::with_theme_for`] on a runtime already made.
    pub fn set_theme_for(&mut self, theme_for: ThemeForAppearance) {
        self.theme_for = Some(theme_for);
        self.pinned_from = None;
    }

    /// Makes the themes of subtrees pinned to the light or the dark appearance again when the window's theme changed,
    /// once some subtree was pinned ([`pinned_themes`]).
    fn refresh_pinned(&mut self) {
        if self
            .pinned_from
            .as_ref()
            .is_some_and(|from| Arc::ptr_eq(from, &self.options.theme))
        {
            return;
        }
        self.pinned_from = Some(Arc::clone(&self.options.theme));
        self.pinned = pinned_themes(&mut self.theme_for, &self.options.theme);
    }

    pub fn set_theme(&mut self, theme: Arc<ThemeTokens>) {
        if *theme == *self.options.theme {
            return;
        }
        for w in &mut self.windows {
            w.env.theme = Arc::clone(&theme);
            sync_backdrop(&mut self.system, w);
            w.invalidate();
            w.invalidate_damage();
        }
        self.options.theme = theme;
    }

    pub fn model(&self) -> &A::Model {
        &self.model
    }

    pub fn window_system(&self) -> &W {
        &self.system
    }

    pub fn window_system_mut(&mut self) -> &mut W {
        &mut self.system
    }

    /// Whether the application asked to exit, or closed its last window and is not [resident](Application::RESIDENT).
    pub fn exited(&self) -> bool {
        self.exit
    }

    /// Runs until the application exits, waiting on the window system between steps.
    pub fn run(mut self) -> Result<(), UiError> {
        let turns = NonZeroU32::new(64).expect("64 is not zero");
        while !self.exit {
            if let StepStatus::BudgetExhausted(_) = self.step(turns)? {
                continue;
            }
            if !self.exit {
                // the time until the next event or frame goes to the widgets that asked for it
                let resume = self.run_idle()?;
                if self.exit || self.ready() {
                    continue;
                }
                // a wake due later bounds the wait; `ready` makes one already due run before waiting at all. Idle work
                // left for a frame that was due goes on once the frame is late, so a window the compositor stopped
                // drawing does not hold it back
                let wake = self.next_wake().into_iter().chain(resume).min();
                let timeout = wake.map(|at| {
                    Duration::from_nanos(at.0.saturating_sub(self.options.clock.now().0))
                });
                let events = &mut self.events;
                self.system
                    .dispatch(timeout, events)
                    .map_err(UiError::Platform)?;
            }
        }
        Ok(())
    }

    /// Does the work that is ready now, at most `max_turns` rounds of it, without blocking and without moving time.
    pub fn step(&mut self, max_turns: NonZeroU32) -> Result<StepStatus, UiError> {
        for _ in 0..max_turns.get() {
            if self.exit {
                return Ok(StepStatus::Idle);
            }
            self.system
                .dispatch(Some(Duration::ZERO), &mut self.events)
                .map_err(UiError::Platform)?;
            if let Some(each_turn) = self.each_turn.as_mut() {
                each_turn();
            }
            if !self.ready() {
                let pending = self.pending();
                return Ok(if pending == Pending::default() {
                    StepStatus::Idle
                } else {
                    StepStatus::Waiting(pending)
                });
            }
            self.turn()?;
        }
        Ok(if self.ready() {
            StepStatus::BudgetExhausted(self.pending())
        } else {
            StepStatus::Waiting(self.pending())
        })
    }

    fn next_wake(&self) -> Option<Timestamp> {
        let timer = self.timers.keys().next().map(|&(at, _)| at);
        self.windows
            .iter()
            .flat_map(|w| w.wakes.values())
            .copied()
            .chain(self.windows.iter().filter_map(|w| w.semantic_due))
            .chain(timer)
            .chain(self.next_fade_end())
            .min()
    }

    fn ready(&self) -> bool {
        !self.events.is_empty()
            || self
                .next_wake()
                .is_some_and(|at| at <= self.options.clock.now())
            || !self.messages.is_empty()
            || self.view_dirty
            || self.executor.has_ready()
            || self.accessibility_behind()
            || self.semantics_behind()
            || self.windows.iter().any(|w| w.dirty && w.may_draw())
    }

    fn pending(&self) -> Pending {
        Pending {
            messages: self.messages.len(),
            tasks: self.executor.pending(),
            frames: self
                .windows
                .iter()
                .filter(|w| w.dirty && !w.may_draw())
                .count(),
            animating: self.windows.iter().any(|w| !w.animate.is_empty()),
            next_wake: self.next_wake(),
        }
    }

    fn turn(&mut self) -> Result<(), UiError> {
        let _turn = tracing::trace_span!("ui.turn").entered();
        let began = self
            .frame_timing
            .as_mut()
            .map(|t| t.begin(self.options.clock.now()));
        let updating = tracing::trace_span!("ui.update").entered();
        tracing::trace_span!("ui.wakes").in_scope(|| {
            self.wake_due();
            self.advance_fades();
            self.advance_grows();
        });
        for event in std::mem::take(&mut self.events) {
            let _event = tracing::trace_span!(
                "ui.event",
                label = tracing::field::display(EventName(&event))
            )
            .entered();
            self.platform_event(event)?;
        }
        self.keyboard_after_dismissal();
        self.assistive_actions();
        self.semantic_actions();
        let ready = tracing::trace_span!("ui.tasks").in_scope(|| self.executor.run_ready());
        self.messages.extend(ready);
        while let Some(message) = self.messages.pop_front() {
            let _message = tracing::trace_span!(
                "ui.message",
                label = std::any::type_name::<A::Message>(),
                variant = tracing::field::debug(std::mem::discriminant(&message))
            )
            .entered();
            for command in A::update(&mut self.model, message) {
                self.command(command);
            }
            self.view_dirty = true;
        }
        self.finish_drops();
        drop(updating);
        lap(&mut self.frame_timing, Phase::Update, began);
        if self.view_dirty && !self.exit {
            self.view_dirty = false;
            let viewing = mark(&self.frame_timing);
            tracing::trace_span!("ui.view").in_scope(|| self.view())?;
            lap(&mut self.frame_timing, Phase::View, viewing);
        }
        let framed = tracing::trace_span!("ui.frame").in_scope(|| self.frame());
        if let Some(t) = &mut self.frame_timing {
            t.end();
        }
        framed
    }

    /// Gives the focus to the node keyed `key` in the first window that has one: with its ring showing when `ring`, else
    /// with the ring showing as it did.
    fn focus_key(&mut self, key: &Key, ring: bool) {
        let now = self.options.clock.now();
        for i in 0..self.windows.len() {
            let w = &mut self.windows[i];
            let Some(root) = w.root else { continue };
            let found = self.tree.preorder(root).into_iter().find(|&n| {
                self.tree
                    .get(n)
                    .is_some_and(|n| n.key.as_ref() == Some(key))
            });
            if let Some(node) = found {
                let visible = ring || w.focus.visible();
                let change = w.focus.set(&self.tree, Some(node), visible);
                let rest = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
                self.apply(i, rest);
                break;
            }
        }
    }

    fn command(&mut self, command: Command<A::Message>) {
        match command {
            Command::Perform(task) => self.executor.spawn(task),
            Command::Focus(key) => self.focus_key(&key, true),
            Command::MoveFocus(key) => self.focus_key(&key, false),
            Command::CloseWindow(key) => {
                if let Some(i) = self.windows.iter().position(|w| w.key == key) {
                    let w = self.windows.remove(i);
                    self.on_presented.retain(|(k, _)| *k != key);
                    self.close(w);
                }
            }
            Command::Window(key, request) => {
                if let Some(w) = self.windows.iter().find(|w| w.key == key) {
                    // A request the compositor refuses changes nothing the application relies on.
                    let _ = self.system.request(w.id, request);
                } else {
                    // a window the view that follows opens takes it before its first frame
                    self.to_open.push((key, request));
                }
            }
            Command::SetClipboard(text) => {
                Self::set_clipboard(&mut self.system, text);
                self.clipboard_offers(Some(&[TEXT_MIMES[0].to_owned()]));
            }
            Command::ActivationToken {
                window,
                app_id,
                on_token,
            } => match self.windows.iter().find(|w| w.key == window) {
                Some(w) => {
                    let request = self
                        .system
                        .request_activation_token(w.id, app_id.as_deref());
                    self.tokens.insert(request, on_token);
                }
                None => self.messages.push_back(on_token(None)),
            },
            Command::StartDrag(drag) => self.start_drag(drag),
            Command::FinishDrop(token, action) => self.drop_done(token, action),
            Command::HoldDrop(token) => self.hold_drop(token),
            Command::OnPresented { window, message } => {
                if self.windows.iter().any(|w| w.key == window && w.presented) {
                    self.messages.push_back(message);
                } else {
                    self.on_presented.push((window, message));
                }
            }
            Command::After { delay, message } => {
                let nanos = u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX);
                let at = Timestamp(self.options.clock.now().0.saturating_add(nanos));
                self.timers.insert((at, self.next_timer), message);
                self.next_timer += 1;
            }
            Command::Exit => self.exit = true,
        }
    }

    /// Draws the drag's image at its window's scale and starts it; a drag that cannot start is cancelled at once.
    fn start_drag(&mut self, drag: Drag<A::Message>) {
        let Drag {
            window,
            data,
            actions,
            image,
            mut on_progress,
        } = drag;
        let Some(w) = self.windows.iter().find(|w| w.key == window) else {
            self.messages
                .push_back(on_progress(DragProgress::Cancelled));
            return;
        };
        let (id, env, now) = (w.id, w.env.clone(), self.options.clock.now());
        // a drag without its image is still a drag
        let image = image.and_then(|preview| {
            drag_image(&mut self.options, &env, preview, now)
                .inspect_err(|e| eprintln!("cupertino-ui: the drag image was not drawn: {e:?}"))
                .ok()
        });
        let request = DragRequest {
            data,
            actions,
            image,
        };
        match self.system.start_drag(id, request) {
            Ok(token) => {
                self.drags.insert(token, on_progress);
            }
            Err(_) => self
                .messages
                .push_back(on_progress(DragProgress::Cancelled)),
        }
    }

    /// Closes the popups among `old` the view no longer declares (`declared` are the ones still to be opened), and
    /// the closing menus still fading, the latest first as the protocol destroys popups.
    fn close_leftover_popups(
        &mut self,
        old: &mut Vec<WindowState<A::Message>>,
        declared: &[WindowKey],
    ) {
        let mut i = old.len();
        while i > 0 {
            i -= 1;
            let w = &old[i];
            if w.popup.is_some() && (w.fade.is_some() || !declared.contains(&w.key)) {
                let w = old.remove(i);
                self.close(w);
            }
        }
    }

    fn close(&mut self, w: WindowState<A::Message>) {
        if let Some(popup) = &w.popup {
            self.forget_placement(&popup.parent, &w.key);
        }
        if let Some(root) = w.root {
            self.tree.unmount(root);
        }
        if let Some(adapter) = &mut self.options.accessibility {
            if w.fade.is_none() {
                adapter.window_closed(w.id);
            }
        }
        self.system.destroy(w.id);
    }

    /// Drops where the compositor placed popup `key` of `parent`: it moves or closes.
    fn forget_placement(&mut self, parent: &WindowKey, key: &WindowKey) {
        if let Some(parent) = self.windows.iter_mut().find(|w| &w.key == parent) {
            parent.popups.retain(|(k, _)| k != key);
        }
    }
}

use helpers::*;

mod drops;
mod fade;
mod frame;
mod handover;
mod helpers;
mod idle;
mod input;
mod morph;
mod semantics;
mod surface_layers;
mod ungrabbed;
mod windows;

pub use surface_layers::SurfaceLayerMark;
