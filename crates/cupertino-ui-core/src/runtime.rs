//! The runtime: one UI thread that owns the model, the node tree and every window. A step takes the window system's
//! events, routes them, runs ready tasks, feeds messages to `update`, rebuilds the view when the model changed,
//! and then, per window, lays out, rebuilds semantics and focus, and paints and presents when the compositor is
//! ready for a frame. Time comes only from the injected clock and the window system's events.

use crate::command::CommandRegistry;
use crate::cx::{Effects, Status, WindowGesture};
use crate::damage::DamageTracker;
use crate::focus::{Focus, FocusChange};
use crate::layout_view::{placements, TreeView};
use crate::paint::{raised_nodes, same_environment, PaintInput, Painter};
use crate::pointer::Pointer;
use crate::route::{hit_test, Geometry, Route};
use crate::scroll::{geometry_events, reveal_events};
use crate::semantics::{build, composing, diff, dismissible, keeps_editing, protected, text_input};
use crate::task::Executor;
use crate::tree::Tree;
use crate::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, Application, Clock, Command,
    Drag, DragHandler, DragPreview, Element, Environment, Event, FocusTarget, FrameSnapshot, Icons,
    Key, Menus, NodeId, NodeSnapshot, Pending, Popup, PopupAnchor, Propagation, StepStatus,
    TokenHandler, UiError, Window, WindowBackground, WindowKey,
};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_menu_model::ActionName;
use cupertino_ui_layout::{
    layout, layout_cached, Constraints, LayoutCache, LayoutDirection, LayoutOptions, LayoutResult,
    LayoutStats,
};
use cupertino_ui_platform::{
    ActivationRequest, Backdrop, ClipboardData, ClipboardKind, ClipboardToken, ConfigureId,
    ConstraintAdjust, DragImage, DragProgress, DragRequest, DragToken, DropAction, DropToken,
    GlassTone, KeyEvent, Keysym, MaterialRegion, MenuDescription, Mods, MouseButton, PlatformError,
    PlatformEvent, PointerEvent, PopupEdge, PopupSpec, PresentOutcome, RaisedLayer,
    SystemAppearance, Timestamp, TouchEvent, WindowEvent, WindowId, WindowRequest, WindowSpec,
    WindowSystem,
};
use cupertino_ui_render::{
    CpuFrame, DamageRegion, DisplayList, PaintResources, RenderError, RenderRequest, UiRenderer,
};
use cupertino_ui_text::TextSystem;
use cupertino_ui_theme::{Color, Surface, ThemeTokens};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

const TAB: Keysym = Keysym(0xff09);
const ESCAPE: Keysym = Keysym(0xff1b);
/// How many frames in a row a window holds back while its scroll containers tell the model a new geometry.
const HELD_PAINTS: u8 = 4;
const LEFT_TAB: Keysym = Keysym(0xfe20);

pub type RendererFactory = Box<dyn FnMut() -> Box<dyn UiRenderer<Frame = CpuFrame>>>;
pub type WindowErrorHandler = Box<dyn FnMut(WindowId, &UiError)>;
/// Makes the theme for the session's appearance and display options.
pub type ThemeForAppearance = Box<dyn FnMut(&SystemAppearance) -> Arc<ThemeTokens>>;

/// Asked each time the window system wakes the loop: the theme to draw with when what a person chose changed since it
/// was last asked, with [`crate::set_personal`] already told the other choices.
pub type PersonalRefresh = Box<dyn FnMut() -> Option<Arc<ThemeTokens>>>;

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
    renderer: Option<Box<dyn UiRenderer<Frame = CpuFrame>>>,
    list: DisplayList,
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
    ack: Option<ConfigureId>,
    active: bool,
    /// A popup opened from this window has the keyboard, or had it as it closed and the window has not heard since
    /// where the keyboard went: the window still draws as active, as a menu takes the keyboard while the window it
    /// opened from stays the one in use. A popup that loses the keyboard while still open clears it.
    keyboard_in_popup: bool,
    /// The last configure said the window is maximized; a zoom asks for the other state.
    maximized: bool,
    /// The last configure said the window is fullscreen; a drag region neither moves nor zooms it.
    fullscreen: bool,
    /// The compositor is ready for a frame.
    frame_ready: bool,
    /// A frame of the window has been committed; the compositor has not necessarily shown it yet.
    committed: bool,
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
    /// When each node asked to be woken, one time per node; ordered so that nodes due together wake in a fixed order.
    wakes: BTreeMap<NodeId, Timestamp>,
    text_input: Option<NodeId>,
    geometry: Geometry,
    accessibility: BTreeMap<NodeId, AccessibilityNode>,
    /// The node of `accessibility` assistive technology is told holds the focus.
    accessibility_focus: Option<NodeId>,
    semantic_revision: u64,
    semantic_snapshot: Option<cupertino_semantics::Snapshot>,
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
    painter: Painter,
    damage: DamageTracker,
    /// The raised nodes ([`Widget::raised`]) of the last layout with their rects, in window coordinates.
    raised: Vec<(NodeId, Rect)>,
    /// The raised layer the window system shows over the window's content.
    layer: LayerState,
    display_list: DisplayList,
    /// The images the display list draws.
    resources: PaintResources,
    renderer: Box<dyn UiRenderer<Frame = CpuFrame>>,
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
    /// When the window last became active, while its colours still cross from the inactive ones.
    activated_at: Option<Timestamp>,
}

/// How long the colours of a window that becomes active take to cross from the inactive ones, in nanoseconds.
const ACTIVATION_NS: u64 = 120_000_000;

struct PopupState<M> {
    parent: WindowKey,
    anchor: PopupAnchor,
    /// What the content measured at the last view.
    size: Size,
    on_dismiss: M,
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
    personal_refresh: Option<PersonalRefresh>,
    /// Follows each scroll event to the screen, when asked ([`Runtime::with_scroll_timing`]).
    scroll_timing: Option<crate::scroll_timing::ScrollTimings>,
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
    /// The messages of [`Command::After`], by when they are due and then in the order they were asked for.
    timers: BTreeMap<(Timestamp, u64), A::Message>,
    /// The order of the next [`Command::After`].
    next_timer: u64,
    semantic_finishing: Vec<(
        WindowId,
        cupertino_semantics::ActionCall,
        cupertino_semantics::Outcome,
    )>,
    /// Popups the window system refused, kept closed until the view stops declaring them.
    refused_popups: Vec<WindowKey>,
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
            personal_refresh: None,
            scroll_timing: None,
            tokens: HashMap::new(),
            drags: HashMap::new(),
            landed: HashMap::new(),
            finishing: Vec::new(),
            held: Vec::new(),
            on_presented: Vec::new(),
            timers: BTreeMap::new(),
            next_timer: 0,
            semantic_finishing: Vec::new(),
            refused_popups: Vec::new(),
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

    /// Follow the session's appearance: `theme_for` makes the theme each time the window system reports it.
    pub fn with_theme_for(mut self, theme_for: ThemeForAppearance) -> Self {
        self.theme_for = Some(theme_for);
        self
    }

    /// Follow what a person chose: `refresh` is asked each time the loop wakes.
    pub fn with_personal_refresh(mut self, refresh: PersonalRefresh) -> Self {
        self.personal_refresh = Some(refresh);
        self
    }

    /// The theme every window is drawn with.
    pub fn theme(&self) -> &Arc<ThemeTokens> {
        &self.options.theme
    }

    /// Draw every window with `theme` from now on. Text is measured and laid out again; focus and input-method
    /// compositions stay. A theme equal to the current one changes nothing.
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
                // a wake due later bounds the wait; `ready` makes one already due run before waiting at all
                let timeout = self.next_wake().map(|at| {
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
            || self
                .windows
                .iter()
                .any(|w| w.dirty && w.configured && w.frame_ready)
    }

    fn pending(&self) -> Pending {
        Pending {
            messages: self.messages.len(),
            tasks: self.executor.pending(),
            frames: self
                .windows
                .iter()
                .filter(|w| w.dirty && !(w.configured && w.frame_ready))
                .count(),
            animating: self.windows.iter().any(|w| !w.animate.is_empty()),
            next_wake: self.next_wake(),
        }
    }

    fn turn(&mut self) -> Result<(), UiError> {
        self.wake_due();
        self.advance_fades();
        for event in std::mem::take(&mut self.events) {
            self.platform_event(event)?;
        }
        self.assistive_actions();
        self.semantic_actions();
        self.messages.extend(self.executor.run_ready());
        while let Some(message) = self.messages.pop_front() {
            for command in A::update(&mut self.model, message) {
                self.command(command);
            }
            self.view_dirty = true;
        }
        self.finish_drops();
        if self.view_dirty && !self.exit {
            self.view_dirty = false;
            self.view()?;
        }
        self.frame()
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
                }
            }
            Command::SetClipboard(text) => Self::set_clipboard(&mut self.system, text),
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

