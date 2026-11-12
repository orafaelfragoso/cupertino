//! Widgets and an environment for the crate's own tests.

use crate::{AccessibilityAdapter, AccessibilityUpdate, ActionRequest, Clock, RuntimeOptions};
use crate::{
    Change, Element, Environment, Event, EventCx, MeasureCx, PaintCx, Propagation, Role,
    SemanticsCx, Widget,
};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_platform::PointerEvent;
use cupertino_ui_platform::{
    ActivationRequest, Capabilities, ClipboardData, ClipboardKind, ClipboardToken, ConfigureId,
    CursorShape, DragRequest, DragToken, DropAction, DropAnswer, DropToken, PlatformError,
    PlatformEvent, PopupSpec, PresentOutcome, Timestamp, WindowEvent, WindowId, WindowRequest,
    WindowSpec, WindowSystem,
};
use cupertino_ui_render::{
    CpuFrame, DamageRegion, Paint, PaintOp, PixelFormat, RenderError, RenderFrameId, RenderRequest,
    Shape, UiRenderer,
};
use cupertino_ui_text::{
    ChangeCause, ContentHints, ContentPurpose, FontIndex, Surrounding, TextInputState, TextSystem,
};
use cupertino_ui_theme::{
    Appearance, ColorTokens, ControlMetrics, ControlSize, FocusRing, Motion, ThemePreferences,
    ThemeTokens, TypeRole, TypeScale,
};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::Waker;
use std::time::Duration;

pub(crate) fn env() -> Environment {
    let role = TypeRole {
        size: 13.0,
        weight: 400,
    };
    Environment {
        theme: Arc::new(ThemeTokens {
            preferences: ThemePreferences {
                appearance: Appearance::Light,
                reduce_transparency: false,
                reduce_motion: false,
            },
            colors: ColorTokens::default(),
            metrics: ControlMetrics {
                size: ControlSize::Regular,
                compact: false,
                button_height: 20.0,
                button_inset: 10.0,
                button_radius: 5.0,
                field_height: 20.0,
                field_inset: 6.0,
                field_radius: 5.0,
                switch: Size::new(38.0, 22.0),
                switch_thumb: 18.0,
                switch_thumb_inset: 2.0,
                min_target: Size::new(24.0, 24.0),
            },
            type_scale: TypeScale {
                family: "Inter".into(),
                body: role,
                caption: role,
                section: role,
                title: role,
            },
            focus: FocusRing {
                width: 2.0,
                outset: 2.0,
                radius_extra: 3.0,
            },
            motion: Motion {
                reduce: false,
                caret_phase_ms: 500,
            },
        }),
        text: Arc::new(TextSystem::new(FontIndex::new(), "Inter")),
        direction: LayoutDirection::Ltr,
        scale: 1.0,
        icons: Arc::new(crate::PlaceholderIcons::default()),
    }
}

pub(crate) fn spec(kind: ContainerKind) -> LayoutSpec {
    LayoutSpec::new(kind)
}

/// A leaf that records the events it saw and reports its label as a change.
pub(crate) struct Leaf {
    pub(crate) label: &'static str,
    pub(crate) role: Option<Role>,
    pub(crate) focusable: bool,
    pub(crate) stop: bool,
    /// Captures the pointer while a button is down.
    pub(crate) capture: bool,
    /// A protected text field: it marks its value protected and, when focused, asks for the input method with its text.
    pub(crate) secret: bool,
    /// A key starts an animation that repaints and asks for the next frame on every frame.
    pub(crate) animated: bool,
    /// A key asks to be woken this many nanoseconds later.
    pub(crate) wake_after: Option<u64>,
    /// Asks for the focus when its scope opens.
    pub(crate) initial: bool,
    /// Takes Escape before the focus path.
    pub(crate) dismissible: bool,
    /// Draws a square whose shade follows its focus and the animation frames it has seen, so those changes show.
    pub(crate) drawn: bool,
}

pub(crate) fn leaf(label: &'static str) -> Leaf {
    Leaf {
        label,
        role: Some(Role::Button),
        focusable: true,
        stop: false,
        capture: false,
        secret: false,
        animated: false,
        wake_after: None,
        initial: false,
        dismissible: false,
        drawn: false,
    }
}

