use cupertino_geometry::Point;
use cupertino_geometry::Size;
use cupertino_ui_platform::{
    ActivationRequest, Capabilities, ClipboardData, ClipboardKind, ClipboardToken, Configure,
    ConfigureId, CursorShape, DragRequest, DragToken, DropAction, DropAnswer, DropEvent, DropToken,
    MenuDescription, Mods, PlatformError, PlatformEvent, PointerEvent, PopupSpec, PresentOutcome,
    Timestamp, WindowEvent, WindowId, WindowRequest, WindowSpec, WindowStates, WindowSystem,
    WmCapabilities,
};
use cupertino_ui_render::{CpuFrame, PixelFormat, PixelRect, RenderFrameId};
use cupertino_ui_text::TextInputState;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

/// What a test sees of one frame the runtime presented. The pixels are kept so a test can read them back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedFrame {
    pub id: RenderFrameId,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub pixels: Vec<u8>,
    pub damage: Vec<PixelRect>,
    pub acknowledged: Option<ConfigureId>,
    /// Whether [`HeadlessWindowSystem::show_oldest`] has reported the frame as shown.
    pub shown: bool,
}

#[derive(Debug)]
struct HeadlessWindow {
    spec: WindowSpec,
    parent: Option<WindowId>,
    /// Where a popup was asked to open.
    placement: Option<PopupSpec>,
    requests: Vec<WindowRequest>,
    cursor: Option<CursorShape>,
    text_input: Option<TextInputState>,
    frame_requested: bool,
    unacknowledged: Vec<ConfigureId>,
    frames: Vec<PresentedFrame>,
    /// The window's pixels, kept up to date with each frame's damage while presented frames keep no pixels.
    buffer: Vec<u8>,
    /// Its configures say it is activated while it has the keyboard ([`HeadlessWindowSystem::activate_by_keyboard`]).
    by_keyboard: bool,
    /// The last focus event queued for it said it has the keyboard.
    keyboard: bool,
}

/// A drag the runtime started, as the window system received it.
#[derive(Clone, Debug, PartialEq)]
pub struct StartedDrag {
    pub window: WindowId,
    pub token: DragToken,
    pub request: DragRequest,
}

/// A drag from another application that a test moves over a window, and what the window answered it.
#[derive(Clone, Debug, PartialEq)]
pub struct IncomingDrag {
    pub window: WindowId,
    pub token: DropToken,
    /// The data in each type the drag offers.
    pub data: Vec<ClipboardData>,
    /// What the source allows.
    pub actions: BTreeSet<DropAction>,
    /// Every answer the window gave, oldest first.
    pub answers: Vec<DropAnswer>,
    /// What a drop would do now, chosen as a compositor does from the source's actions and the last answer.
    pub chosen: Option<DropAction>,
    /// Whether it dropped; a drag released where the answer took nothing leaves instead.
    pub dropped: bool,
    /// The types read after the drop.
    pub reads: Vec<String>,
    /// How the window ended the drop: `Some(action)` took it, `None` refused it; unset while it runs.
    pub finished: Option<Option<DropAction>>,
}

/// The action a compositor chooses: the window's preferred one when the source and the window allow it, else the first
/// both allow.
fn choose(source: &BTreeSet<DropAction>, answer: &DropAnswer) -> Option<DropAction> {
    answer.mime.as_ref()?;
    match answer.preferred {
        Some(p) if source.contains(&p) && answer.actions.contains(&p) => Some(p),
        _ => source.intersection(&answer.actions).next().copied(),
    }
}

/// Copies the damaged rows of `frame` into `buffer`, which takes the frame's size (and all of it) when its size
/// changed.
fn copy_damage(buffer: &mut Vec<u8>, frame: &CpuFrame) {
    if buffer.len() != frame.pixels.len() {
        buffer.clear();
        buffer.extend_from_slice(&frame.pixels);
        return;
    }
    let stride = frame.stride as usize;
    for r in &frame.damage {
        let (x0, x1) = (r.x as usize * 4, (r.x + r.width) as usize * 4);
        for y in r.y as usize..(r.y + r.height) as usize {
            let row = y * stride;
            buffer[row + x0..row + x1].copy_from_slice(&frame.pixels[row + x0..row + x1]);
        }
    }
}

/// Whether the runtime was woken from another thread since a test last waited for it.
#[derive(Debug, Default)]
struct Woken {
    woken: Mutex<bool>,
    changed: Condvar,
}

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        *self.woken.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }
}