    /// Rebuilds every window from the model and reconciles it with the tree.
    fn view(&mut self) -> Result<(), UiError> {
        if let Some(timing) = &mut self.scroll_timing {
            timing.updated(self.options.clock.now());
        }
        let mut old: Vec<WindowState<A::Message>> = std::mem::take(&mut self.windows);
        let before: Vec<WindowKey> = old.iter().map(|w| w.key.clone()).collect();
        let (mut popups, toplevels): (Vec<_>, Vec<_>) = A::view(&self.model)
            .into_iter()
            .partition(|w| w.popup.is_some());
        for Window {
            key,
            spec,
            shortcuts,
            on_close,
            background,
            content,
            popup: _,
            menus,
        } in toplevels
        {
            let mut w = match old.iter().position(|w| w.key == key && w.popup.is_none()) {
                Some(i) => {
                    let mut w = old.remove(i);
                    if w.spec.title != spec.title {
                        let _ = self
                            .system
                            .request(w.id, WindowRequest::SetTitle(spec.title.clone()));
                        if let Some(adapter) = &mut self.options.accessibility {
                            if w.accessible {
                                adapter.window_title(w.id, &spec.title);
                            }
                        }
                    }
                    if w.spec.min_size != spec.min_size {
                        let _ = self
                            .system
                            .request(w.id, WindowRequest::SetMinSize(spec.min_size));
                    }
                    if w.spec.max_size != spec.max_size {
                        let _ = self
                            .system
                            .request(w.id, WindowRequest::SetMaxSize(spec.max_size));
                    }
                    if w.spec.input_region != spec.input_region {
                        let _ = self.system.request(
                            w.id,
                            WindowRequest::SetInputRegion(spec.input_region.clone()),
                        );
                    }
                    w.spec = spec;
                    w
                }
                None => {
                    let id = self
                        .system
                        .create_window(&spec)
                        .map_err(UiError::Platform)?;
                    self.new_window(key, id, spec)
                }
            };
            w.shortcuts = shortcuts;
            w.on_close = on_close;
            set_background(&mut w, background);
            sync_backdrop(&mut self.system, &mut w);
            sync_menus(&mut self.system, &mut w, menus);
            w.root = Some(self.tree.reconcile(w.root, content)?);
            // what changed size the tree marked for the next layout
            w.repaint();
            self.windows.push(w);
        }
        // a refused popup no longer declared may be asked for again
        self.refused_popups
            .retain(|key| popups.iter().any(|w| &w.key == key));
        // each popup after its parent, which may be a popup declared later; one without an open parent stays closed
        while let Some(i) = popups.iter().position(|w| {
            w.popup
                .as_ref()
                .is_some_and(|p| self.windows.iter().any(|open| open.key == p.parent))
        }) {
            let Window {
                key,
                spec,
                shortcuts,
                on_close,
                background,
                content,
                popup,
                menus: _,
            } = popups.remove(i);
            let popup = popup.expect("partitioned as a popup");
            if self.refused_popups.contains(&key) {
                continue;
            }
            // a new popup that takes the grab must stand on top of the grab's chain: the popups going away (a closing
            // menu among them) go first
            let reused = old
                .iter()
                .any(|w| w.key == key && w.popup.is_some() && w.fade.is_none());
            if popup.grab && !reused {
                let declared: Vec<WindowKey> = popups.iter().map(|w| w.key.clone()).collect();
                self.close_leftover_popups(&mut old, &declared);
            }
            if let Some(mut w) = self.popup(key, spec, popup, content, &mut old)? {
                w.shortcuts = shortcuts;
                w.on_close = on_close;
                set_background(&mut w, background);
                sync_backdrop(&mut self.system, &mut w);
                w.invalidate();
                self.windows.push(w);
            }
        }
        if std::mem::take(&mut self.touch_opened) {
            self.touch_taken();
        }
        // a menu the view opened that the last one did not have: the menus that close with it go at once
        let reopened = self.windows.iter().any(|w| {
            w.fade.is_none()
                && w.popup.is_some()
                && matches!(
                    w.background,
                    WindowBackground::Surface {
                        surface: Surface::Menu,
                        ..
                    }
                )
                && !before.contains(&w.key)
        });
        let mut old = self.fade_closed(old, reopened);
        // popups go before their parents, the latest first, as the protocol destroys them
        old.reverse();
        old.sort_by_key(|w| w.popup.is_none());
        for w in old {
            self.close(w);
        }
        let removed = self.tree.take_removed();
        if !removed.is_empty() {
            for w in &mut self.windows {
                w.pointer.retain(&self.tree);
                w.animate.retain(|n| self.tree.contains(*n));
                w.wakes.retain(|n, _| self.tree.contains(*n));
                w.touches
                    .retain(|_, path| path.last().is_some_and(|&n| self.tree.contains(n)));
                if w.text_input.is_some_and(|n| !self.tree.contains(n)) {
                    w.text_input = None;
                    self.system.set_text_input(w.id, None);
                }
            }
        }
        // a message waits only for a window the view has: one closed, or never opened, presents nothing
        let windows = &self.windows;
        self.on_presented
            .retain(|(key, _)| windows.iter().any(|w| &w.key == key));
        if self.windows.is_empty() && !A::RESIDENT {
            self.exit = true;
        }
        Ok(())
    }

    fn new_window(
        &mut self,
        key: WindowKey,
        id: WindowId,
        spec: WindowSpec,
    ) -> WindowState<A::Message> {
        WindowState {
            key,
            id,
            size: spec.size,
            spec,
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            backdrop: None,
            materials: Vec::new(),
            materials_refused: false,
            solid_shown: None,
            solid_refused: false,
            menus: None,
            on_menu: None,
            root: None,
            env: Environment {
                theme: Arc::clone(&self.options.theme),
                text: Arc::clone(&self.options.text),
                icons: Arc::clone(&self.options.icons),
                direction: self.options.direction,
                scale: 1.0,
            },
            configured: false,
            ack: None,
            active: false,
            keyboard_in_popup: false,
            maximized: false,
            fullscreen: false,
            frame_ready: false,
            committed: false,
            presented: false,
            dirty: true,
            prepared: false,
            laid_out: None,
            focus: Focus::default(),
            pointer: Pointer::default(),
            touches: HashMap::new(),
            held: Vec::new(),
            dismissed: None,
            animate: BTreeSet::new(),
            wakes: BTreeMap::new(),
            text_input: None,
            geometry: Geometry::new(),
            accessibility: BTreeMap::new(),
            accessibility_focus: None,
            semantic_revision: 0,
            semantic_snapshot: None,
            semantic_sent: None,
            semantic_due: None,
            accessible: false,
            focusable: Vec::new(),
            roving_groups: Vec::new(),
            roving_items: Vec::new(),
            focus_request: None,
            reveal: None,
            scroll_heard: HashMap::new(),
            held_paints: 0,
            layout: LayoutStats::default(),
            layout_cache: LayoutCache::new(),
            measured_in: None,
            painter: Painter::default(),
            damage: DamageTracker::default(),
            raised: Vec::new(),
            layer: LayerState::default(),
            display_list: DisplayList::default(),
            resources: PaintResources::default(),
            renderer: (self.options.renderers)(),
            failed: None,
            popup: None,
            opening_press: None,
            opening_touch: None,
            popups: Vec::new(),
            drop: None,
            fade: None,
            activated_at: None,
        }
    }

    /// Opens or keeps a declared popup of an open parent. `None` when the parent has not laid out its anchor yet.
    fn popup(
        &mut self,
        key: WindowKey,
        spec: WindowSpec,
        popup: Popup<A::Message>,
        content: Element<A::Message>,
        old: &mut Vec<WindowState<A::Message>>,
    ) -> Result<Option<WindowState<A::Message>>, UiError> {
        let mut existing = old
            .iter()
            .position(|w| w.key == key && w.popup.is_some() && w.fade.is_none())
            .map(|i| old.remove(i));
        let parent = self
            .windows
            .iter()
            .find(|w| w.key == popup.parent)
            .expect("a popup is opened after its parent");
        let root = self
            .tree
            .reconcile(existing.as_mut().and_then(|w| w.root.take()), content)?;
        let env = parent.env.clone();
        let measured = layout(
            &TreeView {
                tree: &self.tree,
                env: &env,
            },
            root,
            Constraints::loose(spec.size).map_err(UiError::Layout)?,
            &LayoutOptions {
                direction: env.direction,
                node_limit: self.options.node_limit,
            },
        )
        .map_err(UiError::Layout)?;
        let size = measured.nodes.get(&root).map_or(spec.size, |n| n.rect.size);
        if let Some(mut w) = existing {
            let state = w.popup.as_mut().expect("only popups are taken");
            state.parent = popup.parent;
            state.anchor = popup.anchor;
            state.size = size;
            state.on_dismiss = popup.on_dismiss;
            w.spec = spec;
            w.root = Some(root);
            return Ok(Some(w));
        }
        let grab = self.popup_grabs(popup.grab, parent);
        let Some(placed) = place(&self.tree, parent, &popup.anchor, grab, size) else {
            // a parent laid out without the anchor waits for the next view; retrying at once would never settle
            if parent.geometry.is_empty() && !self.awaiting_layout.contains(&popup.parent) {
                self.awaiting_layout.push(popup.parent.clone());
            }
            // an anchor node that came with this view has no place before the parent lays out: ask once more after it
            if anchor_declared(&self.tree, parent, &popup.anchor)
                && !self.awaiting_anchor.iter().any(|(k, _)| *k == popup.parent)
            {
                self.awaiting_anchor.push((popup.parent.clone(), false));
            }
            self.tree.unmount(root);
            return Ok(None);
        };
        let id = match self.system.create_popup(parent.id, &placed) {
            Ok(id) => id,
            Err(e) => {
                let parent = parent.id;
                self.tree.unmount(root);
                self.popup_refused(key, parent, popup.on_dismiss, e)?;
                return Ok(None);
            }
        };
        let mut w = self.new_window(key, id, spec);
        w.focus = Focus::shown(self.keyboard);
        w.opening_press = self.press;
        w.opening_touch = self.touch.map(|t| (t.id, t.start));
        self.touch_opened |= w.opening_touch.is_some();
        w.env = env;
        w.size = size;
        w.popup = Some(PopupState {
            parent: popup.parent,
            anchor: popup.anchor,
            size,
            on_dismiss: popup.on_dismiss,
            ungrabbed: popup.grab && !placed.grab,
            placed,
        });
        w.root = Some(root);
        Ok(Some(w))
    }

