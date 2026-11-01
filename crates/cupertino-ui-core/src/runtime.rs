//! The runtime: one UI thread that owns the model, the node tree and every window. A step takes the window system's
//! events, routes them, runs ready tasks, feeds messages to `update`, rebuilds the view when the model changed,
//! and then, per window, lays out, rebuilds semantics and focus, and paints and presents when the compositor is
//! ready for a frame. Time comes only from the injected clock and the window system's events.

use crate::command::CommandRegistry;
use crate::cx::{Effects, Status};
use crate::damage::DamageTracker;
use crate::focus::{Focus, FocusChange};
use crate::layout_view::{placements, TreeView};
use crate::paint::{PaintInput, Painter};
use crate::pointer::Pointer;
use crate::route::{hit_test, Geometry, Route};
use crate::scroll::{geometry_events, reveal_events};
use crate::semantics::{build, composing, diff, dismissible, protected};
use crate::task::Executor;
use crate::tree::Tree;
use crate::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, Application, Clock, Command,
    Element, Environment, Event, FocusTarget, FrameSnapshot, Icons, Key, NodeId, NodeSnapshot,
    Pending, Popup, PopupAnchor, Propagation, StepStatus, UiError, Window, WindowBackground,
    WindowKey,
};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_layout::{layout, Constraints, LayoutDirection, LayoutOptions, LayoutStats};
use cupertino_ui_platform::{
    Backdrop, ClipboardData, ClipboardKind, ConfigureId, ConstraintAdjust, KeyEvent, Keysym,
    PlatformError, PlatformEvent, PointerEvent, PopupEdge, PopupSpec, PresentOutcome,
    SystemAppearance, Timestamp, TouchEvent, WindowEvent, WindowId, WindowRequest, WindowSpec,
    WindowSystem,
};
use cupertino_ui_render::{
    CpuFrame, DamageRegion, DisplayList, PaintResources, RenderRequest, UiRenderer,
};
use cupertino_ui_text::TextSystem;
use cupertino_ui_theme::ThemeTokens;
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
    /// Hears about a window that could not be laid out, rendered or presented. The window keeps running and draws
    /// whole again after its next configure; the application does not end.
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
    }
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
    root: Option<NodeId>,
    env: Environment,
    size: Size,
    configured: bool,
    ack: Option<ConfigureId>,
    active: bool,
    /// The compositor is ready for a frame.
    frame_ready: bool,
    /// Something changed since the last presented frame.
    dirty: bool,
    /// Layout and semantics are up to date with the tree.
    prepared: bool,
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
    painter: Painter,
    damage: DamageTracker,
    display_list: DisplayList,
    /// The images the display list draws.
    resources: PaintResources,
    renderer: Box<dyn UiRenderer<Frame = CpuFrame>>,
    /// The error of the last frame, until one renders and presents; animations wait for that.
    failed: Option<UiError>,
    popup: Option<PopupState<M>>,
}

