//! Thread-local runtime context for the calling component or standalone shell program.
//!
//! Every request function in `cupertino-kit-shell` acts on the component running on the current
//! thread and returns immediately; when called on a thread with no active component context, it
//! logs once and does nothing (`docs/architecture/shell-host.md#rules`).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, Once, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use cupertino_kit::core::{Environment, WindowKey};
use cupertino_kit::geometry::Rect;

use crate::bus::{BusHub, Message, Topic};
use crate::capture::Captures;
use crate::host::{Fault, HelperHandle, Reaper};
use crate::link::ComponentLink;
use crate::outputs::{Output, OutputId};
use crate::transaction::{Gesture, Id as TxId};
use crate::{ComponentName, Event, ServerWindowId, WorkspaceId};

/// The sockets of a component the host shuts down when it cuts the component off (`hosting::Running::shut`), other
/// than its Wayland connection: its session-bus connections.
pub(crate) type Sockets = Arc<Mutex<Vec<UnixStream>>>;

/// Keeps `socket`, a handle to one of the component's sockets, for the host to shut.
pub(crate) fn keep_socket(sockets: &Sockets, socket: UnixStream) {
    sockets
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(socket);
}

/// The shell's events for the component (its protocol objects', `Event::BusIdentity`), waiting for the loop to hand them
/// to [`crate::Program::event`]. The task that waits for them is woken by each push, and by [`Inbox::wake`].
#[derive(Default)]
pub(crate) struct Inbox {
    state: Mutex<InboxState>,
}

#[derive(Default)]
struct InboxState {
    events: VecDeque<Event>,
    /// The task waiting for the next event.
    waker: Option<Waker>,
}