    fn platform_event(&mut self, event: PlatformEvent) -> Result<(), UiError> {
        let (window, event) = match event {
            PlatformEvent::Window { window, event } => (window, event),
            PlatformEvent::ActivationToken { request, token } => {
                if let Some(on_token) = self.tokens.remove(&request) {
                    self.messages.push_back(on_token(token));
                }
                return Ok(());
            }
            PlatformEvent::Drag { token, progress } => {
                let ended = matches!(progress, DragProgress::Finished | DragProgress::Cancelled);
                if let Some(on_progress) = self.drags.get_mut(&token) {
                    self.messages.push_back(on_progress(progress));
                }
                if ended {
                    self.drags.remove(&token);
                }
                return Ok(());
            }
            PlatformEvent::Clipboard { token, data } => {
                self.drop_data(token, data);
                return Ok(());
            }
            PlatformEvent::ClipboardOffered { .. } => return Ok(()),
            PlatformEvent::Wake => {
                if let Some(theme) = self.personal_refresh.as_mut().and_then(|refresh| refresh()) {
                    self.set_theme(theme);
                    // the views read the choices as they are built, and the controls draw them
                    self.view_dirty = true;
                    for w in &mut self.windows {
                        w.invalidate();
                        w.invalidate_damage();
                    }
                }
                return Ok(());
            }
            PlatformEvent::Appearance(appearance) => {
                if let Some(theme_for) = &mut self.theme_for {
                    let theme = theme_for(&appearance);
                    self.set_theme(theme);
                }
                return Ok(());
            }
            PlatformEvent::SessionLock(locked) => {
                if let Some(adapter) = &mut self.options.accessibility {
                    adapter.session_locked(locked);
                }
                return Ok(());
            }
            PlatformEvent::ScrollBars(bars) => {
                // the views pick their bars as they are built: build every window again
                if crate::scroll::set_session_scroll_bars(Some(bars)) {
                    self.view_dirty = true;
                    for w in &mut self.windows {
                        w.invalidate();
                    }
                }
                return Ok(());
            }
        };
        let Some(i) = self.windows.iter().position(|w| w.id == window) else {
            return Ok(());
        };
        // a closing menu hears nothing of the user, nor of the compositor dismissing it
        if self.windows[i].fade.is_some()
            && matches!(
                event,
                WindowEvent::Pointer(..)
                    | WindowEvent::Touch(..)
                    | WindowEvent::Key(..)
                    | WindowEvent::Focus(_)
                    | WindowEvent::Ime(_)
                    | WindowEvent::PopupDone
                    | WindowEvent::PopupPlaced(_)
                    | WindowEvent::CloseRequested
                    | WindowEvent::MenuActivated(_)
                    | WindowEvent::Drop(_)
            )
        {
            return Ok(());
        }
        let Some(i) = self.ungrabbed_input(i, &event) else {
            return Ok(());
        };
        match &event {
            WindowEvent::Key(key, _) if key.pressed => self.keyboard = true,
            WindowEvent::Pointer(PointerEvent::Button { pressed: true, .. }, _)
            | WindowEvent::Touch(TouchEvent::Down { .. }, _) => self.keyboard = false,
            _ => {}
        }
        let now = self.options.clock.now();
        // the press that ends here, if it opened popups the release is not on
        let ended_elsewhere = match event {
            WindowEvent::Pointer(
                PointerEvent::Button {
                    pressed: false,
                    button,
                    serial,
                    mods,
                    ..
                },
                time,
            ) => self
                .press
                .filter(|&start| self.windows[i].opening_press != Some(start))
                .map(|start| (start, button, serial, mods, time)),
            _ => None,
        };
        if let WindowEvent::Pointer(PointerEvent::Button { pressed, .. }, _) = event {
            self.press = pressed.then_some(now);
        }
        if let Some((start, button, serial, mods, time)) = ended_elsewhere {
            self.release_elsewhere(start, button, serial, mods, time, now);
        }
        let w = &mut self.windows[i];
        let mut effects = Effects::default();
        match event {
            WindowEvent::Configure(c) => {
                w.size = c.size.unwrap_or(w.spec.size);
                w.ack = Some(c.id);
                w.configured = true;
                w.maximized = c.states.maximized;
                w.fullscreen = c.states.fullscreen;
                // no longer the window in use, whatever its popups last had
                w.keyboard_in_popup &= c.states.activated;
                w.frame_ready = true;
                w.invalidate();
                w.invalidate_damage();
                if w.active != c.states.activated {
                    w.active = c.states.activated;
                    w.begin_activation(now);
                    if let Some(node) = w.focus.node() {
                        let event = Event::WindowActive(w.active);
                        route(&mut self.tree, w, now, &mut effects).notify(node, &event);
                    }
                    self.invalidate_window(i);
                    self.apply(i, effects);
                }
                return Ok(());
            }
            WindowEvent::CloseRequested => {
                match w.on_close.clone() {
                    Some(message) => self.messages.push_back(message),
                    None => self.exit = true,
                }
                return Ok(());
            }
            WindowEvent::Scale(scale) => {
                if scale.is_finite() && scale > 0.0 && scale != w.env.scale {
                    w.env.scale = scale;
                    w.invalidate();
                    w.invalidate_damage();
                }
                return Ok(());
            }
            WindowEvent::Frame(time) => {
                w.frame_ready = true;
                // the callback of a committed frame: the compositor has shown it, not only received it
                w.presented |= w.committed;
                let animate = std::mem::take(&mut w.animate);
                if w.activated_at.is_some() {
                    // the colours of an activated window are still crossing: paint it again
                    w.repaint();
                    w.invalidate_damage();
                }
                if !animate.is_empty() {
                    w.repaint();
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    for node in animate {
                        route.notify(node, &Event::AnimationFrame(time));
                    }
                }
            }
            WindowEvent::Pointer(event, time) => {
                if let Some(root) = w.root {
                    let mut pointer = std::mem::take(&mut w.pointer);
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    let scroll = matches!(event, PointerEvent::Scroll { .. });
                    pointer.handle(&mut route, root, &w.geometry, event, time);
                    w.pointer = pointer;
                    if let (true, Some(timing)) = (scroll, &mut self.scroll_timing) {
                        timing.handled(w.id, time, self.options.clock.now());
                    }
                    if let PointerEvent::Button {
                        pressed: true,
                        position,
                        ..
                    } = event
                    {
                        Self::press_away_from_text(
                            &mut self.tree,
                            w,
                            root,
                            position,
                            &mut effects,
                            now,
                        );
                    }
                }
            }
            WindowEvent::Touch(event, time) => self.touch(i, event, time, now, &mut effects),
            WindowEvent::Key(key, time) => {
                self.key(i, key, time, now, &mut effects);
            }
            WindowEvent::Focus(focused) => {
                let changed = w.active != focused;
                w.active = focused;
                if changed {
                    w.begin_activation(now);
                }
                if !focused {
                    let held: BTreeSet<NodeId> = w.held.drain(..).map(|(_, node)| node).collect();
                    w.dismissed = None;
                    let mut pointer = std::mem::take(&mut w.pointer);
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    pointer.revoke(&mut route);
                    for node in held {
                        if route.tree.contains(node) {
                            route.notify(node, &Event::Cancel);
                        }
                    }
                    w.pointer = pointer;
                }
                if let Some(node) = w.focus.node().filter(|_| changed) {
                    let mut ancestors = self.tree.path(node);
                    ancestors.pop();
                    if !focused {
                        ancestors.reverse();
                    }
                    let within = Event::FocusWithin {
                        gained: focused,
                        visible: focused && w.focus.visible(),
                    };
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    route.notify(node, &Event::WindowActive(focused));
                    for ancestor in ancestors {
                        route.notify(ancestor, &within);
                    }
                }
                if let Some(adapter) = &mut self.options.accessibility {
                    adapter.window_focus(w.id, focused);
                }
                self.invalidate_window(i);
                self.keyboard_moved(i, focused);
            }
            WindowEvent::Ime(ime) => {
                if let Some(node) = w.text_input.or(w.focus.node()) {
                    route(&mut self.tree, w, now, &mut effects).notify(node, &Event::Ime(ime));
                }
            }
            WindowEvent::PopupDone => {
                if let Some(popup) = &w.popup {
                    self.messages.push_back(popup.on_dismiss.clone());
                }
                return Ok(());
            }
            WindowEvent::PopupPlaced(rect) => {
                let key = w.key.clone();
                if let Some(parent) = w.popup.as_ref().map(|p| p.parent.clone()) {
                    if let Some(parent) = self.windows.iter_mut().find(|w| w.key == parent) {
                        parent.popups.retain(|(k, _)| *k != key);
                        parent.popups.push((key, rect));
                    }
                }
                return Ok(());
            }
            WindowEvent::MenuActivated(action) => {
                if let Some(on_menu) = &w.on_menu {
                    self.messages.push_back(on_menu(action));
                }
                return Ok(());
            }
            WindowEvent::Presented { frame, time } => {
                if let Some(timing) = &mut self.scroll_timing {
                    timing.presented(w.id, frame, Some(time));
                }
                return Ok(());
            }
            WindowEvent::Discarded { frame } => {
                if let Some(timing) = &mut self.scroll_timing {
                    timing.presented(w.id, frame, None);
                }
                return Ok(());
            }
            WindowEvent::GlassTone { region, tone } => {
                let tones = &mut w.painter.tones;
                let slot = match region {
                    None => &mut tones.window,
                    Some(i) if i < w.materials.len() => {
                        if tones.regions.len() <= i {
                            tones.regions.resize(i + 1, GlassTone::Appearance);
                        }
                        &mut tones.regions[i]
                    }
                    Some(_) => return Ok(()),
                };
                if *slot != tone {
                    *slot = tone;
                    // what lies on the glass paints again with the colours of its face
                    w.repaint();
                }
                return Ok(());
            }
            WindowEvent::Drop(event) => {
                self.drop_event(i, event);
                return Ok(());
            }
        }
        self.apply(i, effects);
        Ok(())
    }

    /// The press that began at `start` was let go away from the popups it opened: each one's root hears of the release
    /// at a point outside it, so a menu dragged through and let go elsewhere can close.
    fn release_elsewhere(
        &mut self,
        start: Timestamp,
        button: MouseButton,
        serial: u32,
        mods: Mods,
        time: Timestamp,
        now: Timestamp,
    ) {
        let opened: Vec<usize> = (0..self.windows.len())
            .filter(|&j| {
                let w = &self.windows[j];
                w.popup.is_some() && w.fade.is_none() && w.opening_press == Some(start)
            })
            .collect();
        let outside = Point::new(-f64::from(u16::MAX), -f64::from(u16::MAX));
        let release = Event::Pointer(
            PointerEvent::Button {
                position: outside,
                button,
                pressed: false,
                serial,
                mods,
            },
            time,
        );
        for j in opened {
            let mut effects = Effects::default();
            let w = &mut self.windows[j];
            if let Some(root) = w.root {
                route(&mut self.tree, w, now, &mut effects).notify(root, &release);
            }
            self.apply(j, effects);
        }
    }

