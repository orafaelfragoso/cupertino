//! `org.cupertino.Session1` on the session bus: log out, restart, shut down and sleep (D13), quitting the applications
//! before the first three, which the session agent (the shell host's `agent` component) asks for once the person
//! confirmed, the switch to the login window, and the
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
//! daemon's `ProcessFD`. `QuitNextApplication`, `QuitStatus`, `ForceQuitApplication`, `OpenApplications` and `PendingInterrupt` are for the session agent, known
//! by the connection the call comes on: the bus name the server accepted for the `agent` component (D20), never by its
//! process, which runs other components too; `SetAppearance` and `SetAccessibility` for the settings application (a known program under `/usr`) and
//! that agent; `ListShellProcesses`, `ListShellComponents` and `RestartShellProcess` for any unsandboxed process of the user the server runs as
//! (the `cupertino shell` commands), by the user id the bus daemon gives for the connection; `StartScreenshot` for the
//! portal the server started, known by its process ([`Cupertino::program_of`]). A caller without a process descriptor, a Flatpak or a Snap is refused, with an error name of its own
//! (`org.cupertino.Session1.Error.AnonymousCaller`, `.FlatpakCaller`, `.SnapCaller`; `.UntrustedCaller` for any other
//! process), and every refusal is logged. The properties are open to every caller.
//!
//! | Member | What it does |
//! |---|---|
//! | `LogOut`, `Restart`, `ShutDown` | quit the applications one at a time (the most recently used first, 45 s each, the next 0.1 s after one quits), then end the session through `cupertino-sessiond` (`LoginManager1.LogOut`, logind's `TerminateSession` when it does not answer) or ask logind to `Reboot` or `PowerOff`. Refused (`Failed`) while the session is locked |
//! | `Sleep` | logind's `Suspend` at once: no application quits |
//! | `SwitchToLoginWindow` | lock the session, then once the lock screen covers it ask `cupertino-sessiond` to give the console to the login screen (`LoginManager1.SwitchToLoginWindow` on the system bus, off the compositor's thread); a session already locked switches at once. For the settings application and the session agent |
//! | `OpenApplications -> as` | the identities of the applications that have toplevels, in the order a log out would quit them: what the session agent saves for "Open my windows again" before it asks for the log out. Only the session agent may call it |
//! | `PendingInterrupt -> (b pending, s action, s app_id, s reason)` | whether a quit sequence waits on an interrupt (the one signal of `QuitInterrupted` is sent once): `pending`, the interrupted action's method name (`LogOut`, `Restart` or `ShutDown`), the application and its `QuitStatus`; all empty and false otherwise. A session agent that restarted, or adopts an interrupt it did not start, shows the panel from it. Only the session agent may call it |
//! | `QuitInterrupted(s app_id, s reason)` signal | an application did not quit within 45 s, with its `QuitStatus` as the reason; the sequence waits for an answer |
//! | `SessionActionFailed(s action, s reason)` signal | the final step of `LogOut`, `Restart`, `ShutDown` or `Sleep` was refused or failed (another user logged in, an inhibitor, polkit denied it, no bus): `action` is the method name, `reason` the error. Sent once per failed action and only by the server; Session1 is idle again, so a later request works |
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
//! | `ListShellComponents -> a(ssbb)` | every component of the server's component table, in its order, as `(name, runs_in, from_dev_folder, connected)`: `runs_in` is `host` (the shared host), `host-NAME` (the separate host of that component), `process` (no host runs it: its process entry of the configuration does), `none` (no host runs it and it has no process entry) or `off` (the session does not run it: not in the component list, turned off, or the login screen); `from_dev_folder` says a separate host starts from the development folder; `connected` that a host holds the connection the server handed out for it. Only an unsandboxed process of the session's own user may call it |
//! | `RestartShellProcess(s name) -> (ssuttsbss)` | ends the process (SIGTERM, then SIGKILL after `crate::supervisor::RESTART_STOP_GRACE`) and starts it again at once, the development folder resolved again; answers once it runs, with the same record. A component that runs in a host is restarted through its host and answered once it is ready, with its host's record under its name; `host` and `host-NAME` name the hosts. An unknown name is `InvalidArgs`, a start that failed `Failed`. Only an unsandboxed process of the session's own user may call it |
//! | `StartScreenshot` | starts the screenshot program as the portal's service: the `screenshot` key's command with `--serve`, on a connection the server makes with the `session` and `capture` roles ([`crate::capture::launch`]), unless one it started so still runs and has not dropped that name (one going away drops the name before its exit is seen); returns once it runs, and the portal then waits for `org.cupertino.Screenshot1` on the bus. `Failed` on the login screen or when it cannot start. Only the portal the server started may call it |
//! | `ProgramOf(s unique_name) -> s` | the program the server started itself that owns the connection `unique_name`, by the server's own name for it (`portal`, `login`, `desktop-control`, `polkit-agent`, `keyring-prompter`, `screenshot`): the connection's process, as the bus daemon gives it, is one the server holds the pidfd of and it has not exited ([`Cupertino::program_of`]); empty for any other connection, a host's and its components' included, and for a name that is not a unique name. Open to every caller: it tells which connection is which program, and nothing else |
//! | `Appearance` property `s` | `light` or `dark`, the session's appearance (with `PropertiesChanged`) |
//! | `Accessibility` property `a{ss}` | every key of `accessibility.conf` with its value (with `PropertiesChanged`) |
//! | `Mice` property `a(ssub)` | read-only: the connected mice that have moved, scrolled or clicked (no touchpads, no keyboard's unused pointer interface), each as `(key, name, dpi, stored)`: the `mouse_dpi` key of `compositor.json` (`vendor:product:name`, hex ids), the device name, the resolution in use (the stored one, else the hardware database's, else 1000) and whether it is the stored one; changes with `PropertiesChanged` on a mouse's first use or removal and on a change of `mouse_dpi` |
//!
//! Which application goes first is approximate: the design walks its process list from the end, whose order was not
//! read; most recently used first is our choice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::caller::{Asker, Caller, Denial, Need};
use crate::config::BuiltIn;
use crate::host::HostKind;
use crate::state::Cupertino;
use crate::supervisor::{host_name, ProcessInfo, RestartError};
use cupertino_peer::ProcessHandle;
use cupertino_wm::{AppId, Command, WindowId};

