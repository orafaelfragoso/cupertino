//! Thread-local runtime context for the calling component or standalone shell program.
//!
//! Every request function in `cupertino-kit-shell` acts on the component running on the current
//! thread and returns immediately; when called on a thread with no active component context, it
//! logs once and does nothing (`docs/architecture/shell-host.md#rules`).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use cupertino_kit::core::{Environment, WindowKey};
use cupertino_kit::geometry::Rect;

use crate::bus::{BusHub, Message, Topic};
use crate::host::HelperHandle;
use crate::outputs::{Output, OutputId};
use crate::transaction::{Gesture, Id as TxId};
use crate::{ComponentName, ServerWindowId, WorkspaceId};

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
    pub outgoing: VecDeque<ShellRequest>,
    pub bus: Option<Arc<BusHub>>,
    pub bus_subscriber_id: Option<u64>,
    pub helpers: Arc<Mutex<Vec<HelperHandle>>>,
    pub children: Arc<Mutex<Vec<std::process::Child>>>,
}

impl Default for ComponentContext {
    fn default() -> Self {
        Self {
            component: None,
            instance: 1,
            environment: None,
            holds: Vec::new(),
            locked: false,
            outputs: Vec::new(),
            next_tx_serial: 1,
            next_watch_id: 1,
            next_follow_id: 1,
            outgoing: VecDeque::new(),
            bus: None,
            bus_subscriber_id: None,
            helpers: Arc::new(Mutex::new(Vec::new())),
            children: Arc::new(Mutex::new(Vec::new())),
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

/// Trace-inspect a drained outgoing request (used by the component loop when flushing requests).
pub(crate) fn trace_request(req: &ShellRequest) {
    match req {
        ShellRequest::SessionActivateApp { serial, desktop_id } => {
            tracing::trace!(serial, desktop_id, "shell: session.activate_app");
        }
        ShellRequest::SessionWatchIdle { id, after } => {
            tracing::trace!(id, ?after, "shell: session.watch_idle");
        }
        ShellRequest::SessionUnwatchIdle { id } => {
            tracing::trace!(id, "shell: session.unwatch_idle");
        }
        ShellRequest::MenuBarSetOpen { window, open } => {
            tracing::trace!(?window, open, "shell: menu_bar.set_menu_open");
        }
        ShellRequest::MenuBarWindowCommand { window, command } => {
            tracing::trace!(?window, ?command, "shell: menu_bar.window_command");
        }
        ShellRequest::MenuBarActivateWindow { window, id } => {
            tracing::trace!(?window, ?id, "shell: menu_bar.activate_window");
        }
        ShellRequest::MenuBarMoveToWorkspace { window, workspace } => {
            tracing::trace!(?window, workspace, "shell: menu_bar.move_to_workspace");
        }
        ShellRequest::MenuBarSessionAction { window, action } => {
            tracing::trace!(?window, ?action, "shell: menu_bar.session_action");
        }
        ShellRequest::MenuBarEdit { window, action } => {
            tracing::trace!(?window, ?action, "shell: menu_bar.edit");
        }
        ShellRequest::MenuBarForceQuitApp { window, app_id } => {
            tracing::trace!(?window, app_id, "shell: menu_bar.force_quit_app");
        }
        ShellRequest::DockActivateApp { window, app_id } => {
            tracing::trace!(?window, app_id, "shell: dock.activate_app");
        }
        ShellRequest::DockActivateWindow { window, id } => {
            tracing::trace!(?window, ?id, "shell: dock.activate_window");
        }
        ShellRequest::DockRestoreWindow { window, id } => {
            tracing::trace!(?window, ?id, "shell: dock.restore_window");
        }
        ShellRequest::DockSetMinimizeTarget { window, id, rect } => {
            tracing::trace!(?window, ?id, ?rect, "shell: dock.set_minimize_target");
        }
        ShellRequest::DockForceQuitApp { window, app_id } => {
            tracing::trace!(?window, app_id, "shell: dock.force_quit_app");
        }
        ShellRequest::DockAppCommand {
            window,
            app_id,
            command,
        } => {
            tracing::trace!(?window, app_id, ?command, "shell: dock.app_command");
        }
        ShellRequest::DockRunAction { window, action } => {
            tracing::trace!(?window, action, "shell: dock.run_action");
        }
        ShellRequest::DockSwitcherSelect { window, index } => {
            tracing::trace!(?window, index, "shell: dock.switcher_select");
        }
        ShellRequest::DockSwitcherActivate { window } => {
            tracing::trace!(?window, "shell: dock.switcher_activate");
        }
        ShellRequest::DockSwitcherCancel { window } => {
            tracing::trace!(?window, "shell: dock.switcher_cancel");
        }
        ShellRequest::DockSwitchWorkspace { window, workspace } => {
            tracing::trace!(?window, ?workspace, "shell: dock.switch_workspace");
        }
        ShellRequest::DockAddWorkspace { window } => {
            tracing::trace!(?window, "shell: dock.add_workspace");
        }
        ShellRequest::DockRemoveWorkspace { window, workspace } => {
            tracing::trace!(?window, ?workspace, "shell: dock.remove_workspace");
        }
        ShellRequest::DockMoveWorkspace {
            window,
            workspace,
            index,
        } => {
            tracing::trace!(?window, ?workspace, index, "shell: dock.move_workspace");
        }
        ShellRequest::WindowManagerPlace {
            output,
            window,
            placement,
        } => {
            tracing::trace!(
                output = output.raw(),
                ?window,
                ?placement,
                "shell: wm.place"
            );
        }
        ShellRequest::WindowManagerCommit { output } => {
            tracing::trace!(output = output.raw(), "shell: wm.commit");
        }
        ShellRequest::WindowManagerActivateWindow { output, window } => {
            tracing::trace!(output = output.raw(), ?window, "shell: wm.activate_window");
        }
        ShellRequest::OverviewActivateWindow { output, window } => {
            tracing::trace!(
                output = output.raw(),
                ?window,
                "shell: overview.activate_window"
            );
        }
        ShellRequest::OverviewSwitchWorkspace { output, workspace } => {
            tracing::trace!(
                output = output.raw(),
                ?workspace,
                "shell: overview.switch_workspace"
            );
        }
        ShellRequest::OverviewAddWorkspace { output } => {
            tracing::trace!(output = output.raw(), "shell: overview.add_workspace");
        }
        ShellRequest::OverviewRemoveWorkspace { output, workspace } => {
            tracing::trace!(
                output = output.raw(),
                ?workspace,
                "shell: overview.remove_workspace"
            );
        }
        ShellRequest::OverviewMoveWorkspace {
            output,
            workspace,
            index,
        } => {
            tracing::trace!(
                output = output.raw(),
                ?workspace,
                index,
                "shell: overview.move_workspace"
            );
        }
        ShellRequest::OverviewMoveWindow {
            output,
            window,
            workspace,
        } => {
            tracing::trace!(
                output = output.raw(),
                ?window,
                ?workspace,
                "shell: overview.move_window"
            );
        }
        ShellRequest::TxCreate { id, output } => {
            tracing::trace!(?id, output = output.raw(), "shell: tx.create");
        }
        ShellRequest::TxJoin { id, output } => {
            tracing::trace!(?id, output = output.raw(), "shell: tx.join");
        }
        ShellRequest::TxExpect { id, component } => {
            tracing::trace!(?id, ?component, "shell: tx.expect");
        }
        ShellRequest::TxDeadline { id, deadline } => {
            tracing::trace!(?id, ?deadline, "shell: tx.deadline");
        }
        ShellRequest::TxStage { id, window } => {
            tracing::trace!(?id, ?window, "shell: tx.stage");
        }
        ShellRequest::TxBeginWindowChanges { id } => {
            tracing::trace!(?id, "shell: tx.begin_window_changes");
        }
        ShellRequest::TxEndWindowChanges { id } => {
            tracing::trace!(?id, "shell: tx.end_window_changes");
        }
        ShellRequest::TxBindGesture {
            id,
            gesture,
            response,
            damping,
        } => {
            tracing::trace!(?id, ?gesture, response, damping, "shell: tx.bind_gesture");
        }
        ShellRequest::TxBindReverseGesture { id, gesture } => {
            tracing::trace!(?id, ?gesture, "shell: tx.bind_reverse_gesture");
        }
        ShellRequest::TxSubmit { id } => {
            tracing::trace!(?id, "shell: tx.submit");
        }
        ShellRequest::TxCancel { id } => {
            tracing::trace!(?id, "shell: tx.cancel");
        }
        ShellRequest::TxDestroy { id } => {
            tracing::trace!(?id, "shell: tx.destroy");
        }
        ShellRequest::LockRequest => tracing::trace!("shell: lock.lock"),
        ShellRequest::LockUnlock => tracing::trace!("shell: lock.unlock"),
        ShellRequest::ConsentBind => tracing::trace!("shell: consent.bind"),
        ShellRequest::ConsentSetWindow { window } => {
            tracing::trace!(?window, "shell: consent.set_window");
        }
        ShellRequest::ConsentAnswer { run, answer } => {
            tracing::trace!(run = run.raw(), ?answer, "shell: consent.answer");
        }
        ShellRequest::ConsentStop { run } => {
            tracing::trace!(run = run.raw(), "shell: consent.stop");
        }
        ShellRequest::CaptureWindows => tracing::trace!("shell: capture.windows"),
        ShellRequest::CaptureFollowStart {
            id,
            source,
            options,
        } => {
            tracing::trace!(id, ?source, ?options, "shell: capture.follow_start");
        }
        ShellRequest::CaptureFollowStop { id } => {
            tracing::trace!(id, "shell: capture.follow_stop");
        }
    }
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
