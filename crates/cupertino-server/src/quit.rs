//! `org.cupertino.Session1` on the session bus: quitting the applications before a log out, which the session agent
//! (`cupertino-login`) calls between the confirmed log out and logind's `TerminateSession`, and the session settings the
//! server alone writes (the appearance and `accessibility.conf`), which the settings application sets here.
//!
//! The design quits the foreground applications one at a time: it brings the application forward so that a save
//! sheet is visible, sends it the quit event, and gives it 45 s before the interrupt panel shows; the next application
//! follows 0.1 s after one quits. The timers are the agent's; this side does the Wayland part:
//!
//! Every mutating method checks its caller ([`crate::caller`]): the process behind the connection, from the bus
//! daemon's `ProcessFD`. `QuitNextApplication` and `ForceQuitApplication` are for the session agent the server
//! supervises; `SetAppearance` and `SetAccessibility` for the settings application (a known program under `/usr`) and
//! that agent; `ListShellProcesses` and `RestartShellProcess` for any unsandboxed process of the user the server runs as
//! (the `cupertino shell` commands), by the user id the bus daemon gives for the connection. A caller without a process descriptor, a Flatpak or a Snap is refused, with an error name of its own
//! (`org.cupertino.Session1.Error.AnonymousCaller`, `.FlatpakCaller`, `.SnapCaller`; `.UntrustedCaller` for any other
//! process), and every refusal is logged. The properties are open to every caller.
//!
//! | Member | What it does |
//! |---|---|
//! | `QuitNextApplication -> (s app_id, u windows)` | activates the most recently used application that has toplevels and sends `close` to each of them (`xdg_toplevel.close`); `("", 0) when no application has a window |
//! | `ForceQuitApplication(s app_id) -> u` | sends SIGKILL to the client processes of that application's toplevels, by the rules of Force Quit; returns how many processes. Only the session agent may call it |
//! | `ApplicationQuit(s app_id) signal | the last toplevel of the application being quit went away |
//! | `SetAppearance(s appearance)` | `light` or `dark`: stored as `appearance` in `compositor.json`, then applied at once; `auto` is refused with `NotSupported` (no time-of-day or location backend), anything else with `InvalidArgs`; a failed store, or no settings directory to store in, changes nothing (`Failed`) |
//! | `SetAccessibility(s key, s value)` | one key of `accessibility.conf` with a value as the file writes it (`reduce_motion` `on`, `color_filter` `tint 0.5`, ...): applied and stored like the key actions; an unknown key or bad value is `InvalidArgs` |
//! | `ListShellProcesses -> a(ssuttsbss)` | every process the server supervises, in start order, as `(name, state, pid, start_ticks, started, program, from_dev_folder, installed, dev_program)`: `state` is `running`, `stopping`, `waiting` or `stopped`; `pid`, `start_ticks` (field 22 of `/proc/PID/stat`), `started` (seconds since 1970) are 0 and `program` empty while it does not run; `from_dev_folder` says the program is the development folder's; `installed` is the configured program and `dev_program` the development folder's file it would start, empty when the folder does not apply. Only an unsandboxed process of the session's own user may call it |
//! | `RestartShellProcess(s name) -> (ssuttsbss)` | ends the process (SIGTERM, then SIGKILL after `crate::supervisor::RESTART_STOP_GRACE`) and starts it again at once, the development folder resolved again; answers once it runs, with the same record. An unknown name is `InvalidArgs`, a start that failed `Failed`. Only an unsandboxed process of the session's own user may call it |
//! | `Appearance` property `s` | `light` or `dark`, the session's appearance (with `PropertiesChanged`) |
//! | `Accessibility` property `a{ss}` | every key of `accessibility.conf` with its value (with `PropertiesChanged`) |
//! | `Mice` property `a(ssub)` | read-only: the connected mice that have moved, scrolled or clicked (no touchpads, no keyboard's unused pointer interface), each as `(key, name, dpi, stored)`: the `mouse_dpi` key of `compositor.json` (`vendor:product:name`, hex ids), the device name, the resolution in use (the stored one, else the hardware database's, else 1000) and whether it is the stored one; changes with `PropertiesChanged` on a mouse's first use or removal and on a change of `mouse_dpi` |
//!
//! Which application goes first is approximate: the design walks its process list from the end, whose order was not
//! read; most recently used first is our choice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::channel::{self, Event};
use zbus::object_server::SignalEmitter;