/// A window system with no display. Windows live in memory, every request is kept for inspection, presented
/// frames are kept until the window is destroyed, and the only events are the ones the test enqueues. Its waker
/// does not dispatch anything; a test waits for it with [`HeadlessWindowSystem::wait_for_wake`].
#[derive(Debug)]
pub struct HeadlessWindowSystem {
    capabilities: Capabilities,
    windows: BTreeMap<WindowId, HeadlessWindow>,
    next_window: u32,
    next_configure: u32,
    next_token: u64,
    queue: VecDeque<PlatformEvent>,
    clipboards: HashMap<ClipboardKind, ClipboardData>,
    drags: Vec<StartedDrag>,
    incoming: Vec<IncomingDrag>,
    /// The reads of drops, which a test driver does not attribute to a focused field.
    drop_reads: HashSet<ClipboardToken>,
    accepting_frames: bool,
    /// Whether each presented frame keeps a copy of all its pixels.
    keep_pixels: bool,
    pub(crate) dispatch_one: bool,
    woken: Arc<Woken>,
}

impl Default for HeadlessWindowSystem {
    fn default() -> Self {
        Self::new(Capabilities::default())
    }
}

impl HeadlessWindowSystem {
    pub fn new(capabilities: Capabilities) -> Self {
        Self {
            capabilities,
            windows: BTreeMap::new(),
            next_window: 1,
            next_configure: 1,
            next_token: 1,
            queue: VecDeque::new(),
            clipboards: HashMap::new(),
            drags: Vec::new(),
            incoming: Vec::new(),
            drop_reads: HashSet::new(),
            accepting_frames: true,
            keep_pixels: true,
            dispatch_one: false,
            woken: Arc::default(),
        }
    }