struct PopupState<M> {
    parent: WindowKey,
    anchor: PopupAnchor,
    /// What the content measured at the last view.
    size: Size,
    on_dismiss: M,
    /// Where the popup was last placed, so it moves only when its anchor or size changes.
    placed: PopupSpec,
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
    /// Whether the last press was a key: a popup opened by it shows its focus.
    keyboard: bool,
    exit: bool,
    theme_for: Option<ThemeForAppearance>,
}

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    pub fn new(system: W, options: RuntimeOptions) -> Self {
        let (model, commands) = A::init();
        let executor = Executor::new(system.waker());
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
            keyboard: false,
            exit: false,
            theme_for: None,
        };
        for command in commands {
            runtime.command(command);
        }
        runtime
    }

    /// Follow the session's appearance: `theme_for` makes the theme each time the window system reports it.
    pub fn with_theme_for(mut self, theme_for: ThemeForAppearance) -> Self {
        self.theme_for = Some(theme_for);
        self
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
            w.damage.invalidate();
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
        self.windows
            .iter()
            .flat_map(|w| w.wakes.values())
            .min()
            .copied()
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
        for event in std::mem::take(&mut self.events) {
            self.platform_event(event)?;
        }
        self.assistive_actions();
        self.messages.extend(self.executor.run_ready());
        while let Some(message) = self.messages.pop_front() {
            for command in A::update(&mut self.model, message) {
                self.command(command);
            }
            self.view_dirty = true;
        }
        if self.view_dirty && !self.exit {
            self.view_dirty = false;
            self.view()?;
        }
        self.frame()
    }

    fn command(&mut self, command: Command<A::Message>) {
        match command {
            Command::Perform(task) => self.executor.spawn(task),
            Command::Focus(key) => {
                let now = self.options.clock.now();
                for i in 0..self.windows.len() {
                    let w = &mut self.windows[i];
                    let Some(root) = w.root else { continue };
                    let found = self.tree.preorder(root).into_iter().find(|&n| {
                        self.tree
                            .get(n)
                            .is_some_and(|n| n.key.as_ref() == Some(&key))
                    });
                    if let Some(node) = found {
                        let change = w.focus.set(&self.tree, Some(node), true);
                        let rest =
                            Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
                        self.apply(i, rest);
                        break;
                    }
                }
            }
            Command::CloseWindow(key) => {
                if let Some(i) = self.windows.iter().position(|w| w.key == key) {
                    let w = self.windows.remove(i);
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
            Command::Exit => self.exit = true,
        }
    }

    fn close(&mut self, w: WindowState<A::Message>) {
        if let Some(root) = w.root {
            self.tree.unmount(root);
        }
        if let Some(adapter) = &mut self.options.accessibility {
            adapter.window_closed(w.id);
        }
        self.system.destroy(w.id);
    }

    /// Rebuilds every window from the model and reconciles it with the tree.
    fn view(&mut self) -> Result<(), UiError> {
        let mut old: Vec<WindowState<A::Message>> = std::mem::take(&mut self.windows);
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
        } in toplevels
        {
            let mut w = match old.iter().position(|w| w.key == key && w.popup.is_none()) {
                Some(i) => {
                    let mut w = old.remove(i);
                    if w.spec.title != spec.title {
                        let _ = self
                            .system
                            .request(w.id, WindowRequest::SetTitle(spec.title.clone()));
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
            w.background = background;
            sync_backdrop(&mut self.system, &mut w);
            w.root = Some(self.tree.reconcile(w.root, content)?);
            w.invalidate();
            self.windows.push(w);
        }
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
            } = popups.remove(i);
            let popup = popup.expect("partitioned as a popup");
            if let Some(mut w) = self.popup(key, spec, popup, content, &mut old)? {
                w.shortcuts = shortcuts;
                w.on_close = on_close;
                w.background = background;
                sync_backdrop(&mut self.system, &mut w);
                w.invalidate();
                self.windows.push(w);
            }
        }
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
            frame_ready: false,
            dirty: true,
            prepared: false,
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
            accessible: false,
            focusable: Vec::new(),
            roving_groups: Vec::new(),
            roving_items: Vec::new(),
            focus_request: None,
            reveal: None,
            scroll_heard: HashMap::new(),
            held_paints: 0,
            layout: LayoutStats::default(),
            painter: Painter::default(),
            damage: DamageTracker::default(),
            display_list: DisplayList::default(),
            resources: PaintResources::default(),
            renderer: (self.options.renderers)(),
            failed: None,
            popup: None,
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
            .position(|w| w.key == key && w.popup.is_some())
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
        let Some(placed) = place(&self.tree, parent, &popup.anchor, popup.grab, size) else {
            // a parent laid out without the anchor waits for the next view; retrying at once would never settle
            if parent.geometry.is_empty() && !self.awaiting_layout.contains(&popup.parent) {
                self.awaiting_layout.push(popup.parent);
            }
            self.tree.unmount(root);
            return Ok(None);
        };
        let id = match self.system.create_popup(parent.id, &placed) {
            Ok(id) => id,
            Err(e) => {
                self.tree.unmount(root);
                return Err(UiError::Platform(e));
            }
        };
        let mut w = self.new_window(key, id, spec);
        w.focus = Focus::shown(self.keyboard);
        w.env = env;
        w.size = size;
        w.popup = Some(PopupState {
            parent: popup.parent,
            anchor: popup.anchor,
            size,
            on_dismiss: popup.on_dismiss,
            placed,
        });
        w.root = Some(root);
        Ok(Some(w))
    }

    fn platform_event(&mut self, event: PlatformEvent) -> Result<(), UiError> {
        let (window, event) = match event {
            PlatformEvent::Window { window, event } => (window, event),
            PlatformEvent::Clipboard { .. }
            | PlatformEvent::ClipboardOffered { .. }
            | PlatformEvent::ActivationToken { .. }
            | PlatformEvent::Wake => return Ok(()),
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
        };
        let Some(i) = self.windows.iter().position(|w| w.id == window) else {
            return Ok(());
        };
        match &event {
            WindowEvent::Key(key, _) if key.pressed => self.keyboard = true,
            WindowEvent::Pointer(PointerEvent::Button { pressed: true, .. }, _)
            | WindowEvent::Touch(TouchEvent::Down { .. }, _) => self.keyboard = false,
            _ => {}
        }
        let now = self.options.clock.now();
        let w = &mut self.windows[i];
        let mut effects = Effects::default();
        match event {
            WindowEvent::Configure(c) => {
                w.size = c.size.unwrap_or(w.spec.size);
                w.ack = Some(c.id);
                w.configured = true;
                w.frame_ready = true;
                w.invalidate();
                w.damage.invalidate();
                if w.active != c.states.activated {
                    w.active = c.states.activated;
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
                    w.damage.invalidate();
                }
                return Ok(());
            }
            WindowEvent::Frame(time) => {
                w.frame_ready = true;
                let animate = std::mem::take(&mut w.animate);
                if !animate.is_empty() {
                    w.invalidate();
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
                    pointer.handle(&mut route, root, &w.geometry, event, time);
                    w.pointer = pointer;
                }
            }
            WindowEvent::Touch(event, time) => self.touch(i, event, time, now, &mut effects),
            WindowEvent::Key(key, time) => {
                self.key(i, key, time, now, &mut effects);
            }
            WindowEvent::Focus(focused) => {
                let changed = w.active != focused;
                w.active = focused;
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
            WindowEvent::Presented { .. } | WindowEvent::Discarded { .. } => return Ok(()),
        }
        self.apply(i, effects);
        Ok(())
    }

    /// Repaints every node of window `i`: its active state changed, which every control may show.
    fn invalidate_window(&mut self, i: usize) {
        let w = &mut self.windows[i];
        w.invalidate();
        w.damage.invalidate();
    }

    fn key(
        &mut self,
        i: usize,
        key: KeyEvent,
        time: Timestamp,
        now: Timestamp,
        effects: &mut Effects<A::Message>,
    ) {
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
        if key.keysym == TAB || key.keysym == LEFT_TAB {
            let forward =
                key.keysym == TAB && !key.mods.contains(cupertino_ui_platform::Mods::SHIFT);
            let next = w.focus.step(&self.tree, &w.focusable, forward);
            let change = w.focus.set(&self.tree, next, true);
            let rest = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
            effects.absorb(rest);
        }
    }

    fn touch(
        &mut self,
        i: usize,
        event: TouchEvent,
        time: Timestamp,
        now: Timestamp,
        effects: &mut Effects<A::Message>,
    ) {
        let w = &mut self.windows[i];
        let Some(root) = w.root else { return };
        let path = match event {
            TouchEvent::Down { id, position } => {
                let path = hit_test(&self.tree, root, &w.geometry, position).unwrap_or_default();
                w.touches.insert(id, path.clone());
                path
            }
            TouchEvent::Motion { id, .. } => w.touches.get(&id).cloned().unwrap_or_default(),
            TouchEvent::Up { id } => w.touches.remove(&id).unwrap_or_default(),
            TouchEvent::Cancel => {
                let targets: Vec<NodeId> = w
                    .touches
                    .drain()
                    .filter_map(|(_, p)| p.last().copied())
                    .collect();
                let mut route = route(&mut self.tree, w, now, effects);
                for node in targets {
                    route.notify(node, &Event::Cancel);
                }
                return;
            }
        };
        route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Touch(event, time));
    }

    /// Carries out what handlers asked for while one event was routed.
    /// Wakes every node whose time has come, before this turn's input.
    fn wake_due(&mut self) {
        let now = self.options.clock.now();
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
        for (node, change) in effects.changes {
            if self.tree.contains(node) {
                self.tree.mark(node, change);
                w.invalidate();
            }
        }
        if let Some(cursor) = effects.cursor {
            self.system.set_cursor(w.id, cursor);
        }
        if let Some(text) = effects.clipboard {
            Self::set_clipboard(&mut self.system, text);
        }
        w.wakes.extend(effects.wakes);
        if let Some((node, rect)) = effects.reveal {
            w.reveal = Some((node, Some(rect)));
            w.invalidate();
        }
        if !effects.animate.is_empty() {
            w.animate.extend(effects.animate);
            w.invalidate();
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
            w.text_input = state.as_ref().map(|_| node);
            self.system.set_text_input(w.id, state.as_ref());
        }
        if let Some((node, target)) = effects.focus_item {
            w.focus_request = roving_target(&self.tree, w, node, target);
            w.invalidate();
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
        w.invalidate();
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
                    w.damage.invalidate();
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
        Ok(())
    }

    /// Moves each open popup whose anchor moved or whose content changed size, once the parents are laid out.
    fn follow_anchors(&mut self) {
        for i in 0..self.windows.len() {
            let Some(state) = &self.windows[i].popup else {
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
            let w = &mut self.windows[i];
            let _ = self
                .system
                .request(w.id, WindowRequest::Reposition(placed.clone()));
            if let Some(state) = &mut w.popup {
                state.placed = placed;
            }
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
        if relayout || paint_dirty.iter().any(|n| w.geometry.contains_key(n)) {
            w.invalidate();
        }
        if !w.dirty {
            return Ok(());
        }
        let mut focus_effects = Effects::default();
        let mut hold = false;
        if !w.prepared {
            let available = Constraints::tight(w.size).map_err(UiError::Layout)?;
            let result = layout(
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
            )
            .map_err(UiError::Layout)?;
            w.layout = result.stats;
            w.geometry = placements(&self.tree, root, &result, &w.env);

            let semantics = build(
                &self.tree,
                root,
                &w.geometry,
                w.focus.node(),
                w.env.direction,
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
            let nodes = if refocused {
                build(
                    &self.tree,
                    root,
                    &w.geometry,
                    w.focus.node(),
                    w.env.direction,
                )
                .nodes
            } else {
                semantics.nodes
            };
            if let Some(adapter) = &mut self.options.accessibility {
                if adapter.is_active() && w.accessible {
                    let (changed, removed) = diff(&w.accessibility, &nodes);
                    if !changed.is_empty() || !removed.is_empty() {
                        adapter.update(&AccessibilityUpdate {
                            window: w.id,
                            root,
                            focus: w.focus.node(),
                            nodes: changed,
                            removed,
                            full: false,
                        });
                    }
                }
            }
            w.accessibility = nodes;
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
        let w = &mut self.windows[i];
        if hold {
            w.held_paints += 1;
            return Ok(());
        }

        if !(w.configured && w.frame_ready) {
            return Ok(());
        }
        let painting = w.painter.paint(
            &self.tree,
            root,
            &w.geometry,
            &PaintInput {
                env: &w.env,
                focus: w.focus.node(),
                focus_visible: w.focus.visible(),
                window_active: w.active,
                background: w.background,
                backdrop: w.backdrop.is_some(),
                now,
            },
        );
        let damage = w.damage.damage(&painting);
        w.animate.extend(painting.animating);
        w.display_list = painting.list;
        w.resources = painting.resources;
        w.dirty = false;
        w.held_paints = 0;
        if matches!(&damage, DamageRegion::Rects(r) if r.is_empty()) {
            return Ok(());
        }
        let scale = w.env.scale;
        let id = w
            .renderer
            .render(&RenderRequest {
                list: &w.display_list,
                resources: &w.resources,
                scale,
                width: physical(w.size.width, scale),
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
                self.system.request_frame(w.id);
            }
            PresentOutcome::NotReady => {
                w.frame_ready = false;
                w.dirty = true;
                w.damage.invalidate();
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
            let Some(root) = w.root else { continue };
            if w.accessible || !w.prepared {
                continue;
            }
            adapter.update(&AccessibilityUpdate {
                window: w.id,
                root,
                focus: w.focus.node(),
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
            self.windows
                .iter()
                .any(|w| w.root.is_some() && w.prepared && w.accessible != active)
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
            display_list: w.display_list.clone(),
            pending: self.pending(),
            layout: w.layout,
        })
    }

    /// The windows the application has open, in its view's order.
    pub fn windows(&self) -> Vec<(WindowKey, WindowId)> {
        self.windows.iter().map(|w| (w.key.clone(), w.id)).collect()
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
        },
        effects,
    }
}

/// Asks the window system for the material `w`'s background wants, or for none, when that changed. A platform that
/// refuses leaves the window on its opaque fill.
fn sync_backdrop<M>(system: &mut impl WindowSystem, w: &mut WindowState<M>) {
    let wanted = match w.background {
        WindowBackground::Surface { surface, radius } if system.capabilities().app_effects => w
            .env
            .theme
            .surface(surface)
            .material
            .map(|material| Backdrop {
                material: material.to_owned(),
                radius,
            }),
        _ => None,
    };
    if wanted == w.backdrop {
        return;
    }
    let shown = system
        .request(w.id, WindowRequest::Backdrop(wanted.clone()))
        .is_ok();
    w.backdrop = wanted.filter(|_| shown);
    w.invalidate();
}

#[cfg(test)]
mod tests;
