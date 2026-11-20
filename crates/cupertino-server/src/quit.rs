//! `org.cupertino.Session1` on the session bus: log out, restart, shut down and sleep (D13), quitting the applications
//! before the first three, which the session agent (`cupertino-login`) asks for once the person confirmed, and the
//! session settings the server alone writes (the appearance and `accessibility.conf`), which the settings application
//! sets here.
//!
//! The design quits the foreground applications one at a time: it brings the application forward so that a save
//! sheet is visible, sends it the quit event, and gives it 45 s before the interrupt panel shows; the next application
//! follows 0.1 s after one quits. The sequence and its timers are this side's ([`QUIT_TIMEOUT_S`],
//! [`NEXT_APPLICATION_S`]); the session agent shows the interrupt panel on `QuitInterrupted` and answers it with
//! `RetrySessionAction`, `CancelSessionAction` or `ForceQuitApplication`. Sleep quits nothing.
//!
//! Every mutating method checks its caller ([`crate::caller`]): the process behind the connection, from the bus
//! daemon's `ProcessFD`. `QuitNextApplication`, `QuitStatus`, `ForceQuitApplication` and `OpenApplications` are for the session agent the server
//! supervises; `SetAppearance` and `SetAccessibility` for the settings application (a known program under `/usr`) and
//! that agent; `ListShellProcesses` and `RestartShellProcess` for any unsandboxed process of the user the server runs as
//! (the `cupertino shell` commands), by the user id the bus daemon gives for the connection. A caller without a process descriptor, a Flatpak or a Snap is refused, with an error name of its own
//! (`org.cupertino.Session1.Error.AnonymousCaller`, `.FlatpakCaller`, `.SnapCaller`; `.UntrustedCaller` for any other
//! process), and every refusal is logged. The properties are open to every caller.
//!
//! | Member | What it does |
//! |---|---|
//! | `LogOut`, `Restart`, `ShutDown` | quit the applications one at a time (the most recently used first, 45 s each, the next 0.1 s after one quits), then end the session through `cupertino-sessiond` (`LoginManager1.LogOut`, logind's `TerminateSession` when it does not answer) or ask logind to `Reboot` or `PowerOff`. Refused (`Failed`) while the session is locked |
//! | `Sleep` | logind's `Suspend` at once: no application quits |
//! | `OpenApplications -> as` | the identities of the applications that have toplevels, in the order a log out would quit them: what the session agent saves for "Open my windows again" before it asks for the log out. Only the session agent may call it |
//! | `QuitInterrupted(s app_id, s reason)` signal | an application did not quit within 45 s, with its `QuitStatus` as the reason; the sequence waits for an answer |
//! | `QuitInterruptClosed` signal | the application of the last `QuitInterrupted` quit after all: the sequence goes on |
//! | `RetrySessionAction` | after `QuitInterrupted`: ask the applications to quit again, from the most recently used |
//! | `CancelSessionAction` | end the sequence: the session goes on, the applications that quit stay closed |
//! | `QuitNextApplication -> (s app_id, u windows)` | activates the most recently used application that has toplevels and sends `close` to each of them (`xdg_toplevel.close`); `("", 0) when no application has a window |
//! | `QuitStatus(s app_id) -> s` | why the application asked to quit is still there: `not-responding` (a client behind its windows does not answer), `kept-windows` (it answers and has exactly the windows it was asked to close), `closing` (it closed some or opened another), `gone` (no window left). Only the session agent may call it |
//! | `ForceQuitApplication(s app_id) -> u` | sends SIGKILL to the client processes of that application's toplevels, by the rules of Force Quit; returns how many processes. The application whose `QuitInterrupted` is answered this way gets another 45 s to go. Only the session agent may call it |
//! | `ApplicationQuit(s app_id) signal | the last toplevel of the application being quit went away |
//! | `SetAppearance(s appearance)` | `light` or `dark`: stored as `appearance` in `compositor.json`, then applied at once; `auto` is refused with `NotSupported` (no time-of-day or location backend), anything else with `InvalidArgs`; a failed store, or no settings directory to store in, changes nothing (`Failed`) |
//! | `SetAccessibility(s key, s value)` | one key of `accessibility.conf` with a value as the file writes it (`reduce_motion` `on`, `color_filter` `tint 0.5`, ...): applied and stored like the key actions; an unknown key or bad value is `InvalidArgs` |
//! | `ListShellProcesses -> a(ssuttsbss)` | every process the server supervises, in start order, as `(name, state, pid, start_ticks, started, program, from_dev_folder, installed, dev_program)`: `state` is `running`, `stopping`, `waiting` or `stopped`; `pid`, `start_ticks` (field 22 of `/proc/PID/stat`), `started` (seconds since 1970) are 0 and `program` empty while it does not run; `from_dev_folder` says the program is the development folder's; `installed` is the configured program and `dev_program` the development folder's file it would start, empty when the folder does not apply. Only an unsandboxed process of the session's own user may call it |
//! | `RestartShellProcess(s name) -> (ssuttsbss)` | ends the process (SIGTERM, then SIGKILL after `crate::supervisor::RESTART_STOP_GRACE`) and starts it again at once, the development folder resolved again; answers once it runs, with the same record. A component that runs in a host is restarted through its host and answered once it is ready, with its host's record under its name; `host` and `host-NAME` name the hosts. An unknown name is `InvalidArgs`, a start that failed `Failed`. Only an unsandboxed process of the session's own user may call it |
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
use cupertino_wm::{AppId, Command, WindowId};