impl<M: From<String> + 'static> Widget<M> for Leaf {
    /// How many updates (and, when drawn, animation frames) the node has seen, so tests can tell a kept node from a new
    /// one.
    type State = u32;

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) -> u32 {
        0
    }

    fn update(&self, old: &Self, state: &mut u32) -> Change {
        *state += 1;
        if old.label == self.label {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &u32, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(10.0, 10.0),
            baseline: None,
        }
    }

    fn event(&self, state: &mut u32, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let name = match event {
            Event::Pointer(PointerEvent::Enter { .. }, _) => "enter",
            Event::Pointer(PointerEvent::Leave, _) => "leave",
            Event::Pointer(PointerEvent::Motion { .. }, _) => "motion",
            Event::Pointer(PointerEvent::Button { pressed, .. }, _) => {
                if self.capture && *pressed {
                    cx.capture_pointer();
                } else if self.capture {
                    cx.release_pointer();
                }
                if *pressed {
                    "press"
                } else {
                    "release"
                }
            }
            Event::Pointer(PointerEvent::Scroll { .. }, _) => "scroll",
            Event::Key(..) => {
                if self.animated {
                    cx.request_animation_frame();
                    cx.request_paint();
                }
                if let Some(after) = self.wake_after {
                    cx.wake_at(Timestamp(cx.now().0 + after));
                }
                "key"
            }
            Event::Wake(_) => "wake",
            Event::AnimationFrame(_) => {
                if self.drawn {
                    *state += 1;
                }
                cx.request_animation_frame();
                cx.request_paint();
                "frame"
            }
            Event::Focus { gained: true, .. } => {
                if self.secret {
                    cx.set_text_input(Some(TextInputState {
                        surrounding: Some(Surrounding {
                            text: "hunter2".into(),
                            cursor: 7,
                            anchor: 7,
                        }),
                        purpose: ContentPurpose::Password,
                        hints: ContentHints::default(),
                        cause: ChangeCause::Other,
                        cursor_rect: None,
                        reset: false,
                    }));
                }
                "focus"
            }
            Event::Focus { .. } => "blur",
            Event::Cancel => "cancel",
            Event::Dismiss => "dismiss",
            _ => "other",
        };
        cx.emit(M::from(format!("{}:{:?}:{name}", self.label, cx.phase())));
        if self.stop {
            Propagation::Stop
        } else {
            Propagation::Continue
        }
    }

    fn paint(&self, state: &u32, cx: &mut PaintCx<'_>) {
        if self.drawn {
            let shade = (*state % 8) as f32 / 8.0 + if cx.is_focused() { 0.05 } else { 0.0 };
            let size = cx.size();
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(cupertino_ui_theme::Color::new(shade, shade, shade, 1.0)),
            });
        }
    }

    fn semantics(&self, _: &u32, cx: &mut SemanticsCx<'_>) {
        if let Some(role) = self.role {
            cx.role(role);
            cx.name(self.label);
        }
        if self.focusable {
            cx.focusable();
        }
        if self.secret {
            cx.protected();
        }
        if self.initial {
            cx.initial_focus();
        }
        if self.dismissible {
            cx.dismissible();
        }
    }
}

/// A container with no semantics of its own.
pub(crate) struct Group<M> {
    pub(crate) children: Vec<Element<M>>,
    pub(crate) scope: bool,
    pub(crate) row: bool,
    /// A roving group: its items share one Tab stop.
    pub(crate) roving: bool,
}

pub(crate) fn group<M>(children: Vec<Element<M>>) -> Group<M> {
    Group {
        children,
        scope: false,
        row: false,
        roving: false,
    }
}

/// Children side by side, each at its measured size.
pub(crate) fn row<M>(children: Vec<Element<M>>) -> Group<M> {
    Group {
        row: true,
        ..group(children)
    }
}