    /// Blocks until the runtime's waker is woken (by a task finishing on another thread), or `timeout` passes, which
    /// only guards against a hang. A wake that came since the last wait counts, so none is lost between draining the
    /// runtime and waiting; `true` when woken, and the wake is then used up.
    pub fn wait_for_wake(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut woken = self
            .woken
            .woken
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while !*woken {
            let Some(left) = end
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
            else {
                return false;
            };
            woken = self
                .woken
                .changed
                .wait_timeout(woken, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *woken = false;
        true
    }

    fn window(&self, id: WindowId) -> Result<&HeadlessWindow, PlatformError> {
        self.windows
            .get(&id)
            .ok_or(PlatformError::UnknownWindow(id))
    }

    fn window_mut(&mut self, id: WindowId) -> Result<&mut HeadlessWindow, PlatformError> {
        self.windows
            .get_mut(&id)
            .ok_or(PlatformError::UnknownWindow(id))
    }

    fn open(&mut self, spec: WindowSpec, parent: Option<WindowId>) -> WindowId {
        let id = WindowId(self.next_window);
        self.next_window += 1;
        self.windows.insert(
            id,
            HeadlessWindow {
                spec,
                parent,
                placement: None,
                requests: Vec::new(),
                cursor: Some(CursorShape::Default),
                text_input: None,
                frame_requested: false,
                unacknowledged: Vec::new(),
                frames: Vec::new(),
                buffer: Vec::new(),
                by_keyboard: false,
                keyboard: false,
            },
        );
        id
    }

    /// Queues an event for the next [`dispatch`](WindowSystem::dispatch). A window event for a window that does
    /// not exist is an error, so a test cannot deliver input to a stale ID.
    pub fn enqueue(&mut self, event: PlatformEvent) -> Result<(), PlatformError> {
        if let PlatformEvent::Window { window, event } = &event {
            let w = self.window_mut(*window)?;
            if let WindowEvent::Focus(focused) = event {
                w.keyboard = *focused;
            }
        }
        self.queue.push_back(event);
        Ok(())
    }

    pub fn enqueue_window(
        &mut self,
        window: WindowId,
        event: WindowEvent,
    ) -> Result<(), PlatformError> {
        self.enqueue(PlatformEvent::Window { window, event })
    }

    /// Makes `window` a surface whose role's configure has no activated state, such as a desktop shell's layer
    /// surface: from now its configures say it is activated while it has the keyboard, between a queued
    /// `WindowEvent::Focus(true)` and `Focus(false)`, whatever states the test passes. It models how the Wayland
    /// window system reports such a window, except that a popup of the window holding the keyboard does not count.
    pub fn activate_by_keyboard(&mut self, window: WindowId) -> Result<(), PlatformError> {
        self.window_mut(window)?.by_keyboard = true;
        Ok(())
    }

    /// Queues a configure and returns its ID, which the runtime must acknowledge with a presented frame.
    pub fn configure(
        &mut self,
        window: WindowId,
        size: Option<Size>,
        mut states: WindowStates,
    ) -> Result<ConfigureId, PlatformError> {
        let w = self.window(window)?;
        if w.by_keyboard {
            states.activated = w.keyboard;
        }
        let id = ConfigureId(self.next_configure);
        let wm_capabilities = WmCapabilities {
            window_menu: false,
            maximize: true,
            fullscreen: true,
            minimize: true,
        };
        self.window_mut(window)?.unacknowledged.push(id);
        self.next_configure += 1;
        self.enqueue_window(
            window,
            WindowEvent::Configure(Configure {
                id,
                size,
                states,
                bounds: None,
                server_decorations: self.capabilities.server_decorations,
                wm_capabilities,
            }),
        )?;
        Ok(id)
    }

    /// Queues [`WindowEvent::Frame`] for a window that asked for one. The request is consumed, so a second call
    /// without a new request is an error instead of an invented frame callback.
    pub fn frame_ready(&mut self, window: WindowId, at: Timestamp) -> Result<(), PlatformError> {
        let w = self.window_mut(window)?;
        if !std::mem::take(&mut w.frame_requested) {
            return Err(PlatformError::Protocol(format!(
                "window {} has no frame request",
                window.0
            )));
        }
        self.enqueue_window(window, WindowEvent::Frame(at))
    }

    /// Reports the oldest frame the window presented and has not yet shown as shown at `at`, which lets a test hold
    /// a frame in flight for as long as it needs.
    pub fn show_oldest(
        &mut self,
        window: WindowId,
        at: Timestamp,
    ) -> Result<RenderFrameId, PlatformError> {
        let frame = self
            .window_mut(window)?
            .frames
            .iter_mut()
            .find(|f| !f.shown)
            .ok_or_else(|| PlatformError::Protocol("no presented frame is waiting".into()))?;
        frame.shown = true;
        let id = frame.id;
        self.enqueue_window(
            window,
            WindowEvent::Presented {
                frame: id,
                time: at,
            },
        )?;
        Ok(id)
    }

    /// While false, [`present`](WindowSystem::present) answers [`PresentOutcome::NotReady`], as a compositor does
    /// when it holds every buffer.
    pub fn set_accepting_frames(&mut self, accepting: bool) {
        self.accepting_frames = accepting;
    }

    /// While false, a presented frame keeps no pixels (its `pixels` are empty): only the damage is copied, into one
    /// buffer per window, as a client copies a frame into its shared buffer. A timing run uses it so the copy of a
    /// whole window per frame, which only a test makes, does not count.
    pub fn set_keep_pixels(&mut self, keep: bool) {
        self.keep_pixels = keep;
    }

    pub fn windows(&self) -> Vec<WindowId> {
        self.windows.keys().copied().collect()
    }

    pub fn spec(&self, window: WindowId) -> Result<&WindowSpec, PlatformError> {
        Ok(&self.window(window)?.spec)
    }

    pub fn parent(&self, window: WindowId) -> Result<Option<WindowId>, PlatformError> {
        Ok(self.window(window)?.parent)
    }

    pub fn placement(&self, window: WindowId) -> Result<Option<&PopupSpec>, PlatformError> {
        Ok(self.window(window)?.placement.as_ref())
    }

    pub fn requests(&self, window: WindowId) -> Result<&[WindowRequest], PlatformError> {
        Ok(&self.window(window)?.requests)
    }

    /// The menus the window publishes to the desktop's menu bar now; an item is chosen with
    /// [`WindowEvent::MenuActivated`].
    pub fn menus(&self, window: WindowId) -> Result<Option<&MenuDescription>, PlatformError> {
        let last = self
            .window(window)?
            .requests
            .iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Menus(menus) => Some(menus.as_ref()),
                _ => None,
            });
        Ok(last.flatten())
    }

    pub fn cursor(&self, window: WindowId) -> Result<Option<CursorShape>, PlatformError> {
        Ok(self.window(window)?.cursor)
    }

    pub fn text_input(&self, window: WindowId) -> Result<Option<&TextInputState>, PlatformError> {
        Ok(self.window(window)?.text_input.as_ref())
    }