use crate::authorization::ShellRole;
use crate::caller::{identify, Caller, Denial, Need, Sandbox};
use crate::state::Cupertino;
use crate::supervisor::{ProcessInfo, RestartError};
use cupertino_wm::{AppId, Command};

pub const BUS_NAME: &str = "org.cupertino.Session1";
pub const OBJECT_PATH: &str = "/org/cupertino/Session1";

/// The application being quit.
#[derive(Debug)]
struct Quitting {
    app: AppId,
    identity: String,
}

/// State of the quit sequence and the link to the bus thread.
#[derive(Debug, Default)]
pub struct Quit {
    quitting: Option<Quitting>,
    /// What the bus thread is to emit; `None` until it runs.
    pub(crate) signals: Option<async_channel::Sender<BusOut>>,
}

/// What the compositor tells the bus thread.
#[derive(Debug)]
pub enum BusOut {
    ApplicationQuit(String),
    /// The appearance, the accessibility settings or the mice changed: the values now.
    Settings(SessionSettings),
}

/// One connected mouse as the `Mice` property lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MouseEntry {
    /// `cupertino_defaults::compositor::mouse_dpi_key` of the device.
    pub key: String,
    pub name: String,
    /// The resolution in use, in dots per inch.
    pub dpi: u32,
    /// The user stored the resolution (`mouse_dpi`), as opposed to the hardware database or the default.
    pub stored: bool,
}

/// The values of the properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionSettings {
    pub appearance: String,
    pub accessibility: HashMap<String, String>,
    pub mice: Vec<MouseEntry>,
}

/// Why a settings call was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    InvalidArgs(String),
    NotSupported(String),
    Failed(String),
}

/// Why a call was not carried out.
#[derive(Debug, PartialEq, Eq)]
pub enum Reject {
    Caller(Denial),
    Refusal(Refusal),
}

/// A call that arrived on the bus, with its caller and where its answer goes.
pub enum Call {
    QuitNext(Caller, async_channel::Sender<Result<(String, u32), Reject>>),
    /// The application to force quit.
    ForceQuit(String, Caller, async_channel::Sender<Result<u32, Reject>>),
    SetAppearance(String, Caller, async_channel::Sender<Result<(), Reject>>),
    SetAccessibility(
        String,
        String,
        Caller,
        async_channel::Sender<Result<(), Reject>>,
    ),
    /// The caller and the user id the bus gave for its connection.
    ListShellProcesses(
        Caller,
        Option<u32>,
        async_channel::Sender<Result<Vec<ProcessInfo>, Reject>>,
    ),
    /// The process to restart, the caller and the user id the bus gave for its connection.
    RestartShellProcess(
        String,
        Caller,
        Option<u32>,
        async_channel::Sender<Result<ProcessInfo, Reject>>,
    ),
}

/// A supervised process on the bus: name, state, pid, start ticks, start time in seconds since 1970, program, from the
/// development folder, installed program, development folder's file (see the table above).
pub type ShellProcessRecord = (String, String, u32, u64, u64, String, bool, String, String);

/// The bus record of `info`; what is unknown is 0 or empty.
pub fn shell_process_record(info: &ProcessInfo) -> ShellProcessRecord {
    let text =
        |path: Option<&std::path::Path>| path.map(|p| p.display().to_string()).unwrap_or_default();
    let ticks = info.start_ticks.unwrap_or(0);
    (
        info.name.clone(),
        info.state.name().to_string(),
        info.pid.unwrap_or(0),
        ticks,
        info.start_ticks
            .and_then(crate::supervisor::started_unix)
            .unwrap_or(0),
        text(info.program.as_deref()),
        info.from_dev_folder,
        info.installed.display().to_string(),
        text(info.dev_program.as_deref()),
    )
}