pub const BUS_NAME: &str = "org.cupertino.Session1";
pub const OBJECT_PATH: &str = "/org/cupertino/Session1";

/// Time an application gets to quit during a session-ending sequence before an interrupt is signalled, in seconds.
pub const QUIT_TIMEOUT_S: f64 = 45.0;
/// Pause between one application quitting and asking the next, in seconds.
pub const NEXT_APPLICATION_S: f64 = 0.1;

/// Session-ending or power action orchestrated by `org.cupertino.Session1` (D13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAction {
    /// End the user session through `cupertino-sessiond` (`org.cupertino.LoginManager1.LogOut`).
    LogOut,
    /// Reboot the machine through logind (`Reboot`).
    Restart,
    /// Power off the machine through logind (`PowerOff`).
    ShutDown,
    /// Suspend the machine through logind (`Suspend`) without quitting applications.
    Sleep,
}

/// Phase of a running session-ending application-quit sequence.
#[derive(Clone, Debug, PartialEq)]
enum SequenceStep {
    /// Ready to ask the next application with windows to quit.
    Asking,
    /// Waiting for `app` to close all its windows since `since` (model seconds).
    Waiting { app: String, since: f64 },
    /// `app` did not quit within [`QUIT_TIMEOUT_S`]; waiting for cancel, retry, force quit, or self-quit.
    Interrupted { app: String },
    /// Waiting until `at` (model seconds) before asking the next application.
    Next { at: f64 },
}

/// Active quit-applications-then-act sequence owned by `Session1`.
#[derive(Clone, Debug)]
pub struct SessionSequence {
    pub action: SessionAction,
    step: SequenceStep,
    pub quit_apps: Vec<String>,
}

/// The application being quit.
#[derive(Debug)]
struct Quitting {
    app: AppId,
    identity: String,
    /// The toplevels it was asked to close.
    asked: Vec<WindowId>,
}

/// State of the quit sequence and the link to the bus thread.
#[derive(Default)]
pub struct Quit {
    quitting: Option<Quitting>,
    /// Running session-ending sequence (`LogOut`, `Restart`, `ShutDown`) after quitting applications (D13).
    pub(crate) sequence: Option<SessionSequence>,
    /// Optional sink invoked instead of system-bus `LoginManager1`/logind calls (used by headless tests).
    pub action_sink: Option<Box<dyn FnMut(SessionAction) + Send>>,
    /// What the bus thread is to emit; `None` until it runs.
    pub(crate) signals: Option<async_channel::Sender<BusOut>>,
    /// What the session interface's properties read, shared with the bus thread; `None` until it runs.
    pub(crate) properties: Option<Arc<Mutex<SessionSettings>>>,
}