    /// Repaints every node of window `i`: its active state changed, which every control may show.
    fn invalidate_window(&mut self, i: usize) {
        let w = &mut self.windows[i];
        w.invalidate();
        w.invalidate_damage();
    }

    /// Window `i` gained or lost the keyboard: a popup that gains it keeps the window its chain opens from drawn as
    /// active, until that window hears of the keyboard itself. A popup still open that loses it lets go: the keyboard
    /// went to another popup of the chain, which takes the window up again in the same dispatch, or somewhere else,
    /// which a compositor need not tell the window. One that closed before is no longer here to hear of it.
    fn keyboard_moved(&mut self, i: usize, focused: bool) {
        if self.windows[i].popup.is_none() {
            self.windows[i].keyboard_in_popup = false;
            return;
        }
        let mut key = self.windows[i].key.clone();
        // each popup opens after its parent, so the chain is never longer than the windows open
        for _ in 0..self.windows.len() {
            let Some(w) = self.windows.iter().find(|w| w.key == key) else {
                return;
            };
            match &w.popup {
                Some(popup) => key = popup.parent.clone(),
                None => break,
            }
        }
        if let Some(opener) = self.windows.iter().position(|w| w.key == key) {
            if std::mem::replace(&mut self.windows[opener].keyboard_in_popup, focused) != focused {
                self.invalidate_window(opener);
            }
        }
    }