    pub fn frame_requested(&self, window: WindowId) -> Result<bool, PlatformError> {
        Ok(self.window(window)?.frame_requested)
    }

    pub fn unacknowledged(&self, window: WindowId) -> Result<&[ConfigureId], PlatformError> {
        Ok(&self.window(window)?.unacknowledged)
    }

    pub fn frames(&self, window: WindowId) -> Result<&[PresentedFrame], PlatformError> {
        Ok(&self.window(window)?.frames)
    }

    pub fn clipboard(&self, kind: ClipboardKind) -> Option<&ClipboardData> {
        self.clipboards.get(&kind)
    }

    pub(crate) fn next_event(&self) -> Option<&PlatformEvent> {
        self.queue.front()
    }

    /// The drags started so far, oldest first. The test ends each by enqueueing its [`PlatformEvent::Drag`] steps.
    pub fn drags(&self) -> &[StartedDrag] {
        &self.drags
    }

    /// Starts a drag from another application over `window` at `position`, offering `data` with the `actions` its
    /// source allows, as a compositor's enter. Its steps follow with [`Self::drag_move`], [`Self::drag_leave`] and
    /// [`Self::drag_drop`]; what the window answers is kept in [`Self::incoming_drag`].
    pub fn drag_enter(
        &mut self,
        window: WindowId,
        position: Point,
        data: Vec<ClipboardData>,
        actions: BTreeSet<DropAction>,
        mods: Mods,
    ) -> Result<DropToken, PlatformError> {
        self.window(window)?;
        let token = DropToken(self.next_token);
        self.next_token += 1;
        let mimes = data.iter().map(|d| d.mime.clone()).collect();
        self.incoming.push(IncomingDrag {
            window,
            token,
            data,
            actions: actions.clone(),
            answers: Vec::new(),
            chosen: None,
            dropped: false,
            reads: Vec::new(),
            finished: None,
        });
        self.enqueue_window(
            window,
            WindowEvent::Drop(DropEvent::Enter {
                token,
                position,
                mimes,
                actions,
                mods,
            }),
        )?;
        Ok(token)
    }

    /// The drag over `window` moves to `position` with `mods` held.
    pub fn drag_move(
        &mut self,
        window: WindowId,
        position: Point,
        mods: Mods,
    ) -> Result<(), PlatformError> {
        self.enqueue_window(
            window,
            WindowEvent::Drop(DropEvent::Motion { position, mods }),
        )
    }

    /// The drag leaves `window`.
    pub fn drag_leave(&mut self, window: WindowId) -> Result<(), PlatformError> {
        self.enqueue_window(window, WindowEvent::Drop(DropEvent::Leave))
    }

    /// The drag over `window` is released. As a compositor does, a drag whose last answer took a type and an action
    /// the source allows drops; any other leaves. Whether it dropped.
    pub fn drag_drop(&mut self, window: WindowId) -> Result<bool, PlatformError> {
        self.window(window)?;
        let drag = self
            .incoming
            .iter_mut()
            .rev()
            .find(|d| d.window == window && !d.dropped && d.finished.is_none())
            .ok_or_else(|| PlatformError::Protocol("no drag over the window".into()))?;
        let drops = drag.chosen.is_some();
        drag.dropped = drops;
        if !drops {
            drag.finished = Some(None);
        }
        let event = if drops {
            DropEvent::Drop
        } else {
            DropEvent::Leave
        };
        self.enqueue_window(window, WindowEvent::Drop(event))?;
        Ok(drops)
    }

    /// The drag `token` names, with what the window answered it.
    pub fn incoming_drag(&self, token: DropToken) -> Option<&IncomingDrag> {
        self.incoming.iter().find(|d| d.token == token)
    }

    /// Whether `token` reads a drop.
    pub(crate) fn reads_drop(&self, token: ClipboardToken) -> bool {
        self.drop_reads.contains(&token)
    }

    /// Events queued and not yet dispatched.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }
}

impl WindowSystem for HeadlessWindowSystem {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn create_window(&mut self, spec: &WindowSpec) -> Result<WindowId, PlatformError> {
        Ok(self.open(spec.clone(), None))
    }

    fn create_popup(
        &mut self,
        parent: WindowId,
        spec: &PopupSpec,
    ) -> Result<WindowId, PlatformError> {
        self.window(parent)?;
        let window = WindowSpec {
            title: String::new(),
            app_id: self.window(parent)?.spec.app_id.clone(),
            size: spec.size,
            min_size: None,
            max_size: None,
            decorations: cupertino_ui_platform::DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        };
        let id = self.open(window, Some(parent));
        self.window_mut(id)?.placement = Some(spec.clone());
        Ok(id)
    }

