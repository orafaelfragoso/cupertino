use cupertino_geometry::Size;
use cupertino_ui_platform::{
    ActivationRequest, Capabilities, ClipboardData, ClipboardKind, ClipboardToken, Configure,
    ConfigureId, CursorShape, PlatformError, PlatformEvent, PopupSpec, PresentOutcome, Timestamp,
    WindowEvent, WindowId, WindowRequest, WindowSpec, WindowStates, WindowSystem, WmCapabilities,
};
use cupertino_ui_render::{CpuFrame, PixelFormat, PixelRect, RenderFrameId};
use cupertino_ui_text::TextInputState;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::task::Waker;
use std::time::Duration;

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
}

/// A window system with no display. Windows live in memory, every request is kept for inspection, presented
/// frames are kept until the window is destroyed, and the only events are the ones the test enqueues.
#[derive(Debug)]
pub struct HeadlessWindowSystem {
    capabilities: Capabilities,
    windows: BTreeMap<WindowId, HeadlessWindow>,
    next_window: u32,
    next_configure: u32,
    next_token: u64,
    queue: VecDeque<PlatformEvent>,
    clipboards: HashMap<ClipboardKind, ClipboardData>,
    accepting_frames: bool,
    pub(crate) dispatch_one: bool,
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
            accepting_frames: true,
            dispatch_one: false,
        }
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
            },
        );
        id
    }

    /// Queues an event for the next [`dispatch`](WindowSystem::dispatch). A window event for a window that does
    /// not exist is an error, so a test cannot deliver input to a stale ID.
    pub fn enqueue(&mut self, event: PlatformEvent) -> Result<(), PlatformError> {
        if let PlatformEvent::Window { window, .. } = &event {
            self.window(*window)?;
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

    /// Queues a configure and returns its ID, which the runtime must acknowledge with a presented frame.
    pub fn configure(
        &mut self,
        window: WindowId,
        size: Option<Size>,
        states: WindowStates,
    ) -> Result<ConfigureId, PlatformError> {
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

    fn request_activation_token(&mut self, window: WindowId) -> ActivationRequest {
        let request = ActivationRequest(self.next_token);
        self.next_token += 1;
        if self.windows.contains_key(&window) {
            self.queue.push_back(PlatformEvent::ActivationToken {
                request,
                token: format!("headless-activation-{}", request.0),
            });
        }
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
        let accepting = self.accepting_frames;
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
        w.frames.push(PresentedFrame {
            id: frame.id,
            width: frame.width,
            height: frame.height,
            format: frame.format,
            pixels: frame.pixels.clone(),
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
        Waker::noop().clone()
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
}