mod service;

pub use service::{start_session_service, start_session_service_on};

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

impl SessionAction {
    /// The name of the method that asks for it, and what `SessionActionFailed` carries.
    pub fn method_name(self) -> &'static str {
        match self {
            SessionAction::LogOut => "LogOut",
            SessionAction::Restart => "Restart",
            SessionAction::ShutDown => "ShutDown",
            SessionAction::Sleep => "Sleep",
        }
    }
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

/// A stand-in for the final step of an action that answers whether it worked.
pub type ActionResultSink = Box<dyn FnMut(SessionAction) -> Result<(), String> + Send>;

/// State of the quit sequence and the link to the bus thread.
#[derive(Default)]
pub struct Quit {
    quitting: Option<Quitting>,
    /// Running session-ending sequence (`LogOut`, `Restart`, `ShutDown`) after quitting applications (D13).
    pub(crate) sequence: Option<SessionSequence>,
    /// Optional sink invoked instead of system-bus `LoginManager1`/logind calls (used by headless tests).
    pub action_sink: Option<Box<dyn FnMut(SessionAction) + Send>>,
    /// Like `action_sink`, but answers whether the final step worked; an `Err` is signalled as `SessionActionFailed`
    /// (used by headless tests in place of logind). Tried before `action_sink`.
    pub action_result_sink: Option<ActionResultSink>,
    /// What the bus thread is to emit; `None` until it runs.
    pub(crate) signals: Option<async_channel::Sender<BusOut>>,
    /// What the session interface's properties read, shared with the bus thread; `None` until it runs.
    pub(crate) properties: Option<Arc<Mutex<SessionSettings>>>,
    /// The connection that owns [`BUS_NAME`] while the bus thread serves it.
    pub(crate) connection: SessionConnection,
    /// The calls on the session agent waiting to go out on that connection ([`crate::session_agent`]).
    pub(crate) agent_calls: Option<crate::session_agent::AgentCalls>,
    /// A switch to the login window waits for the lock screen to cover the session.
    pub(crate) switch_pending: bool,
}

/// The connection that owns [`BUS_NAME`], shared with the bus thread: `None` while the name is not served. The
/// server's calls that a shell component takes only from the server go out on it (D20: the owner of the name is the
/// server, and no other connection of its process is).
pub type SessionConnection = Arc<Mutex<Option<zbus::Connection>>>;

/// Empties the shared [`SessionConnection`] when the connection it holds stops serving.
struct Serving<'a>(&'a SessionConnection);

