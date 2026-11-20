//! The bus side of `org.cupertino.Session1`: the interface's methods, signals and properties, served on a thread of
//! its own. Each call is handed to the compositor's loop as a [`Call`] with its caller and answered from there.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::channel::{self, Event};
use zbus::object_server::SignalEmitter;

use super::{
    shell_process_record, BusOut, Call, Refusal, Reject, Serving, SessionAction, SessionConnection,
    SessionSettings, ShellProcessRecord, BUS_NAME, OBJECT_PATH,
};
use crate::caller::{identify, Asker, Caller, Denial, Sandbox};
use crate::capture::launch::SERVICE_NAME;
use crate::state::Cupertino;
use cupertino_peer::ProcessHandle;
use zbus::export::futures_core::Stream;

struct Session1 {
    calls: std::sync::Mutex<channel::Sender<Call>>,
    /// What the properties read; the compositor updates it in [`Cupertino::publish_session_settings`].
    settings: Arc<Mutex<SessionSettings>>,
}

/// What a call of the session interface fails with: the standard errors, and one name for each way a caller is refused.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Session1.Error")]
enum CallError {
    #[zbus(error)]
    ZBus(zbus::Error),
    AnonymousCaller(String),
    FlatpakCaller(String),
    SnapCaller(String),
    UntrustedCaller(String),
}

impl From<zbus::fdo::Error> for CallError {
    fn from(e: zbus::fdo::Error) -> Self {
        CallError::ZBus(e.into())
    }
}

impl From<Reject> for CallError {
    fn from(reject: Reject) -> Self {
        match reject {
            Reject::Caller(Denial::Anonymous) => {
                CallError::AnonymousCaller("the bus gave no process for this caller".into())
            }
            Reject::Caller(Denial::Sandboxed(Sandbox::Flatpak)) => {
                CallError::FlatpakCaller("a Flatpak may not use this method".into())
            }
            Reject::Caller(Denial::Sandboxed(Sandbox::Snap)) => {
                CallError::SnapCaller("a Snap may not use this method".into())
            }
            Reject::Caller(Denial::Untrusted) => {
                CallError::UntrustedCaller("this process may not use this method".into())
            }
            Reject::Refusal(Refusal::InvalidArgs(m)) => zbus::fdo::Error::InvalidArgs(m).into(),
            Reject::Refusal(Refusal::NotSupported(m)) => zbus::fdo::Error::NotSupported(m).into(),
            Reject::Refusal(Refusal::Failed(m)) => zbus::fdo::Error::Failed(m).into(),
        }
    }
}

impl Session1 {
    async fn ask<T>(
        &self,
        make: impl FnOnce(async_channel::Sender<Result<T, Reject>>) -> Call,
    ) -> Result<T, CallError> {
        let (tx, rx) = async_channel::bounded(1);
        self.calls
            .lock()
            .expect("no thread panics while sending a call")
            .send(make(tx))
            .map_err(|_| zbus::fdo::Error::Failed("the compositor is shutting down".into()))?;
        rx.recv()
            .await
            .map_err(|_| zbus::fdo::Error::Failed("the compositor dropped the call".into()))?
            .map_err(CallError::from)
    }
}

/// The caller of a method, from the header of its message.
async fn caller_of(header: &zbus::message::Header<'_>, connection: &zbus::Connection) -> Caller {
    match header.sender() {
        Some(sender) => identify(connection, sender).await,
        None => Caller::Anonymous,
    }
}

/// The caller of a method and the connection it came on, from the header of its message.
async fn asker_of(header: &zbus::message::Header<'_>, connection: &zbus::Connection) -> Asker {
    Asker {
        caller: caller_of(header, connection).await,
        sender: header.sender().map(|s| s.as_str().to_owned()),
    }
}