impl Inbox {
    fn lock(&self) -> std::sync::MutexGuard<'_, InboxState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn push(&self, event: Event) {
        let waker = {
            let mut state = self.lock();
            state.events.push_back(event);
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Wakes the task waiting for an event without giving it one: the loop it runs in takes a turn.
    pub(crate) fn wake(&self) {
        if let Some(waker) = self.lock().waker.take() {
            waker.wake();
        }
    }

    /// The next event, or `Pending` with `cx`'s task woken by the next push.
    pub(crate) fn poll_next(&self, cx: &mut Context<'_>) -> Poll<Event> {
        let mut state = self.lock();
        match state.events.pop_front() {
            Some(event) => Poll::Ready(event),
            None => {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Queued outgoing protocol action flushed before the component's event loop waits.
#[derive(Clone, Debug)]
pub(crate) enum ShellRequest {
    SessionActivateApp {
        serial: u32,
        desktop_id: String,
    },
    SessionWatchIdle {
        id: u64,
        after: Duration,
    },
    SessionUnwatchIdle {
        id: u64,
    },
    MenuBarSetOpen {
        window: WindowKey,
        open: bool,
    },
    MenuBarWindowCommand {
        window: WindowKey,
        command: crate::menu_bar::WindowCommand,
    },
    MenuBarActivateWindow {
        window: WindowKey,
        id: ServerWindowId,
    },
    MenuBarMoveToWorkspace {
        window: WindowKey,
        workspace: u32,
    },
    MenuBarSessionAction {
        window: WindowKey,
        action: crate::menu_bar::SessionAction,
    },
    MenuBarEdit {
        window: WindowKey,
        action: crate::menu_bar::EditAction,
    },
    MenuBarForceQuitApp {
        window: WindowKey,
        app_id: String,
    },
    DockActivateApp {
        window: WindowKey,
        app_id: String,
    },
    DockActivateWindow {
        window: WindowKey,
        id: ServerWindowId,
    },
    DockRestoreWindow {
        window: WindowKey,
        id: ServerWindowId,
    },
    DockSetMinimizeTarget {
        window: WindowKey,
        id: ServerWindowId,
        rect: Rect,
    },
    DockForceQuitApp {
        window: WindowKey,
        app_id: String,
    },
    DockAppCommand {
        window: WindowKey,
        app_id: String,
        command: crate::dock::AppCommand,
    },
    DockRunAction {
        window: WindowKey,
        action: String,
    },
    DockSwitcherSelect {
        window: WindowKey,
        index: u32,
    },
    DockSwitcherActivate {
        window: WindowKey,
    },
    DockSwitcherCancel {
        window: WindowKey,
    },
    DockSwitchWorkspace {
        window: WindowKey,
        workspace: WorkspaceId,
    },
    DockAddWorkspace {
        window: WindowKey,
    },
    DockRemoveWorkspace {
        window: WindowKey,
        workspace: WorkspaceId,
    },
    DockMoveWorkspace {
        window: WindowKey,
        workspace: WorkspaceId,
        index: u32,
    },
    WindowManagerPlace {
        output: OutputId,
        window: ServerWindowId,
        placement: crate::window_manager::Placement,
    },
    WindowManagerCommit {
        output: OutputId,
    },
    WindowManagerActivateWindow {
        output: OutputId,
        window: ServerWindowId,
    },
    OverviewActivateWindow {
        output: OutputId,
        window: ServerWindowId,
    },
    OverviewSwitchWorkspace {
        output: OutputId,
        workspace: WorkspaceId,
    },
    OverviewAddWorkspace {
        output: OutputId,
    },
    OverviewRemoveWorkspace {
        output: OutputId,
        workspace: WorkspaceId,
    },
    OverviewMoveWorkspace {
        output: OutputId,
        workspace: WorkspaceId,
        index: u32,
    },
    OverviewMoveWindow {
        output: OutputId,
        window: ServerWindowId,
        workspace: WorkspaceId,
    },
    AccentPick {
        serial: u32,
        index: u32,
    },
    TxCreate {
        id: TxId,
        output: OutputId,
    },
    TxJoin {
        id: TxId,
        output: OutputId,
    },
    TxExpect {
        id: TxId,
        component: ComponentName,
    },
    TxDeadline {
        id: TxId,
        deadline: Duration,
    },
    TxStage {
        id: TxId,
        window: WindowKey,
    },
    TxBeginWindowChanges {
        id: TxId,
    },
    TxEndWindowChanges {
        id: TxId,
    },
    TxBindGesture {
        id: TxId,
        gesture: Gesture,
        response: f64,
        damping: f64,
    },
    TxBindReverseGesture {
        id: TxId,
        gesture: Gesture,
    },
    TxSubmit {
        id: TxId,
    },
    TxCancel {
        id: TxId,
    },
    TxDestroy {
        id: TxId,
    },
    LockRequest,
    LockUnlock,
    ConsentBind,
    ConsentSetWindow {
        window: WindowKey,
    },
    ConsentAnswer {
        run: crate::consent::Run,
        answer: crate::consent::Answer,
    },
    ConsentStop {
        run: crate::consent::Run,
    },
    CaptureWindows,
    CaptureOnce {
        id: u32,
        source: crate::capture::Source,
        options: crate::capture::Options,
    },
    CaptureFollowStart {
        id: u32,
        source: crate::capture::Source,
        options: crate::capture::Options,
    },
    CaptureFollowStop {
        id: u32,
    },
}

pub(crate) struct ComponentContext {
    pub component: Option<ComponentName>,
    pub instance: u32,
    pub environment: Option<Environment>,
    pub holds: Vec<String>,
    pub locked: bool,
    pub outputs: Vec<Output>,
    pub next_tx_serial: u32,
    pub next_watch_id: u64,
    pub next_follow_id: u32,
    pub next_capture_id: u32,
    pub outgoing: VecDeque<ShellRequest>,
    pub bus: Option<Arc<BusHub>>,
    pub bus_subscriber_id: Option<u64>,
    pub helpers: Arc<Mutex<Vec<HelperHandle>>>,
    /// What the component's helpers tell its host; `None` outside a host.
    pub fault: Option<Arc<Fault>>,
    /// The host's reaper of child processes; `None` outside a host.
    pub reaper: Option<Arc<Reaper>>,
    /// The component's `cupertino_component_v1`; `None` outside a host.
    pub link: Option<Arc<ComponentLink>>,
    pub sockets: Sockets,
    pub inbox: Arc<Inbox>,
    /// The single-frame captures waiting for their frame.
    pub captures: Captures,
    /// Where the component was asked to panic (development builds).
    #[cfg(debug_assertions)]
    pub crash: Arc<crate::crash::Crash>,
}

impl Default for ComponentContext {
    fn default() -> Self {
        Self {
            component: None,
            // a program of its own has no instance: the server knows it by its process
            instance: 0,
            environment: None,
            holds: Vec::new(),
            locked: false,
            outputs: Vec::new(),
            next_tx_serial: 1,
            next_watch_id: 1,
            next_follow_id: 1,
            next_capture_id: 1,
            outgoing: VecDeque::new(),
            bus: None,
            bus_subscriber_id: None,
            helpers: Arc::new(Mutex::new(Vec::new())),
            fault: None,
            reaper: None,
            link: None,
            sockets: Sockets::default(),
            inbox: Arc::default(),
            captures: Captures::default(),
            #[cfg(debug_assertions)]
            crash: Arc::default(),
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Option<ComponentContext>> = const { RefCell::new(None) };
}

static WARNED_NO_CONTEXT: Once = Once::new();

fn warn_no_context() {
    WARNED_NO_CONTEXT.call_once(|| {
        tracing::warn!(
            "cupertino-kit-shell request called on a thread with no active component; ignored"
        );
    });
}

/// Install a component context on the current thread (used by `hosting::Runtime` and `run`).
pub(crate) fn install(ctx: ComponentContext) {
    CURRENT.with(|slot| *slot.borrow_mut() = Some(ctx));
}

/// Remove and return the component context on the current thread.
pub(crate) fn take() -> Option<ComponentContext> {
    CURRENT.with(|slot| slot.borrow_mut().take())
}

/// Access the active context immutably.
pub(crate) fn with_ctx<R>(f: impl FnOnce(&ComponentContext) -> R) -> Option<R> {
    CURRENT.with(|slot| slot.borrow().as_ref().map(f))
}

/// Access the active context mutably, logging once if absent.
pub(crate) fn with_ctx_mut<R>(f: impl FnOnce(&mut ComponentContext) -> R) -> Option<R> {
    let out = CURRENT.with(|slot| slot.borrow_mut().as_mut().map(f));
    if out.is_none() {
        warn_no_context();
    }
    out
}

/// Drain queued outgoing shell requests for flushing to the component's Wayland connection.
pub(crate) fn drain_outgoing() -> Vec<ShellRequest> {
    with_ctx_mut(|ctx| ctx.outgoing.drain(..).collect()).unwrap_or_default()
}

/// Take the queued request that names `window` the consent window out of the queue, if the program made one: the
/// window's surface is created now, and the server learns of it before any commit.
pub(crate) fn take_consent_window(window: &WindowKey) -> bool {
    with_ctx_mut(|ctx| {
        let before = ctx.outgoing.len();
        ctx.outgoing.retain(
            |request| !matches!(request, ShellRequest::ConsentSetWindow { window: w } if w == window),
        );
        ctx.outgoing.len() != before
    })
    .unwrap_or(false)
}

/// Enqueue a protocol request on the calling thread's component context.
pub(crate) fn push_request(req: ShellRequest) {
    let _ = with_ctx_mut(|ctx| ctx.outgoing.push_back(req));
}

/// Publish a host-bus message from the calling thread's component context.
pub(crate) fn publish_bus(message: Message) {
    let _ = with_ctx_mut(|ctx| {
        if message.topic() == Topic::Host {
            tracing::warn!(
                component = ?ctx.component,
                "components may not publish on Topic::Host; dropping message"
            );
            return;
        }
        if let (Some(bus), Some(comp)) = (&ctx.bus, ctx.component) {
            bus.publish_from_component(comp, ctx.bus_subscriber_id, message);
        }
    });
}

/// Follow host-bus topics from the calling thread's component context.
pub(crate) fn follow_bus(topics: &[Topic]) {
    let _ = with_ctx_mut(|ctx| {
        if let (Some(bus), Some(sub_id)) = (&ctx.bus, ctx.bus_subscriber_id) {
            bus.follow(sub_id, topics);
        }
    });
}