    /// Destroying a window destroys its popups, as a compositor does, and drops their queued events.
    fn destroy(&mut self, window: WindowId) {
        let mut gone = vec![window];
        while let Some(next) = self
            .windows
            .iter()
            .find(|(id, w)| !gone.contains(id) && w.parent.is_some_and(|p| gone.contains(&p)))
            .map(|(id, _)| *id)
        {
            gone.push(next);
        }
        for id in &gone {
            self.windows.remove(id);
        }
        self.queue.retain(
            |event| !matches!(event, PlatformEvent::Window { window: w, .. } if gone.contains(w)),
        );
    }

    fn request(&mut self, window: WindowId, request: WindowRequest) -> Result<(), PlatformError> {
        self.window_mut(window)?.requests.push(request);
        Ok(())
    }

    fn set_text_input(&mut self, window: WindowId, state: Option<&TextInputState>) {
        if let Some(w) = self.windows.get_mut(&window) {
            w.text_input = state.cloned();
        }
    }

    fn set_cursor(&mut self, window: WindowId, shape: Option<CursorShape>) {
        if let Some(w) = self.windows.get_mut(&window) {
            w.cursor = shape;
        }
    }

    fn set_clipboard(&mut self, kind: ClipboardKind, data: ClipboardData) {
        self.clipboards.insert(kind, data);
    }

    /// With [`Capabilities::drag_and_drop`], keeps the drag and takes the pointer from the window with a leave, as a
    /// compositor does; without it the drag is refused.
    fn start_drag(
        &mut self,
        window: WindowId,
        request: DragRequest,
    ) -> Result<DragToken, PlatformError> {
        self.window(window)?;
        if !self.capabilities.drag_and_drop {
            return Err(PlatformError::Unsupported);
        }
        let token = DragToken(self.next_token);
        self.next_token += 1;
        self.drags.push(StartedDrag {
            window,
            token,
            request,
        });
        self.queue.push_back(PlatformEvent::Window {
            window,
            event: WindowEvent::Pointer(PointerEvent::Leave, Timestamp::default()),
        });
        Ok(token)
    }

    /// Keeps the answer and, when the choice of action changes, tells it as a compositor does.
    fn answer_drop(&mut self, token: DropToken, answer: &DropAnswer) -> Result<(), PlatformError> {
        if !self.capabilities.drag_and_drop {
            return Err(PlatformError::Unsupported);
        }
        let drag = self
            .incoming
            .iter_mut()
            .find(|d| d.token == token && !d.dropped && d.finished.is_none())
            .ok_or_else(|| PlatformError::Protocol("no such drag".into()))?;
        drag.answers.push(answer.clone());
        let chosen = choose(&drag.actions, answer);
        if chosen != drag.chosen {
            drag.chosen = chosen;
            let window = drag.window;
            self.enqueue_window(window, WindowEvent::Drop(DropEvent::Action(chosen)))?;
        }
        Ok(())
    }

    /// Answers with the drag's data in `mime` with the next dispatch.
    fn read_drop(&mut self, token: DropToken, mime: &str) -> Result<ClipboardToken, PlatformError> {
        let drag = self
            .incoming
            .iter_mut()
            .find(|d| d.token == token && d.dropped && d.finished.is_none())
            .ok_or_else(|| PlatformError::Protocol("no such drop".into()))?;
        drag.reads.push(mime.to_owned());
        let data = drag.data.iter().find(|d| d.mime == mime).cloned();
        let read = ClipboardToken(self.next_token);
        self.next_token += 1;
        self.drop_reads.insert(read);
        self.queue
            .push_back(PlatformEvent::Clipboard { token: read, data });
        Ok(read)
    }

    fn finish_drop(&mut self, token: DropToken, action: Option<DropAction>) {
        if let Some(drag) = self
            .incoming
            .iter_mut()
            .find(|d| d.token == token && d.finished.is_none())
        {
            drag.finished = Some(action);
        }
    }

    fn read_clipboard(&mut self, kind: ClipboardKind, mime: &str) -> ClipboardToken {
        let token = ClipboardToken(self.next_token);
        self.next_token += 1;
        let data = self
            .clipboards
            .get(&kind)
            .filter(|d| d.mime == mime)
            .cloned();
        self.queue
            .push_back(PlatformEvent::Clipboard { token, data });
        token
    }