impl Cupertino {
    /// The applications that have toplevels, most recently used first, then the others by id.
    fn apps_with_windows(&self) -> Vec<AppId> {
        let has_windows = |app: AppId| {
            self.windows
                .iter()
                .any(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
        };
        let mut apps: Vec<AppId> = self
            .wm
            .app_mru()
            .iter()
            .copied()
            .filter(|a| has_windows(*a))
            .collect();
        let mut rest: Vec<AppId> = self
            .wm
            .apps()
            .map(|a| a.id)
            .filter(|a| !apps.contains(a) && has_windows(*a))
            .collect();
        rest.sort();
        apps.extend(rest);
        apps
    }

    /// The application being quit, until its last toplevel is gone.
    pub fn quitting(&self) -> Option<&str> {
        self.quit.quitting.as_ref().map(|q| q.identity.as_str())
    }

    /// `QuitNextApplication`: bring the next application forward and ask each of its toplevels to close.
    pub fn quit_next_application(&mut self) -> (String, u32) {
        let Some(app) = self.apps_with_windows().first().copied() else {
            self.quit.quitting = None;
            return (String::new(), 0);
        };
        let identity = self
            .app_identity_of(app)
            .expect("an application with windows is known to the window model");
        self.command(Command::ActivateApp { app });
        let mut windows = 0;
        for entry in self
            .windows
            .iter()
            .filter(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
        {
            entry.close();
            windows += 1;
        }
        tracing::info!(
            app = identity,
            windows,
            "log out: asking the application to quit"
        );
        self.quit.quitting = Some(Quitting {
            app,
            identity: identity.clone(),
        });
        (identity, windows)
    }

    /// `ForceQuitApplication`: SIGKILL the processes behind the application's toplevels, by the same rules as Force Quit.
    pub fn force_quit_application(&mut self, identity: &str) -> u32 {
        let Some(app) = self.wm.app_for_identity(identity) else {
            return 0;
        };
        let processes = self.app_processes(app, identity);
        for (_, process) in &processes {
            (self.send_signal)(process, crate::force_quit::SIGKILL);
        }
        tracing::info!(
            app = identity,
            processes = processes.len(),
            "log out: force quit"
        );
        processes.len() as u32
    }

    /// Whether `exe` is the settings application: the one under an install root, or by name in a development run.
    fn is_settings_program(&self, exe: &std::path::Path) -> bool {
        if self.config.dev_shell {
            exe.file_name().is_some_and(|n| n == "cupertino-settings")
        } else {
            crate::config::installed_roots().any(|root| exe == root.join("bin/cupertino-settings"))
        }
    }

    /// Judge the caller of `method` ([`Caller::authorize`]); a refusal is logged with the caller's process and kind,
    /// never with the arguments.
    pub fn authorize_call(
        &self,
        method: &'static str,
        caller: &Caller,
        need: Need,
    ) -> Result<(), Denial> {
        let agents = self.supervisor.pids_with_role(ShellRole::SessionLock);
        let decision = crate::caller::authorize(caller, need, &agents, std::process::id(), |exe| {
            self.is_settings_program(exe)
        });
        if let Err(denial) = decision {
            tracing::warn!(
                method,
                pid = caller.pid(),
                ?denial,
                "session interface: the caller is refused"
            );
        }
        decision
    }

    /// Judge the caller of a method on the shell processes ([`crate::caller::authorize_session_user`]); a refusal is
    /// logged with the caller's process and kind.
    pub fn authorize_session_user_call(
        &self,
        method: &'static str,
        caller: &Caller,
        user: Option<u32>,
    ) -> Result<(), Denial> {
        let own = rustix::process::getuid().as_raw();
        let decision = crate::caller::authorize_session_user(caller, user, own);
        if let Err(denial) = decision {
            tracing::warn!(
                method,
                pid = caller.pid(),
                user,
                ?denial,
                "session interface: the caller is refused"
            );
        }
        decision
    }

    /// Restart the supervised process `name` ([`crate::supervisor::Supervisor::request_restart`]) and run the
    /// supervision at once, so a process that is not running starts now and one that is late to end is killed on time.
    /// `reply` gets the process once it runs, or why not.
    pub fn restart_shell_process(&mut self, name: &str, reply: crate::supervisor::RestartReply) {
        if self.socket_name.is_none() {
            reply(Err(RestartError::NotStarted(
                "the server has no socket for its shell processes".into(),
            )));
            return;
        }
        self.supervisor
            .request_restart(name, std::time::Instant::now(), reply);
        crate::cli::supervise(self);
    }

    /// `ForceQuitApplication` from the bus: only for the session agent.
    pub fn force_quit_application_for(
        &mut self,
        caller: &Caller,
        identity: &str,
    ) -> Result<u32, Denial> {
        self.authorize_call("ForceQuitApplication", caller, Need::SessionAgent)?;
        Ok(self.force_quit_application(identity))
    }

    /// Announce the end of the application being quit once its last toplevel is gone; called from [`Cupertino::tick`].
    pub fn tick_quit(&mut self) {
        let Some(q) = &self.quit.quitting else { return };
        if self
            .windows
            .iter()
            .any(|e| self.wm.window(e.id).is_some_and(|w| w.app == q.app))
        {
            return;
        }
        let identity = self.quit.quitting.take().expect("checked above").identity;
        tracing::info!(app = identity, "log out: the application quit");
        if let Some(signals) = &self.quit.signals {
            if signals.try_send(BusOut::ApplicationQuit(identity)).is_err() {
                tracing::warn!(
                    "the session bus thread has stopped: cannot announce that the application quit"
                );
            }
        }
    }

    /// The values of the session interface's properties.
    pub fn session_settings(&self) -> SessionSettings {
        let appearance = if self.config.is_dark() {
            "dark"
        } else {
            "light"
        }
        .to_string();
        let accessibility = self
            .accessibility
            .settings
            .entries()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        SessionSettings {
            appearance,
            accessibility,
            mice: self.mice(),
        }
    }

    /// The connected mice by key; a key two devices share (the nodes of one mouse, identical mice) is listed once.
    fn mice(&self) -> Vec<MouseEntry> {
        let mut mice: Vec<MouseEntry> = self
            .pointer_devices
            .values()
            .filter_map(|device| device.mouse_entry(&self.config.pointer))
            .collect();
        mice.sort_by(|a, b| a.key.cmp(&b.key));
        mice.dedup_by(|a, b| a.key == b.key);
        mice
    }

    /// Tell the session interface that the appearance or the accessibility settings changed; called by every path
    /// that changes them.
    pub fn publish_session_settings(&self) {
        if let Some(signals) = &self.quit.signals {
            if signals
                .try_send(BusOut::Settings(self.session_settings()))
                .is_err()
            {
                tracing::warn!(
                    "the session bus thread has stopped: the settings properties are not updated"
                );
            }
        }
    }

    /// `SetAppearance`: store `appearance` in `compositor.json`, then apply it.
    pub fn set_session_appearance(&mut self, appearance: &str) -> Result<(), Refusal> {
        use cupertino_defaults::compositor::{AppearanceName, CompositorSettings};
        let (name, value) = match appearance {
            "light" => (AppearanceName::Light, cupertino_theme::Appearance::Light),
            "dark" => (AppearanceName::Dark, cupertino_theme::Appearance::Dark),
            "auto" => return Err(Refusal::NotSupported("the automatic appearance needs the time of day or a location, which the session has no source for".into())),
            other => return Err(Refusal::InvalidArgs(format!("unknown appearance {other:?}: light, dark or auto"))),
        };
        match self.live.paths.compositor.clone() {
            Some(path) => {
                let (mut file, _) = CompositorSettings::load(&path, None).map_err(|e| Refusal::Failed(format!("reading {}: {e}", path.display())))?;
                file.appearance = Some(name);
                file.store(&path).map_err(|e| Refusal::Failed(format!("writing {}: {e}", path.display())))?;
                // the watcher sees the key as it is now: the change is applied here, once
                self.live.files.compositor.appearance = Some(name);
            }
            // store, then apply: what cannot be stored is not applied either
            None => return Err(Refusal::Failed("no settings directory (neither XDG_CONFIG_HOME nor HOME is set): the appearance cannot be stored".into())),
        }
        self.set_appearance(value);
        Ok(())
    }

    /// `SetAccessibility`: set one key of `accessibility.conf`, apply and store it.
    pub fn set_session_accessibility(&mut self, key: &str, value: &str) -> Result<(), Refusal> {
        let mut settings = self.accessibility.settings;
        settings.set(key, value).map_err(Refusal::InvalidArgs)?;
        self.set_accessibility(settings);
        // `screen_reader = on` again starts a reader that ended by itself
        self.sync_screen_reader();
        Ok(())
    }

    pub fn on_quit_call(&mut self, call: Call) {
        // a caller that went away before the answer is not an error of ours
        match call {
            Call::QuitNext(caller, reply) => {
                let answer = self
                    .authorize_call("QuitNextApplication", &caller, Need::SessionAgent)
                    .map(|()| self.quit_next_application())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::ForceQuit(identity, caller, reply) => {
                let answer = self
                    .force_quit_application_for(&caller, &identity)
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::SetAppearance(appearance, caller, reply) => {
                let answer = self
                    .authorize_call("SetAppearance", &caller, Need::SettingsClient)
                    .map_err(Reject::Caller)
                    .and_then(|()| {
                        self.set_session_appearance(&appearance)
                            .map_err(Reject::Refusal)
                    });
                let _ = reply.try_send(answer);
            }
            Call::ListShellProcesses(caller, user, reply) => {
                let answer = self
                    .authorize_session_user_call("ListShellProcesses", &caller, user)
                    .map(|()| self.supervisor.processes())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::RestartShellProcess(name, caller, user, reply) => {
                if let Err(denial) =
                    self.authorize_session_user_call("RestartShellProcess", &caller, user)
                {
                    let _ = reply.try_send(Err(Reject::Caller(denial)));
                    return;
                }
                self.restart_shell_process(
                    &name,
                    Box::new(move |result| {
                        let answer = result.map_err(|e| {
                            Reject::Refusal(match e {
                                RestartError::Unknown(name) => Refusal::InvalidArgs(format!(
                                    "no shell process is named {name:?}"
                                )),
                                RestartError::NotStarted(reason) => Refusal::Failed(format!(
                                    "the process ended but did not start again ({reason}); it is tried again later"
                                )),
                            })
                        });
                        let _ = reply.try_send(answer);
                    }),
                );
            }
            Call::SetAccessibility(key, value, caller, reply) => {
                let answer = self
                    .authorize_call("SetAccessibility", &caller, Need::SettingsClient)
                    .map_err(Reject::Caller)
                    .and_then(|()| {
                        self.set_session_accessibility(&key, &value)
                            .map_err(Reject::Refusal)
                    });
                let _ = reply.try_send(answer);
            }
        }
    }
}

struct Session1 {
    calls: std::sync::Mutex<channel::Sender<Call>>,
    /// What the properties read; the bus thread updates it from [`BusOut::Settings`].
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
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::QuitNext(caller, tx)).await
    }

    async fn force_quit_application(
        &self,
        app_id: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<u32, CallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::ForceQuit(app_id, caller, tx)).await
    }

    async fn set_appearance(
        &self,
        appearance: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = caller_of(&header, connection).await;
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
        let caller = caller_of(&header, connection).await;
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
    let (signals, signals_rx) = async_channel::unbounded::<BusOut>();
    state.quit.signals = Some(signals);
    let settings = Arc::new(Mutex::new(state.session_settings()));
    let spawned = std::thread::Builder::new().name("cupertino-session".into()).spawn(move || {
        let served = zbus::block_on(async {
            let iface = Session1 { calls: std::sync::Mutex::new(calls), settings: Arc::clone(&settings) };
            let conn = crate::dbus::bus(address.as_deref())?.serve_at(OBJECT_PATH, iface)?.name(BUS_NAME)?.build().await?;
            let emitter = SignalEmitter::new(&conn, OBJECT_PATH)?;
            let served = conn.object_server().interface::<_, Session1>(OBJECT_PATH).await?;
            while let Ok(out) = signals_rx.recv().await {
                match out {
                    BusOut::ApplicationQuit(app_id) => Session1::application_quit(&emitter, &app_id).await?,
                    BusOut::Settings(now) => {
                        let before = std::mem::replace(&mut *settings.lock().expect("no thread panics while holding the settings"), now.clone());
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
            Ok::<(), zbus::Error>(())
        });
        if let Err(e) = served {
            tracing::warn!(error = %e, "the session interface {BUS_NAME} is not served: a log out does not quit the applications first");
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "cannot start the session interface thread");
    }
}