    fn key(
        &mut self,
        i: usize,
        key: KeyEvent,
        time: Timestamp,
        now: Timestamp,
        effects: &mut Effects<A::Message>,
    ) {
        self.settle_focus_request(i, now);
        let w = &mut self.windows[i];
        let Some(root) = w.root else { return };
        let path = match w.focus.node() {
            Some(node) if self.tree.contains(node) => self.tree.path(node),
            _ => vec![root],
        };
        let held = HeldKey::of(&key);
        if !key.pressed && w.dismissed == Some(held) {
            w.dismissed = None;
            w.held.retain(|&(k, _)| k != held);
            return;
        }
        let escape = key.pressed && key.keysym == ESCAPE && key.mods.is_empty();
        if escape && !path.last().is_some_and(|&n| composing(&self.tree, n)) {
            let nodes = dismissible(&self.tree, root);
            let mut route = route(&mut self.tree, w, now, effects);
            let taken = nodes
                .into_iter()
                .rev()
                .find(|&node| route.offer(node, &Event::Dismiss) == Propagation::Stop);
            if let Some(node) = taken {
                w.held.retain(|&(k, _)| k != held);
                w.held.push((held, node));
                w.dismissed = Some(held);
                return;
            }
        }
        let handled =
            route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Key(key, time));
        w.held.retain(|&(k, _)| k != held);
        if handled == Propagation::Stop && key.pressed {
            w.held.extend(path.last().map(|&node| (held, node)));
        }
        if handled == Propagation::Stop || !key.pressed {
            return;
        }
        if let Some(message) = w.shortcuts.lookup(&key) {
            effects.messages.push(message);
            return;
        }
        if escape {
            Self::escape_from_text(&mut self.tree, w, effects, now);
            return;
        }
        if key.keysym == TAB || key.keysym == LEFT_TAB {
            let forward =
                key.keysym == TAB && !key.mods.contains(cupertino_ui_platform::Mods::SHIFT);
            let next = w.focus.step(&self.tree, &w.focusable, forward);
            let change = w.focus.set(&self.tree, next, true);
            let rest = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
            effects.absorb(rest);
        }
    }

    /// A press at `position` that lands outside the focused text input, where no node took the focus, ends its editing:
    /// the window keeps no focus, so typing goes nowhere until a field is chosen again.
    fn press_away_from_text(
        tree: &mut Tree<A::Message>,
        w: &mut WindowState<A::Message>,
        root: NodeId,
        position: Point,
        effects: &mut Effects<A::Message>,
        now: Timestamp,
    ) {
        if effects.focus.is_some() {
            return;
        }
        let Some(node) = w
            .focus
            .node()
            .filter(|&n| tree.contains(n) && text_input(tree, n))
        else {
            return;
        };
        let path = hit_test(tree, root, &w.geometry, position).unwrap_or_default();
        if path.contains(&node) || path.iter().any(|&n| keeps_editing(tree, n)) {
            return;
        }
        let change = w.focus.set(tree, None, false);
        let mut messages = VecDeque::new();
        let rest = Self::focus_events(tree, w, change, &mut messages, now);
        effects.messages.extend(messages);
        effects.absorb(rest);
    }

    /// Escape that nothing took, in a focused text input, ends its editing as a press away from it does.
    fn escape_from_text(
        tree: &mut Tree<A::Message>,
        w: &mut WindowState<A::Message>,
        effects: &mut Effects<A::Message>,
        now: Timestamp,
    ) {
        if !w
            .focus
            .node()
            .is_some_and(|n| tree.contains(n) && text_input(tree, n))
        {
            return;
        }
        let change = w.focus.set(tree, None, false);
        let mut messages = VecDeque::new();
        let rest = Self::focus_events(tree, w, change, &mut messages, now);
        effects.messages.extend(messages);
        effects.absorb(rest);
    }

    /// Moves the focus a roving item asked for before the next frame when the item is already a Tab stop, so a key
    /// that follows at once, such as Return after Down in a menu, goes to the item the first key chose.
    fn settle_focus_request(&mut self, i: usize, now: Timestamp) {
        let w = &mut self.windows[i];
        let Some(FocusRequest::Node(node)) = w.focus_request else {
            return;
        };
        if !self.tree.contains(node) || !w.focusable.contains(&node) {
            return;
        }
        w.focus_request = None;
        let change = w.focus.set(&self.tree, Some(node), true);
        let effects = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
        self.apply(i, effects);
    }

    fn touch(
        &mut self,
        i: usize,
        event: TouchEvent,
        time: Timestamp,
        now: Timestamp,
        effects: &mut Effects<A::Message>,
    ) {
        let window = self.windows[i].id;
        // the first finger down is the one a popup opened meanwhile follows; one whose window or path went away while
        // it was held no longer counts
        let gone = self.touch.is_some_and(|t| {
            !self
                .windows
                .iter()
                .any(|w| w.id == t.window && w.touches.contains_key(&t.id))
        });
        if gone {
            self.touch = None;
        }
        let held = self.touch.filter(|t| {
            t.window == window
                && match event {
                    TouchEvent::Motion { id, .. } | TouchEvent::Up { id } => id == t.id,
                    TouchEvent::Cancel => true,
                    TouchEvent::Down { .. } => false,
                }
        });
        if let (Some(t), TouchEvent::Motion { position, .. }) = (self.touch.as_mut(), event) {
            if held.is_some() {
                t.at = position;
            }
        }
        // the popups the held touch opened, each with its top-left corner in this window
        let opened: Vec<(usize, Point)> = held.map_or_else(Vec::new, |t| self.opened_by(i, t));
        if matches!(event, TouchEvent::Up { .. } | TouchEvent::Cancel) && held.is_some() {
            self.touch = None;
        }
        let w = &mut self.windows[i];
        let Some(root) = w.root else { return };
        // a popup follows the touch that opened it while it is held: another finger put down on it goes nowhere
        let following = self
            .touch
            .is_some_and(|t| w.opening_touch == Some((t.id, t.start)));
        let path = match event {
            TouchEvent::Down { .. } if following => return,
            TouchEvent::Down { id, position } => {
                let path = hit_test(&self.tree, root, &w.geometry, position).unwrap_or_default();
                w.touches.insert(id, path.clone());
                if self.touch.is_none() {
                    self.touch = Some(HeldTouch {
                        window,
                        id,
                        start: now,
                        at: position,
                    });
                }
                path
            }
            TouchEvent::Motion { id, .. } => w.touches.get(&id).cloned().unwrap_or_default(),
            TouchEvent::Up { id } => w.touches.remove(&id).unwrap_or_default(),
            TouchEvent::Cancel => {
                // every node a touch went through, the touched one first: a scroll view around it ends its drag too
                let mut targets: Vec<NodeId> = Vec::new();
                for (_, path) in w.touches.drain() {
                    for &node in path.iter().rev() {
                        if !targets.contains(&node) {
                            targets.push(node);
                        }
                    }
                }
                let mut route = route(&mut self.tree, w, now, effects);
                for node in targets {
                    route.notify(node, &Event::Cancel);
                }
                // the popups it opened end its interaction too, with nothing chosen
                for (j, _) in opened {
                    self.tell_popup(j, &Event::Touch(TouchEvent::Cancel, time), now);
                }
                return;
            }
        };
        let Some(t) = held else {
            route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
            return;
        };
        let size = |runtime: &Self, j: usize| runtime.windows[j].size;
        let over = opened
            .iter()
            .rev()
            .find(|&&(j, origin)| {
                Rect::new(
                    origin.x,
                    origin.y,
                    size(self, j).width,
                    size(self, j).height,
                )
                .contains(t.at)
            })
            .copied();
        match (event, over) {
            // lifted over a popup it opened, the touch is that popup's, as a press let go there: what it went through
            // here lets go of it without acting
            (TouchEvent::Up { id }, Some((j, origin))) => {
                let w = &mut self.windows[i];
                let mut route = route(&mut self.tree, w, now, effects);
                for &node in path.iter().rev() {
                    route.notify(node, &Event::Cancel);
                }
                // where it lifts, which the popup has not heard of when the touch never moved over it
                let at = Point::new(t.at.x - origin.x, t.at.y - origin.y);
                let moved = TouchEvent::Motion { id, position: at };
                self.tell_popup(j, &Event::Touch(moved, time), now);
                self.tell_popup(j, &Event::Touch(TouchEvent::Up { id }, time), now);
            }
            // lifted anywhere else, it goes where it lifts, and every popup it opened hears it lift at that point,
            // outside it, so a menu dragged through and let go elsewhere can close
            (TouchEvent::Up { id }, None) => {
                let w = &mut self.windows[i];
                route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
                for (j, origin) in opened {
                    let at = Point::new(t.at.x - origin.x, t.at.y - origin.y);
                    let moved = TouchEvent::Motion { id, position: at };
                    self.tell_popup(j, &Event::Touch(moved, time), now);
                    self.tell_popup(j, &Event::Touch(TouchEvent::Up { id }, time), now);
                }
            }
            // it moves over the popups it opened as well as here, at each one's own point
            (TouchEvent::Motion { id, position }, _) => {
                let w = &mut self.windows[i];
                route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
                for (j, origin) in opened {
                    let at = Point::new(position.x - origin.x, position.y - origin.y);
                    let moved = TouchEvent::Motion { id, position: at };
                    self.tell_popup(j, &Event::Touch(moved, time), now);
                }
            }
            _ => {
                let w = &mut self.windows[i];
                route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
            }
        }
    }

    /// The held touch opened a popup: in a window that is not a popup, what it went through lets go of it without
    /// acting, as the popup follows it from now on, so a button it rests on is not clicked as it lifts and a scroll view
    /// does not scroll under the menu. In a menu it went down in, the menu goes on following it.
    fn touch_taken(&mut self) {
        let Some(t) = self.touch else { return };
        let Some(k) = self.windows.iter().position(|w| w.id == t.window) else {
            return;
        };
        let now = self.options.clock.now();
        let mut effects = Effects::default();
        let w = &mut self.windows[k];
        if w.popup.is_some() {
            return;
        }
        let path = w.touches.get(&t.id).cloned().unwrap_or_default();
        let mut route = route(&mut self.tree, w, now, &mut effects);
        for &node in path.iter().rev() {
            route.notify(node, &Event::Cancel);
        }
        self.apply(k, effects);
    }

    /// The popups the touch `t`, held in window `i`, opened, each with its top-left corner in that window, the ones
    /// opened first first. A popup not placed yet, or not under that window, is not among them.
    fn opened_by(&self, i: usize, t: HeldTouch) -> Vec<(usize, Point)> {
        (0..self.windows.len())
            .filter(|&j| {
                let w = &self.windows[j];
                w.popup.is_some() && w.fade.is_none() && w.opening_touch == Some((t.id, t.start))
            })
            .filter_map(|j| self.origin_in(j, i).map(|origin| (j, origin)))
            .collect()
    }

    /// Where the compositor placed popup `j`'s top-left corner, in the coordinates of window `i` it opens from,
    /// directly or through other popups.
    fn origin_in(&self, j: usize, i: usize) -> Option<Point> {
        let mut at = Point::new(0.0, 0.0);
        let mut k = j;
        // each popup opens after its parent, so the chain is never longer than the windows open
        for _ in 0..=self.windows.len() {
            if k == i {
                return Some(at);
            }
            let w = &self.windows[k];
            let parent = &w.popup.as_ref()?.parent;
            let p = self.windows.iter().position(|o| &o.key == parent)?;
            let placed = self.windows[p]
                .popups
                .iter()
                .find(|(key, _)| *key == w.key)?
                .1;
            at = Point::new(at.x + placed.min_x(), at.y + placed.min_y());
            k = p;
        }
        None
    }

    /// Popup `j`'s root hears `event`, as a popup a press opened hears of its release elsewhere.
    fn tell_popup(&mut self, j: usize, event: &Event, now: Timestamp) {
        let mut effects = Effects::default();
        let w = &mut self.windows[j];
        if let Some(root) = w.root {
            route(&mut self.tree, w, now, &mut effects).notify(root, event);
        }
        self.apply(j, effects);
    }

    /// Carries out what handlers asked for while one event was routed.
    /// Wakes every node whose time has come, before this turn's input.
    fn wake_due(&mut self) {
        let now = self.options.clock.now();
        while let Some(entry) = self.timers.first_entry() {
            if entry.key().0 > now {
                break;
            }
            self.messages.push_back(entry.remove());
        }
        for i in 0..self.windows.len() {
            let w = &mut self.windows[i];
            let due: Vec<NodeId> = w
                .wakes
                .iter()
                .filter(|&(_, &at)| at <= now)
                .map(|(&node, _)| node)
                .collect();
            if due.is_empty() {
                continue;
            }
            w.wakes.retain(|_, at| *at > now);
            let mut effects = Effects::default();
            let mut route = route(&mut self.tree, w, now, &mut effects);
            for node in due {
                if route.tree.contains(node) {
                    route.notify(node, &Event::Wake(now));
                }
            }
            self.apply(i, effects);
        }
    }

    fn apply(&mut self, i: usize, effects: Effects<A::Message>) {
        let now = self.options.clock.now();
        let w = &mut self.windows[i];
        self.messages.extend(effects.messages);
        // a layout change is the tree's, which the next frame lays out
        for (node, change) in effects.changes {
            if self.tree.contains(node) {
                self.tree.mark(node, change);
                w.repaint();
            }
        }
        if let Some(cursor) = effects.cursor {
            self.system.set_cursor(w.id, cursor);
        }
        if let Some(text) = effects.clipboard {
            Self::set_clipboard(&mut self.system, text);
        }
        // a fullscreen window stays where it is: a maximize request would take it out of fullscreen
        for gesture in effects.window.into_iter().filter(|_| !w.fullscreen) {
            let request = match gesture {
                WindowGesture::Move => WindowRequest::Move,
                WindowGesture::Zoom => match crate::personal().titlebar_double_click {
                    cupertino_defaults::appearance::TitlebarAction::Zoom => {
                        WindowRequest::Maximize(!w.maximized)
                    }
                    cupertino_defaults::appearance::TitlebarAction::Minimize => {
                        WindowRequest::Minimize
                    }
                    cupertino_defaults::appearance::TitlebarAction::None => continue,
                },
            };
            // a platform without the request, or a press the compositor no longer holds, leaves the window where it is
            let _ = self.system.request(w.id, request);
        }
        w.wakes.extend(effects.wakes);
        // the overlays draw again over the paint the window keeps
        w.dirty |= effects.overlay;
        if let Some((node, rect)) = effects.reveal {
            w.reveal = Some((node, Some(rect)));
            w.repaint();
        }
        if !effects.animate.is_empty() {
            w.animate.extend(effects.animate);
            w.repaint();
            if w.failed.is_none() {
                self.system.request_frame(w.id);
            }
        }
        if let Some((node, mut state)) = effects.text_input {
            if let Some(state) = &mut state {
                if protected(&self.tree, node) {
                    state.surrounding = None;
                }
            }
            // another field takes the input method: the one before lets go of it first, so the input method starts
            // afresh and nothing it still composes for that field reaches this one
            if state.is_some() && w.text_input.is_some_and(|before| before != node) {
                self.system.set_text_input(w.id, None);
            }
            w.text_input = state.as_ref().map(|_| node);
            self.system.set_text_input(w.id, state.as_ref());
        }
        if let Some((node, target)) = effects.focus_item {
            w.focus_request = roving_target(&self.tree, w, node, target);
            w.repaint();
        }
        if let Some(node) = effects.focus {
            let change = w.focus.set(&self.tree, Some(node), false);
            let rest = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
            self.apply(i, rest);
            return;
        }
        if w.text_input.is_some() && w.text_input != w.focus.node() {
            w.text_input = None;
            self.system.set_text_input(w.id, None);
        }
    }

    /// Hands each action assistive technology asked for to its node, in the window whose published tree holds it.
    fn assistive_actions(&mut self) {
        let mut requests = Vec::new();
        if let Some(adapter) = &mut self.options.accessibility {
            adapter.drain_actions(&mut requests);
        }
        let now = self.options.clock.now();
        for request in requests {
            let target = request.target;
            let Some(i) = self
                .windows
                .iter()
                .position(|w| w.accessibility.contains_key(&target))
            else {
                continue;
            };
            if !self.tree.contains(target) {
                continue;
            }
            // whatever came last, a menu the action opens shows its selection, as one the Menu key opens does
            self.keyboard = true;
            let mut effects = Effects::default();
            route(&mut self.tree, &self.windows[i], now, &mut effects)
                .notify(target, &Event::Action(request));
            self.apply(i, effects);
        }
    }

    fn set_clipboard(system: &mut W, text: String) {
        system.set_clipboard(
            ClipboardKind::Clipboard,
            ClipboardData {
                mime: "text/plain;charset=utf-8".into(),
                bytes: text.into_bytes(),
            },
        );
    }

    /// Tells the nodes a focus move concerns, and repaints them. Returns what else their handlers asked for, such as
    /// the input method; a focus request made while focus moves is dropped, so two nodes cannot pass it back and forth.
    fn focus_events(
        tree: &mut Tree<A::Message>,
        w: &mut WindowState<A::Message>,
        change: Option<FocusChange>,
        messages: &mut VecDeque<A::Message>,
        now: Timestamp,
    ) -> Effects<A::Message> {
        let Some(change) = change else {
            return Effects::default();
        };
        w.repaint();
        if change.visible {
            w.reveal = change.to.map(|node| (node, None));
        }
        // the same node with its ring shown or hidden keeps its focus: no events, only a repaint
        if change.from == change.to {
            return Effects::default();
        }
        let mut effects = Effects::default();
        let mut route = route(tree, w, now, &mut effects);
        if let Some(from) = change.from {
            route.notify(
                from,
                &Event::Focus {
                    gained: false,
                    visible: false,
                },
            );
        }
        for &node in &change.left {
            route.notify(
                node,
                &Event::FocusWithin {
                    gained: false,
                    visible: false,
                },
            );
        }
        for &node in &change.entered {
            route.notify(
                node,
                &Event::FocusWithin {
                    gained: true,
                    visible: change.visible,
                },
            );
        }
        if let Some(to) = change.to {
            route.notify(
                to,
                &Event::Focus {
                    gained: true,
                    visible: change.visible,
                },
            );
        }
        messages.extend(effects.messages.drain(..));
        for (node, c) in effects.changes.drain(..) {
            if tree.contains(node) {
                tree.mark(node, c);
            }
        }
        effects.focus = None;
        effects
    }

    /// Lays out, rebuilds semantics and focus, and paints and presents every window that is due. A window that fails
    /// is reported and tries again whole on its next change or configure, whatever failed it: the renderer gives the same
    /// error for a target and for content too large to draw. The same error again is not reported again, and the
    /// window's animations ask for no frames until a frame presents. Only a lost connection ends the application.
    fn frame(&mut self) -> Result<(), UiError> {
        let relayout = self.tree.take_layout_dirty();
        let paint_dirty = self.tree.take_paint_dirty();
        let now = self.options.clock.now();
        for i in 0..self.windows.len() {
            match self.frame_window(i, relayout, &paint_dirty, now) {
                Ok(()) => {}
                Err(error @ UiError::Platform(PlatformError::Disconnected(_))) => {
                    return Err(error)
                }
                Err(error) => {
                    let w = &mut self.windows[i];
                    w.invalidate();
                    w.dirty = false;
                    w.invalidate_damage();
                    if w.failed.as_ref() != Some(&error) {
                        if let Some(report) = &mut self.options.on_window_error {
                            report(w.id, &error);
                        }
                    }
                    w.failed = Some(error);
                }
            }
        }
        self.follow_anchors();
        if !self.awaiting_anchor.is_empty() {
            self.awaiting_anchor.retain(|(_, asked)| !asked);
            for (_, asked) in &mut self.awaiting_anchor {
                *asked = true;
                self.view_dirty = true;
            }
        }
        if !self.awaiting_layout.is_empty() {
            let before = self.awaiting_layout.len();
            let windows = &self.windows;
            self.awaiting_layout.retain(|key| {
                windows
                    .iter()
                    .any(|w| &w.key == key && w.geometry.is_empty())
            });
            if self.awaiting_layout.len() < before {
                self.view_dirty = true;
            }
        }
        self.sync_accessibility();
        self.sync_semantics();
        self.send_presented();
        Ok(())
    }

    /// Sends the messages waiting for windows that have now presented a frame.
    fn send_presented(&mut self) {
        let windows = &self.windows;
        let (ready, waiting) = std::mem::take(&mut self.on_presented)
            .into_iter()
            .partition(|(key, _)| windows.iter().any(|w| &w.key == key && w.presented));
        self.on_presented = waiting;
        let ready: Vec<(WindowKey, A::Message)> = ready;
        self.messages.extend(ready.into_iter().map(|(_, m)| m));
    }

    /// Moves each open popup whose anchor moved or whose content changed size, once the parents are laid out.
    fn follow_anchors(&mut self) {
        for i in 0..self.windows.len() {
            let Some(state) = self.windows[i]
                .popup
                .as_ref()
                .filter(|_| self.windows[i].fade.is_none())
            else {
                continue;
            };
            let Some(placed) = self
                .windows
                .iter()
                .find(|w| w.key == state.parent)
                .and_then(|parent| {
                    place(
                        &self.tree,
                        parent,
                        &state.anchor,
                        state.placed.grab,
                        state.size,
                    )
                })
                .filter(|placed| *placed != state.placed)
            else {
                continue;
            };
            let parent = state.parent.clone();
            let w = &mut self.windows[i];
            let _ = self
                .system
                .request(w.id, WindowRequest::Reposition(placed.clone()));
            if let Some(state) = &mut w.popup {
                state.placed = placed;
            }
            let key = w.key.clone();
            self.forget_placement(&parent, &key);
        }
    }

    fn frame_window(
        &mut self,
        i: usize,
        relayout: bool,
        paint_dirty: &BTreeSet<NodeId>,
        now: Timestamp,
    ) -> Result<(), UiError> {
        let w = &mut self.windows[i];
        let Some(root) = w.root else {
            return Ok(());
        };
        if relayout {
            w.invalidate();
        } else if paint_dirty.iter().any(|n| w.geometry.contains_key(n)) {
            w.repaint();
        }
        if !w.dirty {
            return Ok(());
        }
        let mut focus_effects = Effects::default();
        let mut hold = false;
        let laid_out = !w.prepared;
        if !w.prepared {
            let result = match w.laid_out.take() {
                Some(result) => result,
                None => {
                    let available = Constraints::tight(w.size).map_err(UiError::Layout)?;
                    // a node measures in its environment's type and pixels, none of which its generation covers
                    if !w
                        .measured_in
                        .as_ref()
                        .is_some_and(|env| same_environment(env, &w.env))
                    {
                        w.layout_cache.clear();
                        w.measured_in = Some(w.env.clone());
                    }
                    let result = layout_cached(
                        &TreeView {
                            tree: &self.tree,
                            env: &w.env,
                        },
                        root,
                        available,
                        &LayoutOptions {
                            direction: w.env.direction,
                            node_limit: self.options.node_limit,
                        },
                        &mut w.layout_cache,
                    )
                    .map_err(UiError::Layout)?;
                    w.layout = result.stats;
                    result
                }
            };
            // scroll offsets and hit targets are the nodes' own state, so the placements are made again
            w.geometry = placements(&self.tree, root, &result, &w.env, &[]);
            w.raised = raised_nodes(&self.tree, root, &w.geometry);
            // raised nodes that draw in place leave what scrolls under them out: it would cover the glass under them
            if !w.raised.is_empty() && !self.system.capabilities().raised_layer {
                w.geometry = placements(&self.tree, root, &result, &w.env, &w.raised);
            }
            w.laid_out = Some(result);

            let semantics = build(
                &self.tree,
                root,
                &w.geometry,
                w.focus.node(),
                w.env.direction,
                &w.raised,
            );
            w.focusable = semantics.focusable;
            w.roving_groups = semantics.roving_groups;
            w.roving_items = semantics.roving_items;
            let requested = match w
                .focus_request
                .take()
                .and_then(|r| resolve(&self.tree, &w.roving_groups, &w.roving_items, r))
            {
                Some(item) if w.focusable.contains(&item) => {
                    w.focus.set(&self.tree, Some(item), true)
                }
                _ => None,
            };
            let change = w.focus.sync(
                &self.tree,
                &semantics.scopes,
                &w.focusable,
                &semantics.hidden,
            );
            let refocused = requested.is_some() || change.is_some();
            focus_effects =
                Self::focus_events(&mut self.tree, w, requested, &mut self.messages, now);
            focus_effects.absorb(Self::focus_events(
                &mut self.tree,
                w,
                change,
                &mut self.messages,
                now,
            ));
            let (nodes, accessibility_focus) = if refocused {
                let pass = build(
                    &self.tree,
                    root,
                    &w.geometry,
                    w.focus.node(),
                    w.env.direction,
                    &w.raised,
                );
                (pass.nodes, pass.focus)
            } else {
                (semantics.nodes, semantics.focus)
            };
            if let Some(adapter) = &mut self.options.accessibility {
                if adapter.is_active() && w.accessible {
                    let (changed, removed) = diff(&w.accessibility, &nodes);
                    if !changed.is_empty() || !removed.is_empty() {
                        adapter.update(&AccessibilityUpdate {
                            window: w.id,
                            root,
                            focus: accessibility_focus,
                            nodes: changed,
                            removed,
                            full: false,
                        });
                    }
                }
            }
            w.accessibility = nodes;
            w.accessibility_focus = accessibility_focus;
            w.prepared = true;

            let mut scroll = geometry_events(
                &self.tree,
                root,
                &w.geometry,
                w.env.direction,
                &mut w.scroll_heard,
            );
            if let Some((target, part)) = w.reveal.take() {
                scroll.extend(reveal_events(
                    &self.tree,
                    &w.geometry,
                    &w.raised,
                    w.env.direction,
                    target,
                    part,
                ));
            }
            let mut scroll_effects = Effects::default();
            let mut route = route(&mut self.tree, w, now, &mut scroll_effects);
            for (node, event) in &scroll {
                route.notify(*node, event);
            }
            // the model takes the new geometry before this frame paints, so a list never shows the rows of an old size
            hold = !scroll_effects.messages.is_empty() && w.held_paints < HELD_PAINTS;
            focus_effects.absorb(scroll_effects);
        }
        self.apply(i, focus_effects);
        // what lies under a drag that stood still may have moved
        if laid_out {
            self.drop_over(i);
        }
        let w = &mut self.windows[i];
        if hold {
            w.held_paints += 1;
            return Ok(());
        }

        if !(w.configured && w.frame_ready) {
            return Ok(());
        }
        if let (WindowBackground::Solid(color), false) = (w.background, w.solid_refused) {
            w.dirty = false;
            w.held_paints = 0;
            // the compositor still shows this colour at this size, and no configure waits for an answer
            if w.ack.is_none() && w.solid_shown == Some((w.size, color)) {
                return Ok(());
            }
            // the content paints nothing, so the glass it laid under itself goes with the colour's commit; drawn
            // again, the window asks for it anew
            if !w.materials.is_empty() {
                let _ = self
                    .system
                    .request(w.id, WindowRequest::Materials(Vec::new()));
                w.materials.clear();
                w.painter.tones.regions.clear();
            }
            // and the raised layer goes with it, its subsurface hidden with the colour's commit
            if w.layer.shown.is_some() {
                self.system
                    .present_layer(w.id, None)
                    .map_err(UiError::Platform)?;
                w.layer.shown = None;
                w.layer.damage.invalidate();
            }
            match self.system.present_solid(w.id, color, w.ack) {
                Ok(PresentOutcome::Presented) => {
                    w.failed = None;
                    w.solid_shown = Some((w.size, color));
                    w.ack = None;
                    w.frame_ready = false;
                    w.committed = true;
                    self.system.request_frame(w.id);
                    return Ok(());
                }
                Ok(PresentOutcome::NotReady) => {
                    w.failed = None;
                    w.frame_ready = false;
                    w.dirty = true;
                    self.system.request_frame(w.id);
                    return Ok(());
                }
                // drawn instead, filled with the colour, from now on
                Err(PlatformError::Unsupported) => {
                    w.solid_refused = true;
                    w.invalidate_damage();
                }
                Err(e) => return Err(UiError::Platform(e)),
            }
        }
        let capabilities = self.system.capabilities();
        let effects = capabilities.app_effects;
        let layered = capabilities.raised_layer;
        let paint = |w: &mut WindowState<A::Message>, tree: &Tree<A::Message>| {
            if w.activated_at.is_some() {
                // the colours are crossing: each node's last paint is of other colours, and the crossing is not part
                // of what a node's cached paint is keyed on
                w.painter.forget_nodes();
            }
            cupertino_ui_theme::set_activation(w.activation_progress(now));
            let mut painting = w.painter.paint(
                tree,
                root,
                &w.geometry,
                &PaintInput {
                    env: &w.env,
                    focus: w.focus.node(),
                    focus_visible: w.focus.visible(),
                    window_active: w.active || w.keyboard_in_popup,
                    background: w.background,
                    backdrop: w.backdrop.is_some(),
                    materials: effects && !w.materials_refused,
                    now,
                    raised: &w.raised,
                    layer: layered,
                },
            );
            cupertino_ui_theme::set_activation(1.0);
            if let Some(fade) = w.fade {
                fade::dim(&mut painting.list, fade.opacity);
            }
            painting
        };
        let mut painting = paint(w, &self.tree);
        // the shapes take effect with the frame's commit, so the frame presents in full
        if painting.materials != w.materials {
            let request = WindowRequest::Materials(painting.materials.clone());
            if self.system.request(w.id, request).is_ok() {
                w.materials = std::mem::take(&mut painting.materials);
            } else {
                w.materials_refused = true;
                w.materials.clear();
                painting = paint(w, &self.tree);
            }
            // regions past the new count lose their backdrops; one made again later starts unmeasured
            let kept = w.materials.len();
            w.painter.tones.regions.truncate(kept);
            w.invalidate_damage();
        }
        // the layer's height in whole points, from the window's top edge, as far down as its nodes draw
        let layer_height = painting
            .layer
            .as_ref()
            .map(|l| (l.bottom.min(w.size.height).ceil().max(1.0)) as u32);
        if layer_height != w.layer.shown {
            w.layer.damage.invalidate();
        }
        let damage = w.damage.damage(&painting);
        let layer_damage = painting
            .layer
            .as_ref()
            .map(|l| w.layer.damage.damage_of(&l.list, &l.own, &l.blank));
        w.animate.extend(painting.animating);
        w.display_list = painting.list;
        w.resources = painting.resources;
        w.dirty = false;
        w.held_paints = 0;
        let layer_changed = layer_height != w.layer.shown
            || layer_damage
                .as_ref()
                .is_some_and(|d| !matches!(d, DamageRegion::Rects(r) if r.is_empty()))
            || (painting.layer.is_some() && w.layer.damage.repainted());
        // a frame that repainted nothing sends nothing; one that repainted no pixel commits with no damage
        if matches!(&damage, DamageRegion::Rects(r) if r.is_empty())
            && !w.damage.repainted()
            && !layer_changed
        {
            return Ok(());
        }
        let scale = w.env.scale;
        let width = physical(w.size.width, scale);
        // the layer first: it shows with the content's commit
        if layer_changed {
            let shown = match (painting.layer, layer_height, layer_damage) {
                (Some(layer), Some(height), Some(damage)) => {
                    w.layer.list = layer.list;
                    let renderer = w
                        .layer
                        .renderer
                        .get_or_insert_with(&mut self.options.renderers);
                    let id = renderer
                        .render(&RenderRequest {
                            list: &w.layer.list,
                            resources: &w.resources,
                            scale,
                            width,
                            height: physical(f64::from(height), scale),
                            damage: &damage,
                        })
                        .map_err(UiError::Render)?;
                    let frame = renderer.frame(id).map_err(UiError::Render)?;
                    self.system
                        .present_layer(w.id, Some(RaisedLayer { frame, height }))
                        .map_err(UiError::Platform)?
                }
                _ => self
                    .system
                    .present_layer(w.id, None)
                    .map_err(UiError::Platform)?,
            };
            if shown == PresentOutcome::NotReady {
                w.frame_ready = false;
                w.dirty = true;
                w.invalidate_damage();
                w.layer.shown = None;
                self.system.request_frame(w.id);
                return Ok(());
            }
            w.layer.shown = layer_height;
        }
        let id = w
            .renderer
            .render(&RenderRequest {
                list: &w.display_list,
                resources: &w.resources,
                scale,
                width,
                height: physical(w.size.height, scale),
                damage: &damage,
            })
            .map_err(UiError::Render)?;
        let frame = w.renderer.frame(id).map_err(UiError::Render)?;
        let outcome = self
            .system
            .present(w.id, frame, w.ack)
            .map_err(UiError::Platform)?;
        w.failed = None;
        match outcome {
            PresentOutcome::Presented => {
                w.ack = None;
                w.frame_ready = false;
                w.committed = true;
                self.system.request_frame(w.id);
                if let Some(timing) = &mut self.scroll_timing {
                    timing.submitted(w.id, id, self.options.clock.now());
                }
            }
            PresentOutcome::NotReady => {
                w.frame_ready = false;
                w.dirty = true;
                w.invalidate_damage();
                self.system.request_frame(w.id);
            }
        }
        Ok(())
    }

    /// Gives an adapter that has just become active, or a window it has not seen yet, the whole tree; later frames
    /// send only what changed.
    fn sync_accessibility(&mut self) {
        let Some(adapter) = &mut self.options.accessibility else {
            return;
        };
        let active = adapter.is_active();
        for w in &mut self.windows {
            if !active {
                w.accessible = false;
                continue;
            }
            if w.fade.is_some() {
                continue;
            }
            let Some(root) = w.root else { continue };
            if w.accessible || !w.prepared {
                continue;
            }
            adapter.window_title(w.id, &w.spec.title);
            adapter.update(&AccessibilityUpdate {
                window: w.id,
                root,
                focus: w.accessibility_focus,
                nodes: w
                    .accessibility
                    .iter()
                    .map(|(&id, node)| (id, node.clone()))
                    .collect(),
                removed: Vec::new(),
                full: true,
            });
            w.accessible = true;
        }
    }

    fn accessibility_behind(&self) -> bool {
        self.options.accessibility.as_ref().is_some_and(|adapter| {
            let active = adapter.is_active();
            self.windows.iter().any(|w| {
                w.fade.is_none() && w.root.is_some() && w.prepared && w.accessible != active
            })
        })
    }

    /// Whether the current focus protects input. This reads widget semantics, including nodes without a role.
    pub fn protected_focus(&self, window: WindowId) -> Option<bool> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        Some(
            w.focus
                .node()
                .is_some_and(|node| protected(&self.tree, node)),
        )
    }

    /// A read-only view of one window after the last frame, for tests and the inspector.
    pub fn snapshot(&self, window: WindowId) -> Option<FrameSnapshot> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        let root = w.root?;
        let nodes = self
            .tree
            .preorder(root)
            .into_iter()
            .filter_map(|id| {
                let node = self.tree.get(id)?;
                let placement = w.geometry.get(&id);
                Some(NodeSnapshot {
                    id,
                    parent: node.parent,
                    children: node.children.clone(),
                    widget: node.widget.type_name(),
                    key: node.key.clone(),
                    rect: placement.map_or_else(Default::default, |p| p.rect),
                    clip: placement.and_then(|p| p.clip),
                })
            })
            .collect();
        Some(FrameSnapshot {
            window,
            nodes,
            focus: w.focus.node(),
            pointer_capture: w.pointer.capture(),
            text_input: w.text_input,
            accessibility: w
                .accessibility
                .iter()
                .map(|(&id, n)| (id, n.clone()))
                .collect(),
            accessibility_focus: w.accessibility_focus,
            display_list: w.display_list.clone(),
            pending: self.pending(),
            layout: w.layout,
        })
    }

    /// The windows the application has open, in its view's order.
    pub fn windows(&self) -> Vec<(WindowKey, WindowId)> {
        self.windows
            .iter()
            .filter(|w| w.fade.is_none())
            .map(|w| (w.key.clone(), w.id))
            .collect()
    }

    /// The node under `point` in `window`, from the last layout.
    pub fn node_at(&self, window: WindowId, point: Point) -> Option<NodeId> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        hit_test(&self.tree, w.root?, &w.geometry, point)?
            .last()
            .copied()
    }

    /// The node with `key` in `window`, searched in reading order.
    pub fn find(&self, window: WindowId, key: &Key) -> Option<NodeId> {
        let w = self.windows.iter().find(|w| w.id == window)?;
        self.tree.preorder(w.root?).into_iter().find(|&n| {
            self.tree
                .get(n)
                .is_some_and(|n| n.key.as_ref() == Some(key))
        })
    }
}