    /// Answers at once: `headless-activation-<n>`, with ` for <app_id>` when one was named; `None` for a window it
    /// does not have.
    fn request_activation_token(
        &mut self,
        window: WindowId,
        app_id: Option<&str>,
    ) -> ActivationRequest {
        let request = ActivationRequest(self.next_token);
        self.next_token += 1;
        let token = self.windows.contains_key(&window).then(|| match app_id {
            Some(app_id) => format!("headless-activation-{} for {app_id}", request.0),
            None => format!("headless-activation-{}", request.0),
        });
        self.queue
            .push_back(PlatformEvent::ActivationToken { request, token });
        request
    }

    fn request_frame(&mut self, window: WindowId) {
        if let Some(w) = self.windows.get_mut(&window) {
            w.frame_requested = true;
        }
    }

    fn present(
        &mut self,
        window: WindowId,
        frame: &CpuFrame,
        configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        let (accepting, keep) = (self.accepting_frames, self.keep_pixels);
        let w = self.window_mut(window)?;
        if !accepting {
            return Ok(PresentOutcome::NotReady);
        }
        if let Some(id) = configure {
            let Some(at) = w.unacknowledged.iter().position(|c| *c == id) else {
                return Err(PlatformError::Protocol(format!(
                    "configure {} is not waiting for an acknowledgement",
                    id.0
                )));
            };
            // Acknowledging a configure also settles every older one.
            w.unacknowledged.drain(..=at);
        }
        let pixels = if keep {
            frame.pixels.clone()
        } else {
            copy_damage(&mut w.buffer, frame);
            Vec::new()
        };
        w.frames.push(PresentedFrame {
            id: frame.id,
            width: frame.width,
            height: frame.height,
            format: frame.format,
            pixels,
            damage: frame.damage.clone(),
            acknowledged: configure,
            shown: false,
        });
        Ok(PresentOutcome::Presented)
    }