impl<M: From<String> + 'static> Widget<M> for Group<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        if self.row {
            spec(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Start,
            })
        } else {
            spec(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if matches!(event, Event::Pointer(..) | Event::Key(..)) {
            cx.emit(M::from(format!("group:{:?}", cx.phase())));
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.roving {
            cx.roving_group();
        }
        if self.scope {
            cx.role(Role::Dialog);
            cx.focus_scope();
        }
    }
}

thread_local! {
    /// How many drops the window systems of this thread have finished, for a model to read as it updates.
    pub(crate) static DROPS_FINISHED: Cell<usize> = const { Cell::new(0) };
}

/// A window system in memory: tests queue its events and read what the runtime asked of it.
#[derive(Default)]
pub(crate) struct FakeSystem {
    pub(crate) queue: VecDeque<PlatformEvent>,
    /// The timeout of every wait, in order.
    pub(crate) waits: Rc<RefCell<Vec<Option<Duration>>>>,
    /// A wait that may block (not a step's zero-timeout poll) with nothing queued asks the first window to close, so
    /// `run` returns.
    pub(crate) close_on_wait: bool,
    pub(crate) created: Vec<(WindowId, WindowSpec)>,
    pub(crate) destroyed: Vec<WindowId>,
    pub(crate) requests: Vec<(WindowId, WindowRequest)>,
    pub(crate) presented: Vec<(WindowId, Option<ConfigureId>)>,
    /// The id of every frame presented, in order.
    pub(crate) frame_ids: Vec<cupertino_ui_render::RenderFrameId>,
    pub(crate) frames_requested: usize,
    /// The next present is refused, as when no buffer is free; the window then waits like the Wayland system's does.
    pub(crate) refuse_next: bool,
    stalled: Option<WindowId>,
    frame_wanted: bool,
    pub(crate) text_input: Vec<Option<TextInputState>>,
    pub(crate) clipboard: Option<ClipboardData>,
    /// Offers the desktop's effects.
    pub(crate) effects: bool,
    /// Every activation token asked for: the window and the application named.
    pub(crate) activations: Vec<(WindowId, Option<String>)>,
    /// Starts drags; without it every drag is refused.
    pub(crate) drag: bool,
    /// Every drag started, with its window; the token is its place in the list, from 1.
    pub(crate) drags: Vec<(WindowId, DragRequest)>,
    /// Every answer to a drag over a window, in order.
    pub(crate) drop_answers: Vec<(DropToken, DropAnswer)>,
    /// What a drop read gives; every read is answered with it at the next dispatch, `None` failing.
    pub(crate) drop_data: Option<ClipboardData>,
    /// Every drop read, with the type asked for.
    pub(crate) drop_reads: Vec<(DropToken, String)>,
    /// How each drop ended, in order.
    pub(crate) drops_finished: Vec<(DropToken, Option<DropAction>)>,
    /// Opens popups; without it every popup is refused.
    pub(crate) popups: bool,
    /// Has had no input event yet: a popup grab is refused, as a Wayland client without a serial refuses it.
    pub(crate) no_input: bool,
    /// Publishes every window's semantics, as while desktop control watches them.
    pub(crate) semantics: bool,
    /// The latest semantics published of each window.
    pub(crate) published: Vec<(WindowId, cupertino_semantics::Snapshot)>,
    /// The actions desktop control asks for, handed to the runtime at its next turn.
    pub(crate) semantic_calls: Vec<(WindowId, cupertino_semantics::ActionCall)>,
    /// The actions come with a grab of their own, as from a compositor that hands one out: a popup of the action's
    /// window may grab while the action is handled, input or not.
    pub(crate) action_grabs: bool,
    /// The actions being handled that came with a grab: their window and call.
    granted: Vec<(WindowId, u64)>,
    /// How each action asked for ended, in order.
    pub(crate) semantic_outcomes: Vec<(WindowId, u64, cupertino_semantics::Outcome)>,
    /// Every popup opened: its id, its parent and where it was placed.
    pub(crate) popups_created: Vec<(WindowId, WindowId, PopupSpec)>,
    /// Shows raised layers ([`WindowSystem::present_layer`]).
    pub(crate) layers: bool,
    /// Every raised layer presented, in order: the window, and the layer's height in points, frame size and damage,
    /// or `None` for one hidden.
    pub(crate) layer_presents: Vec<(WindowId, Option<LayerPresent>)>,
    /// The next layer present is refused, as when no buffer is free.
    pub(crate) refuse_next_layer: bool,
    /// Shows a window as one colour without a frame; without it every solid present is refused.
    pub(crate) solid: bool,
    /// Every solid present: the window, its colour and the configure it acknowledged.
    pub(crate) solids: Vec<(WindowId, cupertino_ui_theme::Color, Option<ConfigureId>)>,
    /// The activated state the compositor states for every window ([`WindowSystem::activated`]); `None` where only
    /// the keyboard tells.
    pub(crate) activated: Option<bool>,
}

/// A raised layer as [`FakeSystem`] saw it presented.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayerPresent {
    pub(crate) height: u32,
    pub(crate) size: (u32, u32),
    pub(crate) damage: Vec<cupertino_ui_render::PixelRect>,
    pub(crate) pixels: Vec<u8>,
}