/// The caller of a method and the user id the bus daemon gives for its connection, from one answer of the daemon.
async fn caller_and_user_of(
    header: &zbus::message::Header<'_>,
    connection: &zbus::Connection,
) -> (Caller, Option<u32>) {
    let Some(sender) = header.sender() else {
        return (Caller::Anonymous, None);
    };
    let Ok(proxy) = zbus::fdo::DBusProxy::new(connection).await else {
        return (Caller::Anonymous, None);
    };
    match proxy
        .get_connection_credentials(sender.clone().into())
        .await
    {
        Ok(credentials) => (
            Caller::from_credentials(&credentials, std::path::Path::new("/proc")),
            credentials.unix_user_id(),
        ),
        Err(_) => (Caller::Anonymous, None),
    }
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl Session1 {
    async fn quit_next_application(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(String, u32), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::QuitNext(caller, tx)).await
    }

    async fn quit_status(
        &self,
        app_id: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<String, CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::QuitStatus(app_id, caller, tx)).await
    }

    async fn force_quit_application(
        &self,
        app_id: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<u32, CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::ForceQuit(app_id, caller, tx)).await
    }

    async fn open_applications(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<Vec<String>, CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::OpenApplications(caller, tx)).await
    }

    async fn pending_interrupt(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(bool, String, String, String), CallError> {
        let caller = asker_of(&header, connection).await;
        let pending = self.ask(|tx| Call::PendingInterrupt(caller, tx)).await?;
        Ok(match pending {
            Some((action, app, reason)) => (true, action.to_owned(), app, reason),
            None => (false, String::new(), String::new(), String::new()),
        })
    }

    async fn set_appearance(
        &self,
        appearance: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::SetAppearance(appearance, caller, tx))
            .await
    }

    async fn set_accessibility(
        &self,
        key: String,
        value: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::SetAccessibility(key, value, caller, tx))
            .await
    }

    async fn list_shell_processes(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<Vec<ShellProcessRecord>, CallError> {
        let (caller, user) = caller_and_user_of(&header, connection).await;
        let processes = self
            .ask(|tx| Call::ListShellProcesses(caller, user, tx))
            .await?;
        Ok(processes.iter().map(shell_process_record).collect())
    }

    async fn restart_shell_process(
        &self,
        name: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<ShellProcessRecord, CallError> {
        let (caller, user) = caller_and_user_of(&header, connection).await;
        let process = self
            .ask(|tx| Call::RestartShellProcess(name, caller, user, tx))
            .await?;
        Ok(shell_process_record(&process))
    }

    async fn announce_component(
        &self,
        nonce: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let sender = header
            .sender()
            .map(|s| s.as_str().to_owned())
            .unwrap_or_default();
        let caller = caller_of(&header, connection).await;
        self.ask(|reply| Call::AnnounceComponent {
            sender,
            nonce,
            caller,
            reply,
        })
        .await
    }

    async fn component_of(&self, unique_name: String) -> Result<String, CallError> {
        self.ask(|tx| Call::ComponentOf(unique_name, tx)).await
    }

    async fn shell_hosts(&self) -> Result<Vec<(u32, u64)>, CallError> {
        self.ask(Call::ShellHosts).await
    }

    async fn program_of(
        &self,
        unique_name: String,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<String, CallError> {
        let process = match zbus::names::UniqueName::try_from(unique_name.as_str()) {
            Ok(name) => identify(connection, &name).await,
            Err(_) => Caller::Anonymous,
        };
        self.ask(|tx| Call::ProgramOf(process, tx)).await
    }

    async fn start_screenshot(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = caller_of(&header, connection).await;
        // asked after the call came, so a service that dropped the name before the portal called is seen without it
        let owner = screenshot_service_owner(connection).await;
        self.ask(|tx| Call::StartScreenshot(caller, owner, tx))
            .await
    }

    async fn log_out(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::SessionAction(SessionAction::LogOut, caller, tx))
            .await
    }

    async fn restart(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::SessionAction(SessionAction::Restart, caller, tx))
            .await
    }

    async fn shut_down(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::SessionAction(SessionAction::ShutDown, caller, tx))
            .await
    }

    async fn sleep(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::SessionAction(SessionAction::Sleep, caller, tx))
            .await
    }

    async fn cancel_session_action(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::CancelSessionAction(caller, tx)).await
    }

    async fn switch_to_login_window(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::SwitchToLoginWindow(caller, tx)).await
    }

    async fn retry_session_action(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = asker_of(&header, connection).await;
        self.ask(|tx| Call::RetrySessionAction(caller, tx)).await
    }

    #[zbus(property)]
    async fn appearance(&self) -> String {
        self.settings
            .lock()
            .expect("no thread panics while holding the settings")
            .appearance
            .clone()
    }

    #[zbus(property)]
    async fn accessibility(&self) -> HashMap<String, String> {
        self.settings
            .lock()
            .expect("no thread panics while holding the settings")
            .accessibility
            .clone()
    }

    /// Read-only on purpose: the resolution a user stores goes through `compositor.json`.
    #[zbus(property)]
    async fn mice(&self) -> Vec<(String, String, u32, bool)> {
        self.settings
            .lock()
            .expect("no thread panics while holding the settings")
            .mice
            .iter()
            .map(|m| (m.key.clone(), m.name.clone(), m.dpi, m.stored))
            .collect()
    }

    #[zbus(signal)]
    async fn application_quit(emitter: &SignalEmitter<'_>, app_id: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn quit_interrupted(
        emitter: &SignalEmitter<'_>,
        app_id: &str,
        reason: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn session_action_failed(
        emitter: &SignalEmitter<'_>,
        action: &str,
        reason: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn quit_interrupt_closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Serve `org.cupertino.Session1` on the session bus from a thread of its own. Failures (no bus, the name owned by
/// another compositor) are logged: the session agent then logs out without quitting the applications first.
pub fn start_session_service(state: &mut Cupertino) {
    start_session_service_on(state, None);
}

/// [`start_session_service`] on the bus at `address`, the session bus when `None`.
pub fn start_session_service_on(state: &mut Cupertino, address: Option<String>) {
    let (calls, rx) = channel::channel::<Call>();
    if let Err(e) = state.loop_handle.insert_source(rx, |event, _, state| {
        if let Event::Msg(call) = event {
            state.on_quit_call(call);
        }
    }) {
        tracing::warn!(error = %e, "cannot serve the session interface");
        return;
    }
    let signals_rx = state.quit.subscribe();
    let settings = Arc::new(Mutex::new(state.session_settings()));
    state.quit.properties = Some(Arc::clone(&settings));
    // the values the change signals were last emitted for
    let mut emitted = state.session_settings();
    let connection = Arc::clone(&state.quit.connection);
    let spawned = std::thread::Builder::new().name("session-service".into()).spawn(move || {
        // when the bus connection drops, connect again after a growing pause and claim the name again
        let mut pause = crate::dbus::RECONNECT_FIRST;
        let mut served_before = false;
        loop {
            let ended = zbus::block_on(serve_session(&address, &calls, &settings, &signals_rx, &mut emitted, &connection));
            match ended {
                Ok(true) => break,
                Ok(false) => {
                    tracing::warn!("the session bus connection dropped: serving {BUS_NAME} again");
                    pause = crate::dbus::RECONNECT_FIRST;
                    served_before = true;
                }
                Err(e) => {
                    if served_before {
                        tracing::debug!(error = %e, "the session interface {BUS_NAME} is not served yet");
                    } else {
                        tracing::warn!(error = %e, "the session interface {BUS_NAME} is not served: a log out does not quit the applications first until it is");
                        served_before = true;
                    }
                    pause = (pause * 2).min(crate::dbus::RECONNECT_LONGEST);
                }
            }
            std::thread::sleep(pause);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "cannot start the session interface thread");
    }
}

/// The process that owns the screenshot service's bus name (`SERVICE_NAME`) now, as the daemon names it; `None` when
/// no connection owns it or the daemon names no process for the owner.
async fn screenshot_service_owner(connection: &zbus::Connection) -> Option<ProcessHandle> {
    let dbus = zbus::fdo::DBusProxy::new(connection).await.ok()?;
    let name = zbus::names::BusName::try_from(SERVICE_NAME).ok()?;
    let owner = dbus.get_name_owner(name).await.ok()?;
    process_of_name(&dbus, owner.into_inner()).await
}

/// The process the daemon names for the connection `name`: by the descriptor it passes, else by its process id. No
/// `/proc` of the process is read, so a process cannot stall the session's bus thread with it.
async fn process_of_name(
    dbus: &zbus::fdo::DBusProxy<'_>,
    name: zbus::names::UniqueName<'_>,
) -> Option<ProcessHandle> {
    let credentials = dbus.get_connection_credentials(name.into()).await.ok()?;
    if let Some(fd) = credentials.process_fd() {
        let fd = std::os::fd::AsFd::as_fd(fd).try_clone_to_owned().ok()?;
        return ProcessHandle::from_pidfd(fd).ok();
    }
    ProcessHandle::open(credentials.process_id()?).ok()
}

/// Hand each new owner of the screenshot service's bus name to the compositor (`Call::ScreenshotServiceOwner`).
async fn follow_screenshot_service(
    dbus: &zbus::fdo::DBusProxy<'_>,
    change: zbus::fdo::NameOwnerChanged,
    calls: &channel::Sender<Call>,
) {
    let Ok(args) = change.args() else { return };
    let owner = match args.new_owner().as_ref() {
        Some(owner) => process_of_name(dbus, owner.clone()).await,
        None => None,
    };
    let _ = calls.send(Call::ScreenshotServiceOwner(owner));
}

/// Serve the interface on one connection until it closes (`Ok(false)`) or nothing can send signals any more (`Ok(true)`).
async fn serve_session(
    address: &Option<String>,
    calls: &channel::Sender<Call>,
    settings: &Arc<Mutex<SessionSettings>>,
    signals_rx: &async_channel::Receiver<BusOut>,
    emitted: &mut SessionSettings,
    connection: &SessionConnection,
) -> zbus::Result<bool> {
    let iface = Session1 {
        calls: std::sync::Mutex::new(calls.clone()),
        settings: Arc::clone(settings),
    };
    let conn = crate::dbus::bus(address.as_deref())?
        .serve_at(OBJECT_PATH, iface)?
        .build()
        .await?;
    let emitter = SignalEmitter::new(&conn, OBJECT_PATH)?;
    let served = conn
        .object_server()
        .interface::<_, Session1>(OBJECT_PATH)
        .await?;
    // the screenshot service's name is followed from its owner now, before the session's name is up: a caller that
    // finds the session served is answered knowing every owner since
    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let mut owners = dbus
        .receive_name_owner_changed_with_args(&[(0, SERVICE_NAME)])
        .await?;
    let owner = screenshot_service_owner(&conn).await;
    let _ = calls.send(Call::ScreenshotServiceOwner(owner));
    conn.request_name(BUS_NAME).await?;
    *connection.lock().unwrap_or_else(|e| e.into_inner()) = Some(conn.clone());
    let _serving = Serving(connection);
    loop {
        let owner_changed =
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut owners).poll_next(cx));
        let out = match crate::dbus::either(
            signals_rx.recv(),
            crate::dbus::either(owner_changed, conn.closed()),
        )
        .await
        {
            crate::dbus::Either::First(Ok(out)) => out,
            crate::dbus::Either::First(Err(_)) => return Ok(true),
            crate::dbus::Either::Second(crate::dbus::Either::First(Some(change))) => {
                follow_screenshot_service(&dbus, change, calls).await;
                continue;
            }
            // the daemon's stream of the name's owners ends with the connection
            crate::dbus::Either::Second(crate::dbus::Either::First(None)) => return Ok(false),
            crate::dbus::Either::Second(crate::dbus::Either::Second(())) => return Ok(false),
        };
        match out {
            BusOut::ApplicationQuit(app_id) => {
                Session1::application_quit(&emitter, &app_id).await?
            }
            BusOut::QuitInterrupted { app_id, reason } => {
                Session1::quit_interrupted(&emitter, &app_id, &reason).await?
            }
            BusOut::QuitInterruptClosed => Session1::quit_interrupt_closed(&emitter).await?,
            BusOut::SessionActionFailed { action, reason } => {
                Session1::session_action_failed(&emitter, &action, &reason).await?
            }
            BusOut::Settings(now) => {
                let before = std::mem::replace(emitted, now.clone());
                let iface = served.get().await;
                if before.appearance != now.appearance {
                    iface.appearance_changed(&emitter).await?;
                }
                if before.accessibility != now.accessibility {
                    iface.accessibility_changed(&emitter).await?;
                }
                if before.mice != now.mice {
                    iface.mice_changed(&emitter).await?;
                }
            }
        }
    }
}