/// Whether the anchor's node is in the parent's tree but has no place yet.
fn anchor_declared<M: 'static>(
    tree: &Tree<M>,
    parent: &WindowState<M>,
    anchor: &PopupAnchor,
) -> bool {
    let (PopupAnchor::Below { key, .. }
    | PopupAnchor::Beside { key, .. }
    | PopupAnchor::Over { key, .. }
    | PopupAnchor::Above { key, .. }
    | PopupAnchor::CentredAbove { key, .. }) = anchor
    else {
        return false;
    };
    let Some(root) = parent.root else {
        return false;
    };
    tree.preorder(root)
        .into_iter()
        .find(|&n| tree.get(n).is_some_and(|n| n.key.as_ref() == Some(key)))
        .is_some_and(|node| parent.geometry.get(&node).is_none())
}

/// Where a popup of `parent` opens, in the parent's coordinates; `None` while its anchor is not laid out.
fn place<M: 'static>(
    tree: &Tree<M>,
    parent: &WindowState<M>,
    anchor: &PopupAnchor,
    grab: bool,
    size: Size,
) -> Option<PopupSpec> {
    let rect_of = |key: &Key| {
        let node = tree
            .preorder(parent.root?)
            .into_iter()
            .find(|&n| tree.get(n).is_some_and(|n| n.key.as_ref() == Some(key)))?;
        Some(parent.geometry.get(&node)?.rect)
    };
    let rtl = parent.env.direction == LayoutDirection::Rtl;
    let (anchor_rect, edge, gravity, offset) = match anchor {
        PopupAnchor::Below { key, gap } => (
            rect_of(key)?,
            PopupEdge::Bottom,
            PopupEdge::Bottom,
            Point::new(0.0, *gap),
        ),
        PopupAnchor::At(point) => {
            let edge = if rtl {
                PopupEdge::BottomLeft
            } else {
                PopupEdge::BottomRight
            };
            (
                Rect::new(point.x, point.y, 1.0, 1.0),
                edge,
                edge,
                Point::ZERO,
            )
        }
        PopupAnchor::Beside { key, offset } if rtl => (
            rect_of(key)?,
            PopupEdge::TopLeft,
            PopupEdge::BottomLeft,
            Point::new(-offset.x, offset.y),
        ),
        PopupAnchor::Beside { key, offset } => (
            rect_of(key)?,
            PopupEdge::TopRight,
            PopupEdge::BottomRight,
            *offset,
        ),
        PopupAnchor::CentredAbove { key, gap } => (
            rect_of(key)?,
            PopupEdge::Top,
            PopupEdge::Top,
            Point::new(0.0, -gap),
        ),
        PopupAnchor::Above { key, gap, dx } if rtl => (
            rect_of(key)?,
            PopupEdge::Top,
            PopupEdge::TopLeft,
            Point::new(-dx, -gap),
        ),
        PopupAnchor::Above { key, gap, dx } => (
            rect_of(key)?,
            PopupEdge::Top,
            PopupEdge::TopRight,
            Point::new(*dx, -gap),
        ),
        PopupAnchor::Over { key, offset } if rtl => (
            rect_of(key)?,
            PopupEdge::Right,
            PopupEdge::BottomLeft,
            Point::new(-offset.x, offset.y),
        ),
        PopupAnchor::Over { key, offset } => (
            rect_of(key)?,
            PopupEdge::Left,
            PopupEdge::BottomRight,
            *offset,
        ),
    };
    Some(PopupSpec {
        anchor_rect,
        anchor: edge,
        gravity,
        offset,
        size,
        constraint: ConstraintAdjust {
            slide: true,
            flip: !matches!(anchor, PopupAnchor::Over { .. }),
            resize: false,
        },
        reactive: true,
        grab,
    })
}