impl FakeSystem {
    pub(crate) fn send(&mut self, window: WindowId, event: WindowEvent) {
        self.queue
            .push_back(PlatformEvent::Window { window, event });
    }

    /// An id no window or popup has had.
    fn next_id(&self) -> WindowId {
        WindowId((self.created.len() + self.popups_created.len()) as u32 + 1)
    }

    /// The window `window` hangs from: itself, or the window its popup chain opened from.
    fn root(&self, mut window: WindowId) -> WindowId {
        while let Some((_, parent, _)) = self.popups_created.iter().find(|(id, ..)| *id == window) {
            window = *parent;
        }
        window
    }

    /// Whether an action being handled on the window `parent` hangs from came with a grab.
    fn action_grab(&self, parent: WindowId) -> bool {
        let root = self.root(parent);
        self.granted.iter().any(|(w, _)| self.root(*w) == root)
    }

    /// A buffer comes free: a stalled window gets a frame only if the runtime asked for one while it waited.
    pub(crate) fn release(&mut self, window: WindowId) {
        if self.stalled.take() == Some(window) && std::mem::take(&mut self.frame_wanted) {
            self.send(window, WindowEvent::Frame(Timestamp(0)));
        }
    }
}

impl WindowSystem for FakeSystem {
    fn activated(&self, _: WindowId) -> Option<bool> {
        self.activated
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            app_effects: self.effects,
            raised_layer: self.layers,
            ..Capabilities::default()
        }
    }

    fn can_grab(&self, parent: WindowId) -> bool {
        !self.no_input || self.action_grab(parent)
    }

    fn semantics_active(&self, _: WindowId) -> bool {
        self.semantics
    }

    fn publish_semantics(&mut self, window: WindowId, snapshot: &cupertino_semantics::Snapshot) {
        self.published.retain(|(w, _)| *w != window);
        self.published.push((window, snapshot.clone()));
    }

    fn semantic_requests(&mut self) -> Vec<(WindowId, cupertino_semantics::ActionCall)> {
        let calls = std::mem::take(&mut self.semantic_calls);
        if self.action_grabs {
            self.granted
                .extend(calls.iter().map(|(window, call)| (*window, call.id)));
        }
        calls
    }

    fn semantic_finished(
        &mut self,
        window: WindowId,
        call: &cupertino_semantics::ActionCall,
        outcome: cupertino_semantics::Outcome,
        _: u64,
    ) {
        self.semantic_outcomes.push((window, call.id, outcome));
        self.granted.retain(|g| *g != (window, call.id));
    }

    fn create_window(&mut self, spec: &WindowSpec) -> Result<WindowId, PlatformError> {
        let id = self.next_id();
        self.created.push((id, spec.clone()));
        Ok(id)
    }

    fn create_popup(
        &mut self,
        parent: WindowId,
        spec: &PopupSpec,
    ) -> Result<WindowId, PlatformError> {
        if !self.popups {
            return Err(PlatformError::Unsupported);
        }
        if spec.grab && self.no_input && !self.action_grab(parent) {
            return Err(PlatformError::Protocol(
                "a popup grab needs a user input event first".into(),
            ));
        }
        // the protocol ends a client whose grabbing popup opens from a popup without a grab
        let ungrabbed_parent = self
            .popups_created
            .iter()
            .any(|(id, _, placed)| *id == parent && !placed.grab);
        assert!(
            !(spec.grab && ungrabbed_parent),
            "a grabbing popup opened from popup {parent:?}, which holds no grab"
        );
        let id = self.next_id();
        self.popups_created.push((id, parent, spec.clone()));
        Ok(id)
    }

    fn destroy(&mut self, window: WindowId) {
        self.destroyed.push(window);
    }

    fn request(&mut self, window: WindowId, request: WindowRequest) -> Result<(), PlatformError> {
        self.requests.push((window, request));
        Ok(())
    }

    fn set_text_input(&mut self, _: WindowId, state: Option<&TextInputState>) {
        self.text_input.push(state.cloned());
    }

    fn set_cursor(&mut self, _: WindowId, _: Option<CursorShape>) {}

    fn set_clipboard(&mut self, _: ClipboardKind, data: ClipboardData) {
        self.clipboard = Some(data);
    }

    fn read_clipboard(&mut self, _: ClipboardKind, _: &str) -> ClipboardToken {
        ClipboardToken(0)
    }

    fn request_activation_token(
        &mut self,
        window: WindowId,
        app_id: Option<&str>,
    ) -> ActivationRequest {
        self.activations.push((window, app_id.map(str::to_owned)));
        ActivationRequest(self.activations.len() as u64)
    }

    fn start_drag(
        &mut self,
        window: WindowId,
        request: DragRequest,
    ) -> Result<DragToken, PlatformError> {
        if !self.drag {
            return Err(PlatformError::Unsupported);
        }
        self.drags.push((window, request));
        Ok(DragToken(self.drags.len() as u64))
    }

    fn answer_drop(&mut self, token: DropToken, answer: &DropAnswer) -> Result<(), PlatformError> {
        self.drop_answers.push((token, answer.clone()));
        Ok(())
    }

    fn read_drop(&mut self, token: DropToken, mime: &str) -> Result<ClipboardToken, PlatformError> {
        self.drop_reads.push((token, mime.to_owned()));
        let read = ClipboardToken(1000 + self.drop_reads.len() as u64);
        self.queue.push_back(PlatformEvent::Clipboard {
            token: read,
            data: self.drop_data.clone(),
        });
        Ok(read)
    }

    fn finish_drop(&mut self, token: DropToken, action: Option<DropAction>) {
        self.drops_finished.push((token, action));
        DROPS_FINISHED.with(|n| n.set(n.get() + 1));
    }

    fn request_frame(&mut self, window: WindowId) {
        self.frames_requested += 1;
        if self.stalled == Some(window) {
            self.frame_wanted = true;
        }
    }

    fn present(
        &mut self,
        window: WindowId,
        frame: &CpuFrame,
        configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        if std::mem::take(&mut self.refuse_next) {
            self.stalled = Some(window);
            return Ok(PresentOutcome::NotReady);
        }
        self.presented.push((window, configure));
        self.frame_ids.push(frame.id);
        Ok(PresentOutcome::Presented)
    }

    fn present_layer(
        &mut self,
        window: WindowId,
        layer: Option<cupertino_ui_platform::RaisedLayer<'_>>,
    ) -> Result<PresentOutcome, PlatformError> {
        if layer.is_some() && !self.layers {
            return Err(PlatformError::Unsupported);
        }
        if std::mem::take(&mut self.refuse_next_layer) {
            self.stalled = Some(window);
            return Ok(PresentOutcome::NotReady);
        }
        self.layer_presents.push((
            window,
            layer.map(|l| LayerPresent {
                height: l.height,
                size: (l.frame.width, l.frame.height),
                damage: l.frame.damage.clone(),
                pixels: l.frame.pixels.clone(),
            }),
        ));
        Ok(PresentOutcome::Presented)
    }

    fn present_solid(
        &mut self,
        window: WindowId,
        color: cupertino_ui_theme::Color,
        configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        if !self.solid {
            return Err(PlatformError::Unsupported);
        }
        if std::mem::take(&mut self.refuse_next) {
            self.stalled = Some(window);
            return Ok(PresentOutcome::NotReady);
        }
        self.solids.push((window, color, configure));
        Ok(PresentOutcome::Presented)
    }

    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError> {
        self.waits.borrow_mut().push(timeout);
        if self.close_on_wait && timeout != Some(Duration::ZERO) && self.queue.is_empty() {
            if let Some(&(window, _)) = self.created.first() {
                self.queue.push_back(PlatformEvent::Window {
                    window,
                    event: WindowEvent::CloseRequested,
                });
            }
        }
        events.extend(self.queue.drain(..));
        Ok(())
    }

    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }
}

