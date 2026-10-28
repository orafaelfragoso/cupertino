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
use crate::semantics::{build, diff, protected};
use crate::task::Executor;
use crate::tree::Tree;
use crate::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, Application, Clock, Command,
    Environment, Event, FrameSnapshot, Key, NodeId, NodeSnapshot, Pending, Propagation, StepStatus,
    UiError, Window, WindowKey,
};
use cupertino_geometry::{Point, Size};
use cupertino_ui_layout::{layout, Constraints, LayoutDirection, LayoutError, LayoutOptions};
use cupertino_ui_platform::{
    ClipboardData, ClipboardKind, ConfigureId, KeyEvent, Keysym, PlatformError, PlatformEvent,
    PresentOutcome, Timestamp, TouchEvent, WindowEvent, WindowId, WindowRequest, WindowSpec,
    WindowSystem,
};
use cupertino_ui_render::{
    CpuFrame, DamageRegion, DisplayList, PaintResources, RenderError, RenderRequest, UiRenderer,
};
use cupertino_ui_theme::ThemeTokens;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

const TAB: Keysym = Keysym(0xff09);
const LEFT_TAB: Keysym = Keysym(0xfe20);

pub type RendererFactory = Box<dyn FnMut() -> Box<dyn UiRenderer<Frame = CpuFrame>>>;
pub type WindowErrorHandler = Box<dyn FnMut(WindowId, &UiError)>;