    /// Never blocks: it appends what the test enqueued and returns, whatever `timeout` says.
    fn dispatch(
        &mut self,
        _timeout: Option<Duration>,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError> {
        let count = if self.dispatch_one {
            self.queue.len().min(1)
        } else {
            self.queue.len()
        };
        events.extend(self.queue.drain(..count));
        Ok(())
    }

    fn waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.woken))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::Size;
    use cupertino_ui_platform::DecorationMode;

    fn spec() -> WindowSpec {
        WindowSpec {
            title: "Counter".into(),
            app_id: "test.counter".into(),
            size: Size {
                width: 320.0,
                height: 240.0,
            },
            min_size: None,
            max_size: None,
            decorations: DecorationMode::Client,
            foreign_parent: None,
            input_region: None,
        }
    }

    fn frame(id: u64) -> CpuFrame {
        CpuFrame {
            id: RenderFrameId(id),
            width: 2,
            height: 1,
            stride: 8,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: vec![1, 2, 3, 4, 5, 6, 7, 8],
            damage: vec![PixelRect {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            }],
        }
    }

    fn drain(ws: &mut HeadlessWindowSystem) -> Vec<PlatformEvent> {
        let mut events = Vec::new();
        ws.dispatch(None, &mut events).unwrap();
        events
    }

    #[test]
    fn a_window_activated_by_the_keyboard_is_configured_activated_while_it_has_it() {
        let mut ws = HeadlessWindowSystem::default();
        let window = ws.create_window(&spec()).unwrap();
        ws.activate_by_keyboard(window).unwrap();
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        let activated = |ws: &mut HeadlessWindowSystem| {
            drain(ws).into_iter().find_map(|e| match e {
                PlatformEvent::Window {
                    event: WindowEvent::Configure(c),
                    ..
                } => Some(c.states.activated),
                _ => None,
            })
        };
        ws.configure(window, None, active).unwrap();
        assert_eq!(activated(&mut ws), Some(false), "no keyboard yet");
        ws.enqueue_window(window, WindowEvent::Focus(true)).unwrap();
        ws.configure(window, None, WindowStates::default()).unwrap();
        assert_eq!(activated(&mut ws), Some(true));
        ws.enqueue_window(window, WindowEvent::Focus(false))
            .unwrap();
        ws.configure(window, None, active).unwrap();
        assert_eq!(activated(&mut ws), Some(false));
    }

    #[test]
    fn a_configure_is_acknowledged_once_by_a_presented_frame() {
        let mut ws = HeadlessWindowSystem::default();
        let window = ws.create_window(&spec()).unwrap();
        let first = ws.configure(window, None, WindowStates::default()).unwrap();
        let second = ws.configure(window, None, WindowStates::default()).unwrap();
        assert_eq!((first, second), (ConfigureId(1), ConfigureId(2)));
        assert_eq!(ws.unacknowledged(window).unwrap(), &[first, second]);

        assert_eq!(
            ws.present(window, &frame(10), Some(second)),
            Ok(PresentOutcome::Presented)
        );
        assert_eq!(ws.unacknowledged(window).unwrap(), &[]);
        assert_eq!(
            ws.present(window, &frame(11), Some(second)),
            Err(PlatformError::Protocol(
                "configure 2 is not waiting for an acknowledgement".into()
            ))
        );
        assert_eq!(ws.frames(window).unwrap().len(), 1);
        assert_eq!(ws.frames(window).unwrap()[0].acknowledged, Some(second));
        assert_eq!(
            ws.frames(window).unwrap()[0].pixels,
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn without_kept_pixels_a_frame_copies_only_its_damage_into_the_window_buffer() {
        let mut ws = HeadlessWindowSystem::default();
        ws.set_keep_pixels(false);
        let window = ws.create_window(&spec()).unwrap();
        ws.present(window, &frame(10), None).unwrap();
        let mut second = frame(11);
        second.pixels = vec![9; 8];
        second.damage = vec![PixelRect {
            x: 1,
            y: 0,
            width: 1,
            height: 1,
        }];
        ws.present(window, &second, None).unwrap();
        let frames = ws.frames(window).unwrap();
        assert!(frames.iter().all(|f| f.pixels.is_empty()));
        assert_eq!(frames[1].damage, second.damage);
        assert_eq!(
            ws.window_mut(window).unwrap().buffer,
            [1, 2, 3, 4, 9, 9, 9, 9]
        );
    }

    #[test]
    fn a_frame_callback_needs_a_request_and_is_consumed() {
        let mut ws = HeadlessWindowSystem::default();
        let window = ws.create_window(&spec()).unwrap();
        assert!(ws.frame_ready(window, Timestamp(16)).is_err());
        ws.request_frame(window);
        assert_eq!(ws.frame_requested(window), Ok(true));
        ws.frame_ready(window, Timestamp(16)).unwrap();
        assert_eq!(ws.frame_requested(window), Ok(false));
        assert!(ws.frame_ready(window, Timestamp(32)).is_err());
        assert_eq!(
            drain(&mut ws),
            vec![PlatformEvent::Window {
                window,
                event: WindowEvent::Frame(Timestamp(16))
            }]
        );
    }

    #[test]
    fn a_presented_frame_stays_in_flight_until_the_test_shows_it() {
        let mut ws = HeadlessWindowSystem::default();
        let window = ws.create_window(&spec()).unwrap();
        ws.present(window, &frame(1), None).unwrap();
        ws.present(window, &frame(2), None).unwrap();
        assert_eq!(drain(&mut ws), vec![]);
        assert_eq!(ws.show_oldest(window, Timestamp(5)), Ok(RenderFrameId(1)));
        assert_eq!(ws.show_oldest(window, Timestamp(6)), Ok(RenderFrameId(2)));
        assert!(ws.show_oldest(window, Timestamp(7)).is_err());
        assert_eq!(
            drain(&mut ws),
            vec![
                PlatformEvent::Window {
                    window,
                    event: WindowEvent::Presented {
                        frame: RenderFrameId(1),
                        time: Timestamp(5)
                    }
                },
                PlatformEvent::Window {
                    window,
                    event: WindowEvent::Presented {
                        frame: RenderFrameId(2),
                        time: Timestamp(6)
                    }
                },
            ]
        );
    }

    #[test]
    fn presenting_reports_not_ready_while_no_buffer_is_free() {
        let mut ws = HeadlessWindowSystem::default();
        let window = ws.create_window(&spec()).unwrap();
        ws.set_accepting_frames(false);
        assert_eq!(
            ws.present(window, &frame(1), None),
            Ok(PresentOutcome::NotReady)
        );
        assert_eq!(ws.frames(window).unwrap().len(), 0);
        ws.set_accepting_frames(true);
        assert_eq!(
            ws.present(window, &frame(2), None),
            Ok(PresentOutcome::Presented)
        );
    }

    #[test]
    fn events_for_a_destroyed_window_are_refused_or_dropped() {
        let mut ws = HeadlessWindowSystem::default();
        let window = ws.create_window(&spec()).unwrap();
        ws.enqueue_window(window, WindowEvent::Focus(true)).unwrap();
        ws.destroy(window);
        assert_eq!(drain(&mut ws), vec![]);
        assert_eq!(
            ws.enqueue_window(window, WindowEvent::Focus(false)),
            Err(PlatformError::UnknownWindow(window))
        );
        assert_eq!(
            ws.request(window, WindowRequest::Minimize),
            Err(PlatformError::UnknownWindow(window))
        );
        assert_eq!(ws.windows(), vec![]);
    }

    #[test]
    fn destroying_a_window_destroys_its_popups() {
        let mut ws = HeadlessWindowSystem::default();
        let parent = ws.create_window(&spec()).unwrap();
        let other = ws.create_window(&spec()).unwrap();
        let popup_spec = PopupSpec {
            anchor_rect: cupertino_geometry::Rect::ZERO,
            anchor: Default::default(),
            gravity: Default::default(),
            offset: cupertino_geometry::Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 50.0,
                height: 20.0,
            },
            constraint: Default::default(),
            reactive: false,
            grab: true,
        };
        let popup = ws.create_popup(parent, &popup_spec).unwrap();
        let nested = ws.create_popup(popup, &popup_spec).unwrap();
        let sibling = ws.create_popup(other, &popup_spec).unwrap();
        ws.enqueue_window(nested, WindowEvent::PopupDone).unwrap();
        ws.destroy(parent);
        assert_eq!(ws.windows(), vec![other, sibling]);
        assert_eq!(drain(&mut ws), vec![]);
        assert_eq!(ws.parent(sibling), Ok(Some(other)));
    }

    #[test]
    fn the_clipboard_answers_only_the_mime_type_that_was_set() {
        let mut ws = HeadlessWindowSystem::default();
        ws.set_clipboard(
            ClipboardKind::Clipboard,
            ClipboardData {
                mime: "text/plain".into(),
                bytes: b"abc".to_vec(),
            },
        );
        let hit = ws.read_clipboard(ClipboardKind::Clipboard, "text/plain");
        let miss = ws.read_clipboard(ClipboardKind::Clipboard, "image/png");
        let primary = ws.read_clipboard(ClipboardKind::Primary, "text/plain");
        assert_eq!(
            drain(&mut ws),
            vec![
                PlatformEvent::Clipboard {
                    token: hit,
                    data: Some(ClipboardData {
                        mime: "text/plain".into(),
                        bytes: b"abc".to_vec()
                    })
                },
                PlatformEvent::Clipboard {
                    token: miss,
                    data: None
                },
                PlatformEvent::Clipboard {
                    token: primary,
                    data: None
                },
            ]
        );
    }

    #[test]
    fn requests_and_text_input_state_are_kept_per_window() {
        let mut ws = HeadlessWindowSystem::default();
        let a = ws.create_window(&spec()).unwrap();
        let b = ws.create_window(&spec()).unwrap();
        ws.request(a, WindowRequest::SetTitle("One".into()))
            .unwrap();
        ws.request(a, WindowRequest::Maximize(true)).unwrap();
        ws.set_cursor(b, Some(CursorShape::Text));
        assert_eq!(
            ws.requests(a).unwrap(),
            &[
                WindowRequest::SetTitle("One".into()),
                WindowRequest::Maximize(true)
            ]
        );
        assert_eq!(ws.requests(b).unwrap(), &[]);
        assert_eq!(ws.cursor(a), Ok(Some(CursorShape::Default)));
        assert_eq!(ws.cursor(b), Ok(Some(CursorShape::Text)));
        assert_eq!(ws.text_input(a), Ok(None));
    }

    #[test]
    fn a_wake_from_another_thread_ends_the_wait_and_none_is_lost() {
        let system = HeadlessWindowSystem::default();
        let waker = system.waker();
        // a wake before the wait counts once
        waker.wake_by_ref();
        assert!(system.wait_for_wake(Duration::from_secs(10)));
        assert!(
            !system.wait_for_wake(Duration::ZERO),
            "the wake was used up"
        );
        // a wake from another thread while the test waits
        let (started, starts) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            starts.recv().unwrap();
            waker.wake();
        });
        started.send(()).unwrap();
        assert!(system.wait_for_wake(Duration::from_secs(10)));
        thread.join().unwrap();
    }
}