/// The fake renderer's allocation limit, in physical pixels.
pub(crate) const MAX_PIXELS: u64 = 4096 * 4096;

/// A renderer that keeps a blank frame and records the damage of every request.
pub(crate) struct FakeRenderer {
    pub(crate) damage: Rc<RefCell<Vec<DamageRegion>>>,
    /// While set, every request fails as content too large to draw would, such as an over-cap blur.
    broken: Rc<Cell<bool>>,
    frame: CpuFrame,
}

impl UiRenderer for FakeRenderer {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        if u64::from(request.width) * u64::from(request.height) > MAX_PIXELS {
            return Err(RenderError::ResourceLimit);
        }
        if self.broken.get() {
            return Err(RenderError::ResourceLimit);
        }
        self.damage.borrow_mut().push(request.damage.clone());
        self.frame.id = RenderFrameId(self.frame.id.0 + 1);
        self.frame.width = request.width;
        self.frame.height = request.height;
        self.frame.stride = request.width * 4;
        self.frame
            .pixels
            .resize(self.frame.stride as usize * request.height as usize, 0);
        Ok(self.frame.id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        if id == self.frame.id {
            Ok(&self.frame)
        } else {
            Err(RenderError::UnknownFrame(id))
        }
    }
}

/// An adapter whose assistive technology a test switches on and off; it keeps every update.
#[derive(Clone, Default)]
pub(crate) struct FakeAdapter {
    pub(crate) active: Rc<Cell<bool>>,
    pub(crate) updates: Rc<RefCell<Vec<AccessibilityUpdate>>>,
    pub(crate) closed: Rc<RefCell<Vec<WindowId>>>,
    pub(crate) locked: Rc<RefCell<Vec<bool>>>,
    /// Each title it was told and the number of updates it held then.
    pub(crate) titles: Rc<RefCell<Vec<(WindowId, String, usize)>>>,
    /// The actions its assistive technology asks for, handed over at the next drain.
    pub(crate) actions: Rc<RefCell<Vec<ActionRequest>>>,
}