/// The buffer size of a logical length, rounded to the nearest pixel with halves up, as the window system sizes the
/// buffer it accepts for a configure at a fractional scale.
fn physical(logical: f64, scale: f64) -> u32 {
    (logical * scale).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The image of a drag from a window with `env`: its content in a tree of its own, laid out at whole points and painted
/// over nothing at whole pixels per point, so each side of the buffer is a whole number of points.
fn drag_image<M: 'static>(
    options: &mut RuntimeOptions,
    env: &Environment,
    preview: DragPreview<M>,
    now: Timestamp,
) -> Result<DragImage, UiError> {
    let DragPreview {
        content,
        size,
        hotspot,
    } = preview;
    let scale = if env.scale.is_finite() {
        env.scale.ceil().clamp(1.0, f64::from(MAX_DRAG_SCALE))
    } else {
        1.0
    };
    let whole = |v: f64| {
        if v.is_finite() {
            v.ceil().max(1.0)
        } else {
            1.0
        }
    };
    let size = Size::new(whole(size.width), whole(size.height));
    let env = Environment {
        scale,
        ..env.clone()
    };
    let mut tree = Tree::new(options.node_limit);
    let root = tree.reconcile(None, content)?;
    let result = layout(
        &TreeView {
            tree: &tree,
            env: &env,
        },
        root,
        Constraints::tight(size).map_err(UiError::Layout)?,
        &LayoutOptions {
            direction: env.direction,
            node_limit: options.node_limit,
        },
    )
    .map_err(UiError::Layout)?;
    let geometry = placements(&tree, root, &result, &env, &[]);
    let painting = Painter::default().paint(
        &tree,
        root,
        &geometry,
        &PaintInput {
            env: &env,
            focus: None,
            focus_visible: false,
            window_active: true,
            background: WindowBackground::None,
            backdrop: false,
            // nothing draws a compositor material under a drag icon, so material nodes paint their own fill
            materials: false,
            now,
            raised: &[],
            layer: false,
        },
    );
    let mut renderer = (options.renderers)();
    let (width, height) = (physical(size.width, scale), physical(size.height, scale));
    let id = renderer
        .render(&RenderRequest {
            list: &painting.list,
            resources: &painting.resources,
            scale,
            width,
            height,
            damage: &DamageRegion::Full,
        })
        .map_err(UiError::Render)?;
    let frame = renderer.frame(id).map_err(UiError::Render)?;
    let row = width as usize * 4;
    if (frame.width, frame.height) != (width, height)
        || (frame.stride as usize) < row
        || frame.pixels.len() < frame.stride as usize * height as usize
    {
        return Err(UiError::Render(RenderError::InvalidSize));
    }
    let pixels = frame
        .pixels
        .chunks(frame.stride as usize)
        .take(height as usize)
        .flat_map(|line| &line[..row])
        .copied()
        .collect();
    Ok(DragImage {
        width,
        height,
        scale: scale as u32,
        pixels,
        hotspot,
    })
}