/// How a runtime is set up besides its window system.
pub struct RuntimeOptions {
    pub theme: Arc<ThemeTokens>,
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
    animate: BTreeSet<NodeId>,
    text_input: Option<NodeId>,
    geometry: Geometry,
    accessibility: BTreeMap<NodeId, AccessibilityNode>,
    /// The adapter holds this window's whole tree, so a diff is enough.
    accessible: bool,
    focusable: Vec<NodeId>,
    painter: Painter,
    damage: DamageTracker,
    display_list: DisplayList,
    renderer: Box<dyn UiRenderer<Frame = CpuFrame>>,
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
    exit: bool,
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
            exit: false,
        };
        for command in commands {
            runtime.command(command);
        }
        runtime
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

    /// Whether the application asked to exit or closed its last window.
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
                let events = &mut self.events;
                self.system
                    .dispatch(None, events)
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

    fn ready(&self) -> bool {
        !self.events.is_empty()
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
        }
    }

    fn turn(&mut self) -> Result<(), UiError> {
        for event in std::mem::take(&mut self.events) {
            self.platform_event(event)?;
        }
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
            Command::SetClipboard(text) => self.system.set_clipboard(
                ClipboardKind::Clipboard,
                ClipboardData {
                    mime: "text/plain;charset=utf-8".into(),
                    bytes: text.into_bytes(),
                },
            ),
            Command::Exit => self.exit = true,
        }
    }

    fn close(&mut self, w: WindowState<A::Message>) {
        if let Some(root) = w.root {
            self.tree.unmount(root);
        }
        self.system.destroy(w.id);
    }

    /// Rebuilds every window from the model and reconciles it with the tree.
    fn view(&mut self) -> Result<(), UiError> {
        let mut old: Vec<WindowState<A::Message>> = std::mem::take(&mut self.windows);
        for Window {
            key,
            spec,
            shortcuts,
            on_close,
            content,
        } in A::view(&self.model)
        {
            let mut w = match old.iter().position(|w| w.key == key) {
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
            w.root = Some(self.tree.reconcile(w.root, content)?);
            w.invalidate();
            self.windows.push(w);
        }
        for w in old {
            self.close(w);
        }
        let removed = self.tree.take_removed();
        if !removed.is_empty() {
            for w in &mut self.windows {
                w.pointer.retain(&self.tree);
                w.animate.retain(|n| self.tree.contains(*n));
                w.touches
                    .retain(|_, path| path.last().is_some_and(|&n| self.tree.contains(n)));
                if w.text_input.is_some_and(|n| !self.tree.contains(n)) {
                    w.text_input = None;
                    self.system.set_text_input(w.id, None);
                }
            }
        }
        if self.windows.is_empty() {
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
            root: None,
            env: Environment {
                theme: Arc::clone(&self.options.theme),
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
            animate: BTreeSet::new(),
            text_input: None,
            geometry: Geometry::new(),
            accessibility: BTreeMap::new(),
            accessible: false,
            focusable: Vec::new(),
            painter: Painter::default(),
            damage: DamageTracker::default(),
            display_list: DisplayList::default(),
            renderer: (self.options.renderers)(),
        }
    }

    fn platform_event(&mut self, event: PlatformEvent) -> Result<(), UiError> {
        let (window, event) = match event {
            PlatformEvent::Window { window, event } => (window, event),
            PlatformEvent::Clipboard { .. }
            | PlatformEvent::ClipboardOffered { .. }
            | PlatformEvent::ActivationToken { .. }
            | PlatformEvent::Wake => return Ok(()),
        };
        let Some(i) = self.windows.iter().position(|w| w.id == window) else {
            return Ok(());
        };
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
                    self.invalidate_window(i);
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
                w.active = focused;
                if !focused {
                    let mut pointer = std::mem::take(&mut w.pointer);
                    let mut route = route(&mut self.tree, w, now, &mut effects);
                    pointer.revoke(&mut route);
                    w.pointer = pointer;
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
            WindowEvent::PopupDone | WindowEvent::Presented { .. } => return Ok(()),
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
        let handled =
            route(&mut self.tree, w, now, effects).dispatch(&path, &Event::Key(key, time));
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
        if !effects.animate.is_empty() {
            w.animate.extend(effects.animate);
            w.invalidate();
            self.system.request_frame(w.id);
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
    /// is reported: one the compositor sized or scaled out of reach, or that the window system refused, waits for its
    /// next configure; one whose content failed tries again when the application next changes it. Only a lost
    /// connection ends the application.
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
                    w.damage.invalidate();
                    if size_caused(&error) {
                        w.frame_ready = false;
                    } else {
                        w.dirty = false;
                    }
                    if let Some(report) = &mut self.options.on_window_error {
                        report(w.id, &error);
                    }
                }
            }
        }
        self.sync_accessibility();
        Ok(())
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
            w.geometry = placements(&self.tree, root, &result, w.env.direction);

            let semantics = build(&self.tree, root, &w.geometry, w.focus.node());
            w.focusable = semantics.focusable;
            let change = w.focus.sync(&self.tree, &semantics.scopes, &w.focusable);
            let refocused = change.is_some();
            focus_effects = Self::focus_events(&mut self.tree, w, change, &mut self.messages, now);
            let nodes = if refocused {
                build(&self.tree, root, &w.geometry, w.focus.node()).nodes
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
        }
        self.apply(i, focus_effects);
        let w = &mut self.windows[i];

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
                now,
            },
        );
        let damage = w.damage.damage(&painting);
        w.display_list = painting.list;
        w.dirty = false;
        if matches!(&damage, DamageRegion::Rects(r) if r.is_empty()) {
            return Ok(());
        }
        let scale = w.env.scale;
        let id = w
            .renderer
            .render(&RenderRequest {
                list: &w.display_list,
                resources: &PaintResources::default(),
                scale,
                width: physical(w.size.width, scale),
                height: physical(w.size.height, scale),
                damage: &damage,
            })
            .map_err(UiError::Render)?;
        let frame = w.renderer.frame(id).map_err(UiError::Render)?;
        match self
            .system
            .present(w.id, frame, w.ack)
            .map_err(UiError::Platform)?
        {
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

/// Whether the window's size or scale, or the window system, failed the frame rather than the content.
fn size_caused(error: &UiError) -> bool {
    matches!(
        error,
        UiError::Platform(_)
            | UiError::Layout(LayoutError::InvalidConstraint)
            | UiError::Render(
                RenderError::ResourceLimit | RenderError::InvalidSize | RenderError::InvalidScale
            )
    )
}

/// The buffer size of a logical length, rounded to the nearest pixel with halves up, as the window system sizes the
/// buffer it accepts for a configure at a fractional scale.
fn physical(logical: f64, scale: f64) -> u32 {
    (logical * scale).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

fn route<'a, M: 'static>(
    tree: &'a mut Tree<M>,
    w: &'a WindowState<M>,
    now: Timestamp,
    effects: &'a mut Effects<M>,
) -> Route<'a, M> {
    Route {
        tree,
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

#[cfg(test)]
mod tests;