impl AccessibilityAdapter for FakeAdapter {
    fn is_active(&self) -> bool {
        self.active.get()
    }

    fn window_focus(&mut self, _: WindowId, _: bool) {}

    fn update(&mut self, update: &AccessibilityUpdate) {
        self.updates.borrow_mut().push(update.clone());
    }

    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>) {
        out.append(&mut self.actions.borrow_mut());
    }

    fn window_closed(&mut self, window: WindowId) {
        self.closed.borrow_mut().push(window);
    }

    fn session_locked(&mut self, locked: bool) {
        self.locked.borrow_mut().push(locked);
    }

    fn window_title(&mut self, window: WindowId, title: &str) {
        let held = self.updates.borrow().len();
        self.titles
            .borrow_mut()
            .push((window, title.to_owned(), held));
    }
}

#[derive(Default)]
pub(crate) struct FakeClock(pub(crate) AtomicU64);

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.0.load(Ordering::SeqCst))
    }
}

pub(crate) fn options(damage: &Rc<RefCell<Vec<DamageRegion>>>) -> RuntimeOptions {
    options_with(damage, &Rc::default())
}

/// Options whose renderers fail while `broken` is set.
pub(crate) fn options_with(
    damage: &Rc<RefCell<Vec<DamageRegion>>>,
    broken: &Rc<Cell<bool>>,
) -> RuntimeOptions {
    let damage = Rc::clone(damage);
    let broken = Rc::clone(broken);
    RuntimeOptions {
        theme: env().theme,
        text: env().text,
        icons: env().icons,
        direction: LayoutDirection::Ltr,
        node_limit: 1000,
        renderers: Box::new(move || {
            Box::new(FakeRenderer {
                damage: Rc::clone(&damage),
                broken: Rc::clone(&broken),
                frame: CpuFrame {
                    id: RenderFrameId(0),
                    width: 0,
                    height: 0,
                    stride: 0,
                    format: PixelFormat::Rgba8Premultiplied,
                    pixels: Vec::new(),
                    damage: Vec::new(),
                },
            })
        }),
        accessibility: None,
        clock: Arc::new(FakeClock::default()),
        on_window_error: None,
    }
}

/// A scroll container scrolled by `offset` along both axes.
pub(crate) struct Scroller<M> {
    pub(crate) children: Vec<Element<M>>,
    pub(crate) offset: Point,
}

impl<M: From<String> + 'static> Widget<M> for Scroller<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Scroll {
            horizontal: true,
            vertical: true,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn scroll_offset(&self, _: &()) -> Point {
        self.offset
    }
}