/// The most pixels per point a drag image is drawn at.
const MAX_DRAG_SCALE: u32 = 4;

/// A roving focus move waiting for the next frame.
enum FocusRequest {
    Node(NodeId),
    /// The item with this key in this group, found once the view is rebuilt.
    Key(NodeId, Key),
}

/// The nearest roving group above `node`.
fn group_of<M: 'static>(tree: &Tree<M>, groups: &[NodeId], node: NodeId) -> Option<NodeId> {
    tree.path(node)
        .into_iter()
        .rev()
        .skip(1)
        .find(|a| groups.contains(a))
}

fn resolve<M: 'static>(
    tree: &Tree<M>,
    groups: &[NodeId],
    items: &[NodeId],
    request: FocusRequest,
) -> Option<NodeId> {
    match request {
        FocusRequest::Node(node) => Some(node),
        FocusRequest::Key(group, key) => items.iter().copied().find(|&item| {
            tree.get(item).is_some_and(|n| n.key.as_ref() == Some(&key))
                && group_of(tree, groups, item) == Some(group)
        }),
    }
}

/// The item `target` names among the items of `node`'s roving group (the group itself, or the nearest one above it),
/// counted from the focused item, else from `node`.
fn roving_target<M: 'static>(
    tree: &Tree<M>,
    w: &WindowState<M>,
    node: NodeId,
    target: FocusTarget,
) -> Option<FocusRequest> {
    let group = if w.roving_groups.contains(&node) {
        node
    } else {
        group_of(tree, &w.roving_groups, node)?
    };
    if let FocusTarget::Key(key) = target {
        return Some(FocusRequest::Key(group, key));
    }
    let items: Vec<NodeId> = w
        .roving_items
        .iter()
        .copied()
        .filter(|&item| group_of(tree, &w.roving_groups, item) == Some(group))
        .collect();
    let n = items.len();
    if n == 0 {
        return None;
    }
    let at = |id: NodeId| items.iter().position(|&item| item == id);
    let current = w.focus.node().and_then(at).or_else(|| at(node));
    let index = match target {
        FocusTarget::Next => current.map_or(0, |c| (c + 1) % n),
        FocusTarget::Previous => current.map_or(n - 1, |c| (c + n - 1) % n),
        FocusTarget::First => 0,
        FocusTarget::Last => n - 1,
        FocusTarget::Index(k) => (k < n).then_some(k)?,
        FocusTarget::Key(_) => unreachable!("resolved after the rebuild, above"),
    };
    Some(FocusRequest::Node(items[index]))
}

fn route<'a, M: 'static>(
    tree: &'a mut Tree<M>,
    w: &'a WindowState<M>,
    now: Timestamp,
    effects: &'a mut Effects<M>,
) -> Route<'a, M> {
    Route {
        tree,
        geometry: &w.geometry,
        env: &w.env,
        status: Status {
            focus: w.focus.node(),
            focus_visible: w.focus.visible(),
            capture: w.pointer.capture(),
            window_active: w.active,
            now,
            opening_press: w.opening_press.or(w.opening_touch.map(|(_, start)| start)),
        },
        popups: &w.popups,
        effects,
    }
}

/// Gives `w` the background `background`. A window that leaves a solid colour shown without a frame draws its next
/// frame whole, and one that turns solid shows its colour with its next frame.
fn set_background<M>(w: &mut WindowState<M>, background: WindowBackground) {
    let solid = |b: WindowBackground| matches!(b, WindowBackground::Solid(_));
    if w.background != background
        && (solid(w.background) || solid(background))
        && w.solid_shown.take().is_some()
    {
        w.damage.invalidate();
    }
    w.background = background;
}

/// Asks the window system for the material `w`'s background wants, or for none, when that changed. A platform that
/// refuses leaves the window on its opaque fill.
fn sync_backdrop<M>(system: &mut impl WindowSystem, w: &mut WindowState<M>) {
    let shaped = match w.background {
        WindowBackground::Surface {
            surface,
            radius,
            alpha,
        } => Some((surface, radius, alpha)),
        _ => None,
    };
    let wanted = match shaped {
        Some((surface, radius, alpha)) if system.capabilities().app_effects => w
            .env
            .theme
            .surface(surface)
            .material
            .map(|material| Backdrop {
                material: material.to_owned(),
                radius,
                alpha: crate::surface_alpha(alpha),
            }),
        _ => None,
    };
    if wanted == w.backdrop {
        return;
    }
    let shown = system
        .request(w.id, WindowRequest::Backdrop(wanted.clone()))
        .is_ok();
    // another material is a new backdrop, whose face nothing measured yet
    if wanted.as_ref().map(|b| &b.material) != w.backdrop.as_ref().map(|b| &b.material) {
        w.painter.tones.window = GlassTone::Appearance;
    }
    w.backdrop = wanted.filter(|_| shown);
    // the material is shaped with the next present, which a change that paints nothing new (new corners on a material
    // the compositor draws) must still bring
    w.invalidate();
    w.invalidate_damage();
}

/// Asks the window system to publish `menus`, or to withdraw them, when they changed.
fn sync_menus<M>(system: &mut impl WindowSystem, w: &mut WindowState<M>, menus: Option<Menus<M>>) {
    let (wanted, on_menu) = menus
        .map(|m| {
            let description = MenuDescription {
                bar: m.bar,
                disabled: m.disabled,
            };
            (description, m.on_activate)
        })
        .unzip();
    w.on_menu = on_menu;
    if wanted != w.menus {
        let _ = system.request(w.id, WindowRequest::Menus(wanted.clone()));
        w.menus = wanted;
    }
}

mod drops;
mod fade;
mod semantics;
mod ungrabbed;

#[cfg(test)]
mod tests;