impl Drop for Serving<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl Quit {
    /// The connection that owns [`BUS_NAME`] now; `None` while it is not served.
    pub fn session_connection(&self) -> Option<zbus::Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

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
    /// The final step of a session action was refused or failed: `action` is [`SessionAction::method_name`].
    SessionActionFailed {
        action: String,
        reason: String,
    },
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
    QuitNext(Asker, async_channel::Sender<Result<(String, u32), Reject>>),
    /// The application whose quit status is asked.
    QuitStatus(String, Asker, async_channel::Sender<Result<String, Reject>>),
    /// The application to force quit.
    ForceQuit(String, Asker, async_channel::Sender<Result<u32, Reject>>),
    SetAppearance(String, Asker, async_channel::Sender<Result<(), Reject>>),
    SetAccessibility(
        String,
        String,
        Asker,
        async_channel::Sender<Result<(), Reject>>,
    ),
    /// The caller and the user id the bus gave for its connection.
    ListShellProcesses(
        Caller,
        Option<u32>,
        async_channel::Sender<Result<Vec<ProcessInfo>, Reject>>,
    ),
    /// The caller and the user id the bus gave for its connection.
    ListShellComponents(
        Caller,
        Option<u32>,
        async_channel::Sender<Result<Vec<ShellComponentRecord>, Reject>>,
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
    OpenApplications(Asker, async_channel::Sender<Result<Vec<String>, Reject>>),
    /// The quit sequence's waiting interrupt, if any (`PendingInterrupt`): action, application, its quit status.
    PendingInterrupt(
        Asker,
        async_channel::Sender<Result<Option<PendingInterrupt>, Reject>>,
    ),
    /// Start a session-ending or power action (`LogOut`, `Restart`, `ShutDown`, `Sleep`), quitting apps first (D13).
    SessionAction(
        SessionAction,
        Asker,
        async_channel::Sender<Result<(), Reject>>,
    ),
    /// Cancel an in-progress session-ending quit sequence.
    CancelSessionAction(Asker, async_channel::Sender<Result<(), Reject>>),
    /// Lock, then hand the console to the login screen (`SwitchToLoginWindow`).
    SwitchToLoginWindow(Asker, async_channel::Sender<Result<(), Reject>>),
    /// Retry asking the next application after an interrupt during a session-ending quit sequence.
    RetrySessionAction(Asker, async_channel::Sender<Result<(), Reject>>),
    /// Start the screenshot program as the portal's service (`StartScreenshot`): the caller, and the process that owns
    /// the service's bus name when the call came (`crate::capture::launch::SERVICE_NAME`), none when no process the
    /// bus names does.
    StartScreenshot(
        Caller,
        Option<ProcessHandle>,
        async_channel::Sender<Result<(), Reject>>,
    ),
    /// The service's bus name has a new owner, the process the bus names for it, or none
    /// ([`Cupertino::screenshot_service_owner`]).
    ScreenshotServiceOwner(Option<ProcessHandle>),
    /// The program the server started itself whose process owns a bus connection, empty for any other (`ProgramOf`):
    /// the connection's process as the bus daemon gives it.
    ProgramOf(Caller, async_channel::Sender<Result<String, Reject>>),
}

/// An interrupt of a quit sequence that waits for an answer: the interrupted action's method name, the application
/// that did not quit and its `QuitStatus`.
pub type PendingInterrupt = (&'static str, String, String);

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

/// A component of the server's table as `ListShellComponents` sends it: name, where it runs, whether its separate host
/// starts from the development folder, whether it is connected.
pub type ShellComponentRecord = (String, String, bool, bool);

impl Cupertino {
    /// Where every component of the server's table runs and whether it is connected, in the table's order
    /// (`ListShellComponents`): the host the last plan puts it in, else the host the server was handed that lists it, else
    /// its supervised process entry; a component the session does not run is `off`.
    pub fn shell_components(&self) -> Vec<ShellComponentRecord> {
        let active = self.config.active_shell_components(self.login_screen);
        let attached = self.hosts.host_views();
        crate::host::COMPONENTS
            .iter()
            .map(|spec| {
                let name = spec.name;
                let host = self.hosts.planned_host(name).or_else(|| {
                    attached
                        .iter()
                        .find(|h| h.components.contains(&name))
                        .map(|h| h.kind)
                });
                let (runs_in, from_dev_folder) = match host {
                    Some(kind) => (
                        host_name(kind),
                        matches!(kind, HostKind::Separate(_))
                            && self
                                .supervisor
                                .host_info(kind)
                                .is_some_and(|p| p.dev_program.is_some()),
                    ),
                    None if !active.contains(&name) => ("off".to_owned(), false),
                    None if self.supervisor.processes().iter().any(|p| p.name == name) => {
                        ("process".to_owned(), false)
                    }
                    None => ("none".to_owned(), false),
                };
                (
                    name.to_owned(),
                    runs_in,
                    from_dev_folder,
                    self.hosts.component_connected(name),
                )
            })
            .collect()
    }

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

    /// `PendingInterrupt`: the interrupt the quit sequence waits on, if it is waiting on one.
    pub fn pending_interrupt(&self) -> Option<PendingInterrupt> {
        let seq = self.quit.sequence.as_ref()?;
        let SequenceStep::Interrupted { app } = &seq.step else {
            return None;
        };
        Some((
            seq.action.method_name(),
            app.clone(),
            self.quit_status(app).to_owned(),
        ))
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

    /// Judge the caller of `method` ([`crate::caller::authorize`]) when it is not the session agent's connection; a
    /// refusal is logged with the caller's process and kind, never with the arguments.
    pub fn authorize_call(
        &self,
        method: &'static str,
        caller: &Caller,
        need: Need,
    ) -> Result<(), Denial> {
        self.authorize_from(method, caller, false, need)
    }

    /// Judge the caller of `method` as [`Self::authorize_call`] does, the session agent known by the connection the
    /// call came on: the bus name the server accepted for the `agent` component.
    pub fn authorize_asker(
        &self,
        method: &'static str,
        asker: &Asker,
        need: Need,
    ) -> Result<(), Denial> {
        let agent = asker
            .sender
            .as_deref()
            .and_then(|sender| self.hosts.component_of_bus_name(sender))
            == Some(crate::session_agent::COMPONENT);
        self.authorize_from(method, &asker.caller, agent, need)
    }

    fn authorize_from(
        &self,
        method: &'static str,
        caller: &Caller,
        agent: bool,
        need: Need,
    ) -> Result<(), Denial> {
        let decision = crate::caller::authorize(caller, need, agent, std::process::id(), |exe| {
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

    /// The program the server started itself whose process `caller` is, by the server's own name for it (`portal`,
    /// `login`, `desktop-control`, `polkit-agent`, `keyring-prompter`, `screenshot`): the process of a running
    /// [`BuiltIn`] the supervisor has not reaped, or of a screenshot process the server holds the pidfd of. `None` for any
    /// other process (a host and its components, a configured shell process, an application), for one that has exited,
    /// and for an anonymous caller. A caller whose sandbox `/proc` would not tell is still named by its process.
    pub fn program_of(&self, caller: &Caller) -> Option<&str> {
        let process = match caller {
            Caller::Process(identified) => &identified.process,
            Caller::Unclassified(process) => process,
            Caller::Anonymous => return None,
        };
        if process.has_exited() {
            return None;
        }
        self.supervisor.built_in_of(process.pid).or_else(|| {
            self.capture
                .launched
                .holds(process)
                .then_some(crate::capture::launch::PROGRAM)
        })
    }

    /// Judge the caller of `method`, which only the program the server started as `program` may call
    /// ([`Cupertino::program_of`]); a refusal is logged with the caller's process and kind, never with the arguments.
    pub fn authorize_program_call(
        &self,
        method: &'static str,
        caller: &Caller,
        program: &str,
    ) -> Result<(), Denial> {
        if self.program_of(caller) == Some(program) {
            return Ok(());
        }
        let denial = caller.unsandboxed().err().unwrap_or(Denial::Untrusted);
        tracing::warn!(
            method,
            pid = caller.pid(),
            ?denial,
            "session interface: the caller is refused"
        );
        Err(denial)
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
        asker: &Asker,
        identity: &str,
    ) -> Result<u32, Denial> {
        self.authorize_asker("ForceQuitApplication", asker, Need::SessionAgent)?;
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
        if let Some(sink) = self.quit.action_result_sink.as_mut() {
            let result = sink(action);
            signal_action_result(self.quit.signals.as_ref(), action, result);
            return;
        }
        if let Some(sink) = self.quit.action_sink.as_mut() {
            sink(action);
            return;
        }
        let signals = self.quit.signals.clone();
        let spawned = std::thread::Builder::new()
            .name("session-action".into())
            .spawn(move || {
                let result = execute_system_session_action(action);
                signal_action_result(signals.as_ref(), action, result);
            });
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
                    .authorize_asker("QuitNextApplication", &caller, Need::SessionAgent)
                    .map(|()| self.quit_next_application())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::QuitStatus(identity, caller, reply) => {
                let answer = self
                    .authorize_asker("QuitStatus", &caller, Need::SessionAgent)
                    .map(|()| self.quit_status(&identity).to_owned())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::OpenApplications(caller, reply) => {
                let answer = self
                    .authorize_asker("OpenApplications", &caller, Need::SessionAgent)
                    .map(|()| self.open_applications())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::PendingInterrupt(caller, reply) => {
                let answer = self
                    .authorize_asker("PendingInterrupt", &caller, Need::SessionAgent)
                    .map(|()| self.pending_interrupt())
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
                    .authorize_asker("SetAppearance", &caller, Need::SettingsClient)
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
            Call::ListShellComponents(caller, user, reply) => {
                let answer = self
                    .authorize_session_user_call("ListShellComponents", &caller, user)
                    .map(|()| self.shell_components())
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
                    .authorize_asker("SetAccessibility", &caller, Need::SettingsClient)
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
                let answer = self
                    .authorize_asker(action.method_name(), &caller, Need::SettingsClient)
                    .map_err(Reject::Caller)
                    .and_then(|()| self.request_session_action(action).map_err(Reject::Refusal));
                let _ = reply.try_send(answer);
            }
            Call::CancelSessionAction(caller, reply) => {
                let answer = self
                    .authorize_asker("CancelSessionAction", &caller, Need::SettingsClient)
                    .map(|()| self.cancel_session_action())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::SwitchToLoginWindow(caller, reply) => {
                let answer = self
                    .authorize_asker("SwitchToLoginWindow", &caller, Need::SettingsClient)
                    .map(|()| self.switch_to_login_window())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::RetrySessionAction(caller, reply) => {
                let answer = self
                    .authorize_asker("RetrySessionAction", &caller, Need::SettingsClient)
                    .map(|()| self.retry_session_action())
                    .map_err(Reject::Caller);
                let _ = reply.try_send(answer);
            }
            Call::ProgramOf(process, reply) => {
                let name = self.program_of(&process).unwrap_or("").to_owned();
                let _ = reply.try_send(Ok(name));
            }
            Call::StartScreenshot(caller, owner, reply) => {
                let answer = self
                    .authorize_program_call("StartScreenshot", &caller, BuiltIn::Portal.name())
                    .map_err(Reject::Caller)
                    .and_then(|()| {
                        self.start_serving_screenshot(owner.as_ref())
                            .map_err(|e| Reject::Refusal(Refusal::Failed(e)))
                    });
                let _ = reply.try_send(answer);
            }
            Call::ScreenshotServiceOwner(owner) => self.screenshot_service_owner(owner.as_ref()),
        }
    }
}

/// Log a failed final step of a session action and tell the session agent (`SessionActionFailed`); nothing for a success.
fn signal_action_result(
    signals: Option<&async_channel::Sender<BusOut>>,
    action: SessionAction,
    result: Result<(), String>,
) {
    let Err(reason) = result else { return };
    tracing::error!(?action, reason, "session action: refused or failed");
    if let Some(signals) = signals {
        let _ = signals.try_send(BusOut::SessionActionFailed {
            action: action.method_name().to_owned(),
            reason,
        });
    }
}

/// System-bus execution of a completed `Session1` action (D13): `LogOut` asks `cupertino-sessiond`
/// (`org.cupertino.LoginManager1.LogOut`, falling back to logind `TerminateSession`), and `Restart`, `ShutDown`,
/// `Sleep` ask `org.freedesktop.login1.Manager`.
/// The error text says why it did not happen.
fn execute_system_session_action(action: SessionAction) -> Result<(), String> {
    let conn = zbus::blocking::Connection::system()
        .map_err(|e| format!("cannot connect to the system bus: {e}"))?;
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
                let session_id = std::env::var("XDG_SESSION_ID")
                    .map_err(|_| format!("{e}; and the session id is not known"))?;
                conn.call_method(
                    Some("org.freedesktop.login1"),
                    "/org/freedesktop/login1",
                    Some("org.freedesktop.login1.Manager"),
                    "TerminateSession",
                    &(session_id,),
                )
                .map_err(|err| format!("{e}; logind TerminateSession: {err}"))?;
            }
            Ok(())
        }
        SessionAction::Restart | SessionAction::ShutDown | SessionAction::Sleep => {
            let method = match action {
                SessionAction::Restart => "Reboot",
                SessionAction::ShutDown => "PowerOff",
                SessionAction::Sleep => "Suspend",
                SessionAction::LogOut => unreachable!(),
            };
            conn.call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                method,
                &(false,),
            )
            .map(drop)
            .map_err(|e| format!("logind {method}: {e}"))
        }
    }
}