impl Quit {
    /// Where [`BusOut`] goes from now on: the bus thread emits it as signals and property changes. A later call takes
    /// the place of an earlier one.
    pub fn subscribe(&mut self) -> async_channel::Receiver<BusOut> {
        let (signals, out) = async_channel::unbounded::<BusOut>();
        self.signals = Some(signals);
        out
    }
}

impl std::fmt::Debug for Quit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Quit")
            .field("quitting", &self.quitting)
            .field("sequence", &self.sequence)
            .field("has_action_sink", &self.action_sink.is_some())
            .finish()
    }
}

/// What the compositor tells the bus thread.
#[derive(Debug)]
pub enum BusOut {
    ApplicationQuit(String),
    /// An application did not quit within [`QUIT_TIMEOUT_S`]: `(app_id, quit_status)`.
    QuitInterrupted {
        app_id: String,
        reason: String,
    },
    /// The application whose interrupt panel was shown has now quit.
    QuitInterruptClosed,
    /// The appearance, the accessibility settings or the mice changed: the values now.
    Settings(SessionSettings),
}

pub use cupertino_server_input::events::MouseEntry;

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
    /// The application whose quit status is asked.
    QuitStatus(
        String,
        Caller,
        async_channel::Sender<Result<String, Reject>>,
    ),
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
    /// Prove ownership of an announced component session-bus unique name using the Wayland nonce (D20).
    AnnounceComponent {
        sender: String,
        nonce: String,
        caller: Caller,
        reply: async_channel::Sender<Result<(), Reject>>,
    },
    /// Resolve an accepted session-bus unique name to its component name, or empty string when none (D20).
    ComponentOf(String, async_channel::Sender<Result<String, Reject>>),
    /// PIDs and start ticks of currently connected shell host processes (D20).
    ShellHosts(async_channel::Sender<Result<Vec<(u32, u64)>, Reject>>),
    /// The identities of the applications with toplevels, in quit order (`OpenApplications`).
    OpenApplications(Caller, async_channel::Sender<Result<Vec<String>, Reject>>),
    /// Start a session-ending or power action (`LogOut`, `Restart`, `ShutDown`, `Sleep`), quitting apps first (D13).
    SessionAction(
        SessionAction,
        Caller,
        async_channel::Sender<Result<(), Reject>>,
    ),
    /// Cancel an in-progress session-ending quit sequence.
    CancelSessionAction(Caller, async_channel::Sender<Result<(), Reject>>),
    /// Retry asking the next application after an interrupt during a session-ending quit sequence.
    RetrySessionAction(Caller, async_channel::Sender<Result<(), Reject>>),
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
        let mut asked = Vec::new();
        for entry in self
            .windows
            .iter()
            .filter(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
        {
            entry.close();
            asked.push(entry.id);
        }
        let windows = asked.len() as u32;
        tracing::info!(
            app = identity,
            windows,
            "log out: asking the application to quit"
        );
        self.quit.quitting = Some(Quitting {
            app,
            identity: identity.clone(),
            asked,
        });
        (identity, windows)
    }

    /// `OpenApplications`: the identities of the applications that have toplevels, in the order a log out quits them.
    pub fn open_applications(&self) -> Vec<String> {
        self.apps_with_windows()
            .into_iter()
            .filter_map(|app| self.app_identity_of(app))
            .collect()
    }

    /// `QuitStatus`: why `identity`, asked to quit, is still there. `not-responding` while a client behind its windows
    /// does not answer; `gone` when it has no window left; `kept-windows` when it still has exactly the windows it was
    /// asked to close, which is how an application that declines to quit looks; `closing` otherwise (it closed some, or
    /// opened another, such as a sheet asking to save).
    pub fn quit_status(&self, identity: &str) -> &'static str {
        if self.unresponsive_apps().contains(identity) {
            return "not-responding";
        }
        let Some(app) = self.wm.app_for_identity(identity) else {
            return "gone";
        };
        let mut open: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
            .map(|e| e.id)
            .collect();
        if open.is_empty() {
            return "gone";
        }
        let asked = self
            .quit
            .quitting
            .as_ref()
            .filter(|q| q.app == app)
            .map(|q| q.asked.clone());
        match asked {
            Some(mut asked) => {
                open.sort();
                asked.sort();
                if open == asked {
                    "kept-windows"
                } else {
                    "closing"
                }
            }
            None => "closing",
        }
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
        // the answer to its interrupt: it gets another full timeout to go away, and interrupts again if it does not
        let now = self.now();
        if let Some(seq) = &mut self.quit.sequence {
            if matches!(&seq.step, SequenceStep::Interrupted { app } if app == identity) {
                seq.step = SequenceStep::Waiting {
                    app: identity.to_owned(),
                    since: now,
                };
            }
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

    /// Restart `name` (`cupertino shell restart NAME`): a component that runs in a host through its host
    /// ([`Cupertino::restart_hosted_component`]), answered once it is ready; any other name is a supervised process
    /// ([`crate::supervisor::Supervisor::request_restart`]), answered once it runs: a configured process, a component's
    /// process entry, `host` or `host-NAME` (whose exit is then expected; `host` tries a refused host program again).
    /// The supervision runs at once, so a process that is not running starts now and one that is late to end is killed
    /// on time. `reply` gets the process, or why not.
    pub fn restart_shell_process(&mut self, name: &str, reply: crate::supervisor::RestartReply) {
        let reply = match crate::host::component_spec(name) {
            Some(spec) => match self.restart_hosted_component(spec.name, reply) {
                Some(reply) => reply,
                None => {
                    crate::cli::supervise(self);
                    return;
                }
            },
            None => reply,
        };
        if name == crate::config::BuiltIn::Host.name() && self.supervisor.host_refused().is_some() {
            self.supervisor.set_host_refused(None);
            self.apply_shell_plan();
        }
        if let Some(id) = self.supervisor.host_id_of(name) {
            self.expect_host_exit(id);
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

    /// Start a session action (`LogOut`, `Restart`, `ShutDown`, `Sleep`) per D13.
    ///
    /// `Sleep` executes immediately without quitting applications; `LogOut`, `Restart`, and `ShutDown` quit
    /// applications one by one (45 s timeout, 0.1 s between applications) and then ask `cupertino-sessiond` or
    /// logind. Fails while the session is locked.
    pub fn request_session_action(&mut self, action: SessionAction) -> Result<(), Refusal> {
        if self.session_lock.is_active() {
            return Err(Refusal::Failed("the session is locked".into()));
        }
        if action == SessionAction::Sleep {
            self.dispatch_session_action(SessionAction::Sleep);
            return Ok(());
        }
        self.quit.sequence = Some(SessionSequence {
            action,
            step: SequenceStep::Asking,
            quit_apps: Vec::new(),
        });
        self.advance_session_sequence();
        Ok(())
    }

    /// Cancel an active session-ending quit sequence so the session continues.
    pub fn cancel_session_action(&mut self) {
        self.quit.sequence = None;
        self.quit.quitting = None;
    }

    /// Retry quitting applications after an interrupt panel.
    pub fn retry_session_action(&mut self) {
        if let Some(seq) = &mut self.quit.sequence {
            if matches!(seq.step, SequenceStep::Interrupted { .. }) {
                seq.step = SequenceStep::Asking;
                self.advance_session_sequence();
            }
        }
    }

    /// Whether a session-ending quit sequence is active, and which action it targets.
    pub fn pending_session_action(&self) -> Option<SessionAction> {
        self.quit.sequence.as_ref().map(|s| s.action)
    }

    /// Carry out `action` once applications have quit (or immediately for `Sleep`). Runs system-bus IPC on a
    /// background thread so the compositor loop never blocks.
    fn dispatch_session_action(&mut self, action: SessionAction) {
        if let Some(sink) = self.quit.action_sink.as_mut() {
            sink(action);
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("session-action".into())
            .spawn(move || execute_system_session_action(action));
        if let Err(e) = spawned {
            tracing::error!(?action, error = %e, "cannot spawn session action thread");
        }
    }

    fn advance_session_sequence(&mut self) {
        let now = self.now();
        loop {
            let Some(seq) = &self.quit.sequence else {
                return;
            };
            match &seq.step {
                SequenceStep::Asking => {
                    let (app, windows) = self.quit_next_application();
                    if app.is_empty() || windows == 0 {
                        let action = self.quit.sequence.take().expect("checked above").action;
                        tracing::info!(?action, "session sequence: all applications quit");
                        self.dispatch_session_action(action);
                        return;
                    }
                    if let Some(seq) = &mut self.quit.sequence {
                        seq.quit_apps.push(app.clone());
                        seq.step = SequenceStep::Waiting { app, since: now };
                    }
                    return;
                }
                SequenceStep::Next { at } if now >= *at => {
                    if let Some(seq) = &mut self.quit.sequence {
                        seq.step = SequenceStep::Asking;
                    }
                }
                SequenceStep::Waiting { app, since } if now - since >= QUIT_TIMEOUT_S => {
                    let app = app.clone();
                    let reason = self.quit_status(&app).to_owned();
                    tracing::warn!(
                        app,
                        reason,
                        "session sequence: application did not quit within timeout"
                    );
                    if let Some(seq) = &mut self.quit.sequence {
                        seq.step = SequenceStep::Interrupted { app: app.clone() };
                    }
                    if let Some(signals) = &self.quit.signals {
                        let _ = signals.try_send(BusOut::QuitInterrupted {
                            app_id: app,
                            reason,
                        });
                    }
                    return;
                }
                _ => return,
            }
        }
    }

    /// Announce the end of the application being quit once its last toplevel is gone, and drive any active
    /// `Session1` quit sequence; called from [`Cupertino::tick`].
    pub fn tick_quit(&mut self) {
        let now = self.now();
        if let Some(q) = &self.quit.quitting {
            let still_open = self
                .windows
                .iter()
                .any(|e| self.wm.window(e.id).is_some_and(|w| w.app == q.app));
            if !still_open {
                let identity = self.quit.quitting.take().expect("checked above").identity;
                tracing::info!(app = identity, "log out: the application quit");
                if let Some(signals) = &self.quit.signals {
                    if signals
                        .try_send(BusOut::ApplicationQuit(identity.clone()))
                        .is_err()
                    {
                        tracing::warn!(
                            "the session bus thread has stopped: cannot announce that the application quit"
                        );
                    }
                }
                let mut closed_interrupt = false;
                if let Some(seq) = &mut self.quit.sequence {
                    match &seq.step {
                        SequenceStep::Waiting { app, .. } if app == &identity => {
                            seq.step = SequenceStep::Next {
                                at: now + NEXT_APPLICATION_S,
                            };
                        }
                        SequenceStep::Interrupted { app } if app == &identity => {
                            seq.step = SequenceStep::Next {
                                at: now + NEXT_APPLICATION_S,
                            };
                            closed_interrupt = true;
                        }
                        _ => {}
                    }
                }
                if closed_interrupt {
                    if let Some(signals) = &self.quit.signals {
                        let _ = signals.try_send(BusOut::QuitInterruptClosed);
                    }
                }
            }
        }
        self.advance_session_sequence();
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
    ///
    /// The properties change here, on the compositor's thread, before a setter call that changed them replies: a
    /// caller that reads a property after its call returned reads what it set. Only the change signals wait for the
    /// bus thread.
    pub fn publish_session_settings(&self) {
        let now = self.session_settings();
        if let Some(properties) = &self.quit.properties {
            *properties
                .lock()
                .expect("no thread panics while holding the settings") = now.clone();
        }
        if let Some(signals) = &self.quit.signals {
            if signals.try_send(BusOut::Settings(now)).is_err() {
                tracing::warn!(
                    "the session bus thread has stopped: the settings properties are not updated"
                );
            }
        }
    }

    /// `SetAppearance`: apply `appearance`, and store it in `compositor.json` on the settings writer's thread.
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
                // read here, so a file that cannot be read refuses the change; written after it is applied
                let (mut file, _) = CompositorSettings::load(&path, None).map_err(|e| Refusal::Failed(format!("reading {}: {e}", path.display())))?;
                file.appearance = Some(name);
                crate::store_later::store_later(move || {
                    if let Err(e) = file.store(&path) {
                        tracing::error!(path = %path.display(), error = %e, "cannot store the appearance: it lasts until the server stops");
                    }
                });
                // the watcher sees the key as it is now: the change is applied here, once
                self.live.files.compositor.appearance = Some(name);
            }
            // without a settings directory nothing is applied, as nothing could last
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
            Call::QuitStatus(identity, caller, reply) => {
                let answer = self
                    .authorize_call("QuitStatus", &caller, Need::SessionAgent)
                    .map(|()| self.quit_status(&identity).to_owned())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::OpenApplications(caller, reply) => {
                let answer = self
                    .authorize_call("OpenApplications", &caller, Need::SessionAgent)
                    .map(|()| self.open_applications())
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
                                RestartError::NotReady(reason) => Refusal::Failed(reason),
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
            Call::AnnounceComponent {
                sender,
                nonce,
                caller,
                reply,
            } => {
                let answer = self
                    .verify_component_bus_nonce(&sender, &caller, &nonce)
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::ComponentOf(unique_name, reply) => {
                let name = self
                    .hosts
                    .component_of_bus_name(&unique_name)
                    .unwrap_or("")
                    .to_owned();
                let _ = reply.try_send(Ok(name));
            }
            Call::ShellHosts(reply) => {
                let _ = reply.try_send(Ok(self.hosts.shell_host_processes()));
            }
            Call::SessionAction(action, caller, reply) => {
                let method = match action {
                    SessionAction::LogOut => "LogOut",
                    SessionAction::Restart => "Restart",
                    SessionAction::ShutDown => "ShutDown",
                    SessionAction::Sleep => "Sleep",
                };
                let answer = self
                    .authorize_call(method, &caller, Need::SettingsClient)
                    .map_err(Reject::Caller)
                    .and_then(|()| self.request_session_action(action).map_err(Reject::Refusal));
                let _ = reply.try_send(answer);
            }
            Call::CancelSessionAction(caller, reply) => {
                let answer = self
                    .authorize_call("CancelSessionAction", &caller, Need::SettingsClient)
                    .map(|()| self.cancel_session_action())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::RetrySessionAction(caller, reply) => {
                let answer = self
                    .authorize_call("RetrySessionAction", &caller, Need::SettingsClient)
                    .map(|()| self.retry_session_action())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
        }
    }
}

/// System-bus execution of a completed `Session1` action (D13): `LogOut` asks `cupertino-sessiond`
/// (`org.cupertino.LoginManager1.LogOut`, falling back to logind `TerminateSession`), and `Restart`, `ShutDown`,
/// `Sleep` ask `org.freedesktop.login1.Manager`.
fn execute_system_session_action(action: SessionAction) {
    let conn = match zbus::blocking::Connection::system() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(?action, error = %e, "session action: cannot connect to system bus");
            return;
        }
    };
    match action {
        SessionAction::LogOut => {
            let lm = conn.call_method(
                Some("org.cupertino.LoginManager1"),
                "/org/cupertino/LoginManager1",
                Some("org.cupertino.LoginManager1"),
                "LogOut",
                &(),
            );
            if let Err(e) = lm {
                tracing::warn!(
                    error = %e,
                    "session action: LoginManager1.LogOut failed, falling back to logind TerminateSession"
                );
                if let Ok(session_id) = std::env::var("XDG_SESSION_ID") {
                    if let Err(err) = conn.call_method(
                        Some("org.freedesktop.login1"),
                        "/org/freedesktop/login1",
                        Some("org.freedesktop.login1.Manager"),
                        "TerminateSession",
                        &(session_id,),
                    ) {
                        tracing::error!(error = %err, "session action: logind TerminateSession failed");
                    }
                }
            }
        }
        SessionAction::Restart | SessionAction::ShutDown | SessionAction::Sleep => {
            let method = match action {
                SessionAction::Restart => "Reboot",
                SessionAction::ShutDown => "PowerOff",
                SessionAction::Sleep => "Suspend",
                SessionAction::LogOut => unreachable!(),
            };
            if let Err(e) = conn.call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                method,
                &(false,),
            ) {
                tracing::error!(?action, method, error = %e, "session action: logind call failed");
            }
        }
    }
}

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

    async fn quit_status(
        &self,
        app_id: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<String, CallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::QuitStatus(app_id, caller, tx)).await
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

    async fn open_applications(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<Vec<String>, CallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::OpenApplications(caller, tx)).await
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

    async fn log_out(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::SessionAction(SessionAction::LogOut, caller, tx))
            .await
    }

    async fn restart(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::SessionAction(SessionAction::Restart, caller, tx))
            .await
    }

    async fn shut_down(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::SessionAction(SessionAction::ShutDown, caller, tx))
            .await
    }

    async fn sleep(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::SessionAction(SessionAction::Sleep, caller, tx))
            .await
    }

    async fn cancel_session_action(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|tx| Call::CancelSessionAction(caller, tx)).await
    }

    async fn retry_session_action(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallError> {
        let caller = caller_of(&header, connection).await;
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
    let spawned = std::thread::Builder::new().name("session-service".into()).spawn(move || {
        // when the bus connection drops, connect again after a growing pause and claim the name again
        let mut pause = crate::dbus::RECONNECT_FIRST;
        let mut served_before = false;
        loop {
            let ended = zbus::block_on(serve_session(&address, &calls, &settings, &signals_rx, &mut emitted));
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

/// Serve the interface on one connection until it closes (`Ok(false)`) or nothing can send signals any more (`Ok(true)`).
async fn serve_session(
    address: &Option<String>,
    calls: &channel::Sender<Call>,
    settings: &Arc<Mutex<SessionSettings>>,
    signals_rx: &async_channel::Receiver<BusOut>,
    emitted: &mut SessionSettings,
) -> zbus::Result<bool> {
    let iface = Session1 {
        calls: std::sync::Mutex::new(calls.clone()),
        settings: Arc::clone(settings),
    };
    let conn = crate::dbus::bus(address.as_deref())?
        .serve_at(OBJECT_PATH, iface)?
        .name(BUS_NAME)?
        .build()
        .await?;
    let emitter = SignalEmitter::new(&conn, OBJECT_PATH)?;
    let served = conn
        .object_server()
        .interface::<_, Session1>(OBJECT_PATH)
        .await?;
    loop {
        let out = match crate::dbus::either(signals_rx.recv(), conn.closed()).await {
            crate::dbus::Either::First(Ok(out)) => out,
            crate::dbus::Either::First(Err(_)) => return Ok(true),
            crate::dbus::Either::Second(()) => return Ok(false),
        };
        match out {
            BusOut::ApplicationQuit(app_id) => {
                Session1::application_quit(&emitter, &app_id).await?
            }
            BusOut::QuitInterrupted { app_id, reason } => {
                Session1::quit_interrupted(&emitter, &app_id, &reason).await?
            }
            BusOut::QuitInterruptClosed => Session1::quit_interrupt_closed(&emitter).await?,
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
