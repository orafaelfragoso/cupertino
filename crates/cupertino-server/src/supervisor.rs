//! Supervision of the shell processes named in the configuration and of the built-in ones ([`BuiltIn`], their program checked before every start): started with the server,
//! restarted by [`crate::restart_policy`], and killed when the server goes away. A pass of
//! [`Supervisor::poll`] runs when a child's pidfd becomes readable (it exited) or at the earliest
//! restart ([`Supervisor::next_deadline`]), see [`Wakeups`]; `try_wait` reaps it there. The server
//! installs no SIGCHLD handler, and nothing wakes the loop while every child runs.
//!
//! A process the development folder applies to ([`Supervisor::set_dev_folder`]) starts the folder's file of its
//! program's name at every start when [`dev_folder_program`] accepts it, else its installed program. A restart asked
//! for by name ([`Supervisor::request_restart`]) ends the process with SIGTERM, then SIGKILL after
//! [`RESTART_STOP_GRACE`], and starts it again at once, whatever its exit status; the request is answered once it runs.
//!
//! The checks of a program before a start, and the reading of a started process's start time, run on threads the loop
//! waits for until a deadline ([`crate::process::CHECK_WAIT`]); a check that does not answer by then refuses the program.
//! The process itself is spawned from the loop's thread, whose end kills it ([`end_with_server`]).
//!
//! The shell's hosts are supervised processes too (`docs/architecture/shell-host.md`, *Programs the server starts
//! itself*): `host`, the shared host `cupertino-shell`, and `host-NAME`, a separate host `cupertino-shell --only NAME`,
//! each holding the `host` role alone on a connection the [`Launcher`] makes ([`HostKind`]). A host is restarted
//! whatever its exit status, never sooner than the throttle after its last start ([`decide_host`]), and never given up;
//! an exit the server expected ([`Launcher::host_ended`]) is not counted and restarts it at once. A host whose program is
//! refused or cannot be spawned is not tried again: the server runs its components as their own processes
//! ([`Supervisor::host_refused`]) until `cupertino shell restart host` or a change of the `shell` section.
//! [`Supervisor::reconcile`] adds and ends the configured processes and the hosts as the component list says; an ended
//! one is removed once it exited.
//!
//! Each exit and each start that fails is also a diagnostic event: `server.shell.exited` (restarted),
//! `server.shell.gave_up` (not restarted) and `server.shell.start_failed`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::{self as diag, Severity, Value};

use crate::config::{check_built_in, dev_folder_program, BuiltIn, DevFolder, ShellProcess};
use crate::host::HostKind;
use crate::restart_policy::{decide, decide_host, with_exit_status, Backoff, Decision};
use crate::shell_connection::{bind_holder, hand_over, BoundConnection};
use crate::xwayland::XDisplay;

/// How long a process asked to end for a restart has after SIGTERM before it is killed.
pub const RESTART_STOP_GRACE: Duration = Duration::from_secs(5);

/// The program of the shell's hosts (`cupertino-shell`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostProgram {
    pub path: PathBuf,
    /// Arguments before the host's own (`--only NAME`); none for an installed host.
    pub args: Vec<String>,
    /// Checked before every start as a built-in program is ([`check_built_in`]); a program found on `PATH` for
    /// `--dev-shell` is not.
    pub checked: bool,
}

impl HostProgram {
    /// The supervised process of a host of `kind`: `host` runs the program, `host-NAME` runs it with `--only NAME`;
    /// both hold the `host` role alone.
    pub fn process(&self, kind: HostKind) -> ShellProcess {
        let mut command = vec![self.path.display().to_string()];
        command.extend(self.args.iter().cloned());
        if let HostKind::Separate(component) = kind {
            command.extend(["--only".to_string(), component.to_string()]);
        }
        let name = host_name(kind);
        ShellProcess {
            name,
            command,
            roles: vec![crate::authorization::ShellRole::Host],
        }
    }
}

/// The name of the supervised host of `kind`: `host`, or `host-NAME` for the separate host of component NAME.
pub fn host_name(kind: HostKind) -> String {
    match kind {
        HostKind::Shared => BuiltIn::Host.name().to_string(),
        HostKind::Separate(component) => format!("{}-{component}", BuiltIn::Host.name()),
    }
}

/// What the supervision needs from the server to start its processes and to follow its hosts.
pub trait Launcher {
    /// The connection of a shell process, holding `spec.roles` and no other.
    fn connect(&mut self, spec: &ShellProcess) -> std::io::Result<BoundConnection>;
    /// The client end of a new connection for a host of `kind`, and the id the server gave that host.
    fn connect_host(
        &mut self,
        kind: HostKind,
    ) -> std::io::Result<(std::os::unix::net::UnixStream, u64)>;
    /// The host `host_id` was started as `pid`, from the development folder or not: its connection is bound to the
    /// process. An error ends the process.
    fn host_started(
        &mut self,
        host_id: u64,
        pid: u32,
        from_dev_folder: bool,
    ) -> std::io::Result<()>;
    /// The process of host `host_id` exited with `status`; whether the server expected the exit, which is then not
    /// counted.
    fn host_ended(&mut self, host_id: u64, status: ExitStatus, from_dev_folder: bool) -> bool;
    /// The connection of host `host_id` could not be bound to its process, which is ended unbound: the host never ran,
    /// so the end of its connection is expected and counts as no crash; the next start makes another connection.
    fn host_unbound(&mut self, host_id: u64, from_dev_folder: bool);
    /// A host of `kind` could not be started for `reason` and is not tried again; `host_id` is the connection made for
    /// it, if one was.
    fn host_refused(&mut self, kind: HostKind, host_id: Option<u64>, reason: &str);
}

/// Where a supervised process comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// An entry of the configuration's `shell` section.
    Configured,
    /// A program the server starts itself ([`BuiltIn`]).
    BuiltIn,
    /// A shell host.
    Host(HostKind),
}

/// What one start ran.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Launch {
    /// The program started: the installed one, or the development folder's.
    program: PathBuf,
    from_dev_folder: bool,
    /// The process's start time in clock ticks after boot (`/proc/PID/stat`); `None` when it could not be read.
    start_ticks: Option<u64>,
}

enum Phase {
    /// Not running; start at this time.
    Waiting(Instant),
    Running {
        child: Child,
        started: Instant,
        launch: Launch,
    },
    /// Asked to end for a restart: it was sent SIGTERM and is killed at `kill_at`, `None` once it was.
    Stopping {
        child: Child,
        kill_at: Option<Instant>,
        launch: Launch,
    },
    /// Exited successfully: never restarted.
    Stopped,
}

/// Where a supervised process is, as [`ProcessInfo`] reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    /// Ending for a restart.
    Stopping,
    /// Waiting for its next start.
    Waiting,
    /// Ended successfully and not started again.
    Stopped,
}

impl ProcessState {
    /// `running`, `stopping`, `waiting` or `stopped`.
    pub fn name(self) -> &'static str {
        match self {
            ProcessState::Running => "running",
            ProcessState::Stopping => "stopping",
            ProcessState::Waiting => "waiting",
            ProcessState::Stopped => "stopped",
        }
    }
}

/// One supervised process as the server reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub name: String,
    pub state: ProcessState,
    /// While it runs (or stops).
    pub pid: Option<u32>,
    /// The start time of the process in clock ticks after boot, as field 22 of `/proc/PID/stat`.
    pub start_ticks: Option<u64>,
    /// The program that runs, while it runs.
    pub program: Option<PathBuf>,
    /// The program that runs is the development folder's.
    pub from_dev_folder: bool,
    /// The program of the configuration.
    pub installed: PathBuf,
    /// The file of the development folder the process starts when it is there; `None` when the folder does not apply.
    pub dev_program: Option<PathBuf>,
}

/// Why a restart was not done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestartError {
    /// No supervised process has this name.
    Unknown(String),
    /// The process ended but could not be started again; the supervisor keeps trying after its throttle.
    NotStarted(String),
    /// A component of a host was started again and did not say it is ready in time.
    NotReady(String),
}

/// Where the answer to a restart goes: called once, from [`Supervisor::poll`] or [`Supervisor::request_restart`].
pub type RestartReply = Box<dyn FnOnce(Result<ProcessInfo, RestartError>) + Send>;

struct Supervised {
    spec: ShellProcess,
    phase: Phase,
    backoff: Backoff,
    origin: Origin,
    /// Its program is checked again before every start ([`check_built_in`]).
    checked: bool,
    /// The server's id of the host connection of the running host.
    host_id: Option<u64>,
    /// Ended for good: removed once it exited.
    retiring: bool,
    /// Exited after it was retired: removed at the end of the pass.
    gone: bool,
    /// The development folder the process may start from; `None` when it starts its installed program alone.
    dev_folder: Option<PathBuf>,
    /// The restarts asked for and not answered yet.
    restarts: Vec<RestartReply>,
}

impl Supervised {
    fn new(spec: ShellProcess, now: Instant, origin: Origin, checked: bool) -> Self {
        Self {
            spec,
            phase: Phase::Waiting(now),
            backoff: Backoff::default(),
            origin,
            checked,
            host_id: None,
            retiring: false,
            gone: false,
            dev_folder: None,
            restarts: Vec::new(),
        }
    }

    fn host(&self) -> Option<HostKind> {
        match self.origin {
            Origin::Host(kind) => Some(kind),
            Origin::Configured | Origin::BuiltIn => None,
        }
    }

    fn info(&self) -> ProcessInfo {
        let installed = PathBuf::from(&self.spec.command[0]);
        let dev_program = self
            .dev_folder
            .as_ref()
            .and_then(|folder| Some(folder.join(installed.file_name()?)));
        let (state, running) = match &self.phase {
            Phase::Running { child, launch, .. } => (ProcessState::Running, Some((child, launch))),
            Phase::Stopping { child, launch, .. } => {
                (ProcessState::Stopping, Some((child, launch)))
            }
            Phase::Waiting(_) => (ProcessState::Waiting, None),
            Phase::Stopped => (ProcessState::Stopped, None),
        };
        ProcessInfo {
            name: self.spec.name.clone(),
            state,
            pid: running.map(|(child, _)| child.id()),
            start_ticks: running.and_then(|(_, launch)| launch.start_ticks),
            program: running.map(|(_, launch)| launch.program.clone()),
            from_dev_folder: running.is_some_and(|(_, launch)| launch.from_dev_folder),
            installed,
            dev_program,
        }
    }

    /// The child while it runs or stops.
    fn child(&self) -> Option<&Child> {
        match &self.phase {
            Phase::Running { child, .. } | Phase::Stopping { child, .. } => Some(child),
            Phase::Waiting(_) | Phase::Stopped => None,
        }
    }
}

/// The supervised shell processes.
pub struct Supervisor {
    procs: Vec<Supervised>,
    /// The program of the shell's hosts; `None` when it is not installed.
    host_program: Option<HostProgram>,
    /// Why the host program was refused at a start; while set no host starts.
    host_refused: Option<String>,
    /// The development folder the configured processes may start from ([`Supervisor::set_dev_folder`]).
    dev_folder: Option<DevFolder>,
}

impl Supervisor {
    /// Processes start at the first [`Supervisor::poll`] at or after `now`.
    pub fn new(specs: &[ShellProcess], now: Instant) -> Self {
        Self {
            procs: specs
                .iter()
                .map(|spec| Supervised::new(spec.clone(), now, Origin::Configured, false))
                .collect(),
            host_program: None,
            host_refused: None,
            dev_folder: None,
        }
    }

    /// The program the hosts start, `None` when it is not installed; it applies from each host's next start, and takes
    /// a refusal away.
    pub fn set_host_program(&mut self, program: Option<HostProgram>) {
        self.host_program = program;
        self.host_refused = None;
    }

    /// The program the hosts start.
    pub fn host_program(&self) -> Option<&HostProgram> {
        self.host_program.as_ref()
    }

    /// Whether hosts may start: their program is installed and was not refused.
    pub fn hosts_available(&self) -> bool {
        self.host_program.is_some() && self.host_refused.is_none()
    }

    /// Why the host program was refused, while it is.
    pub fn host_refused(&self) -> Option<&str> {
        self.host_refused.as_deref()
    }

    /// Record that the host program was refused for `reason`; `None` lets the hosts try it again.
    pub fn set_host_refused(&mut self, reason: Option<String>) {
        self.host_refused = reason;
    }

    /// Let the processes `folder` names start from it ([`DevFolder::eligible`]); `None` takes the folder away. A name the
    /// folder may not apply to is logged and ignored, and a built-in process never starts from it; a name of a component
    /// without a process entry is left to the hosts. It applies from each process's next start.
    pub fn set_dev_folder(&mut self, folder: Option<&DevFolder>) {
        self.dev_folder = folder.cloned();
        if let Some(folder) = folder {
            let configured = self.configured();
            let for_processes = DevFolder {
                path: folder.path.clone(),
                components: folder
                    .components
                    .iter()
                    .filter(|name| {
                        crate::host::component_spec(name).is_none()
                            || configured.iter().any(|p| &p.name == *name)
                    })
                    .cloned()
                    .collect(),
            };
            let (eligible, warnings) = for_processes.eligible(&configured);
            for warning in warnings {
                tracing::warn!(folder = %folder.path.display(), "{warning}: ignored");
            }
            tracing::info!(folder = %folder.path.display(), components = ?eligible, "development folder");
        }
        self.refresh_dev_folder();
    }

    /// The configured processes' specs.
    fn configured(&self) -> Vec<ShellProcess> {
        self.procs
            .iter()
            .filter(|p| p.origin == Origin::Configured)
            .map(|p| p.spec.clone())
            .collect()
    }

    /// Give every configured process the development folder when the folder applies to it.
    fn refresh_dev_folder(&mut self) {
        let eligible = match &self.dev_folder {
            Some(folder) => folder.eligible(&self.configured()).0,
            None => Vec::new(),
        };
        let path = self.dev_folder.as_ref().map(|f| f.path.clone());
        for p in &mut self.procs {
            match p.origin {
                Origin::Configured => {
                    let applies = eligible.contains(&p.spec.name);
                    p.dev_folder = path.clone().filter(|_| applies);
                }
                // a folder taken away applies to the hosts from their next start too; the plan gives one back
                Origin::Host(_) if path.is_none() => p.dev_folder = None,
                Origin::Host(_) | Origin::BuiltIn => {}
            }
        }
    }

    /// Supervise `cupertino-login --greeter` for the login session; one whose program is not installed (no path, or
    /// nothing at it) is logged once and never started. The program is checked before every start ([`check_built_in`]).
    pub fn add_login_greeter(&mut self, program: Option<PathBuf>, now: Instant) {
        match program.filter(|p| p.symlink_metadata().is_ok()) {
            Some(program) => self.procs.push(Supervised::new(
                BuiltIn::login_greeter_process(&program),
                now,
                Origin::BuiltIn,
                true,
            )),
            None => tracing::info!(
                name = BuiltIn::Login.name(),
                "not installed: the greeter login process is not started"
            ),
        }
    }

    /// Supervise the built-in processes from `programs`, each at the first [`Supervisor::poll`] at or after `now`. One
    /// whose program is not installed (no path, or nothing at it) is logged once and never started. The program is
    /// checked before every start ([`check_built_in`]): a refused start waits out the throttle, like a failed one.
    pub fn add_built_ins(
        &mut self,
        programs: impl IntoIterator<Item = (BuiltIn, Option<PathBuf>)>,
        now: Instant,
    ) {
        for (built_in, program) in programs {
            match program.filter(|p| p.symlink_metadata().is_ok()) {
                Some(program) => self.procs.push(Supervised::new(
                    built_in.process(&program),
                    now,
                    Origin::BuiltIn,
                    true,
                )),
                None => {
                    tracing::info!(
                        name = built_in.name(),
                        "not installed: the process is not started"
                    )
                }
            }
        }
    }

    /// Make the configured processes `processes` and the hosts `hosts` (each with the development folder it starts
    /// from, if any) the ones that run: a missing one starts at the next pass at or after `now`, one that is no longer
    /// named ends and is removed once it exited (a configured process and the shared host after SIGTERM, killed after
    /// [`RESTART_STOP_GRACE`]; a separate host by itself, once the server sent it an empty list). Built-in processes are
    /// left alone, and hosts are not started while [`Supervisor::hosts_available`] does not hold. Returns the hosts that
    /// end, with the id of their connection: the server expects their exit.
    pub fn reconcile(
        &mut self,
        processes: &[ShellProcess],
        hosts: &[(HostKind, Option<PathBuf>)],
        now: Instant,
    ) -> Vec<(HostKind, Option<u64>)> {
        let hosts: &[(HostKind, Option<PathBuf>)] =
            if self.hosts_available() { hosts } else { &[] };
        let mut ended = Vec::new();
        for p in &mut self.procs {
            let wanted = match p.origin {
                Origin::BuiltIn => continue,
                Origin::Configured => processes.iter().any(|s| s.name == p.spec.name),
                Origin::Host(kind) => hosts.iter().any(|(k, _)| *k == kind),
            };
            if wanted {
                p.retiring = false;
            } else if !p.retiring {
                if let Some(kind) = p.host() {
                    ended.push((kind, p.host_id));
                }
                retire(p, now);
            }
        }
        self.procs.retain(|p| !p.gone);
        for spec in processes {
            match self
                .procs
                .iter_mut()
                .find(|p| p.origin == Origin::Configured && p.spec.name == spec.name)
            {
                // a changed entry applies from its next start
                Some(p) => p.spec = spec.clone(),
                None => self.procs.push(Supervised::new(
                    spec.clone(),
                    now,
                    Origin::Configured,
                    false,
                )),
            }
        }
        if let Some(program) = self.host_program.clone() {
            for (kind, dev_folder) in hosts {
                let p = match self.procs.iter_mut().find(|p| p.host() == Some(*kind)) {
                    Some(p) => p,
                    None => {
                        let spec = program.process(*kind);
                        self.procs.push(Supervised::new(
                            spec,
                            now,
                            Origin::Host(*kind),
                            program.checked,
                        ));
                        self.procs.last_mut().expect("just pushed")
                    }
                };
                p.dev_folder = dev_folder.clone();
            }
        }
        self.refresh_dev_folder();
        ended
    }

    /// Whether the process named `name` is supervised, running or not.
    pub fn supervises(&self, name: &str) -> bool {
        self.procs.iter().any(|p| p.spec.name == name)
    }

    /// Whether no process is supervised.
    pub fn is_empty(&self) -> bool {
        self.procs.is_empty()
    }

    /// Every supervised process, in the order they start.
    pub fn processes(&self) -> Vec<ProcessInfo> {
        self.procs.iter().map(Supervised::info).collect()
    }

    /// The host of `kind` as a process, while it is supervised.
    pub fn host_info(&self, kind: HostKind) -> Option<ProcessInfo> {
        self.procs
            .iter()
            .find(|p| p.host() == Some(kind))
            .map(Supervised::info)
    }

    /// The id of the connection of the running host named `name` (`host`, `host-NAME`).
    pub fn host_id_of(&self, name: &str) -> Option<u64> {
        self.procs
            .iter()
            .find(|p| p.host().is_some() && p.spec.name == name)
            .and_then(|p| p.host_id)
    }

    /// Restart the process named `name`: a running one is sent SIGTERM and killed after [`RESTART_STOP_GRACE`], and once
    /// it ended it starts again at once, whatever its exit status; one that is not running starts at once. `reply` gets
    /// the process once it runs, or why not. The starts happen in [`Supervisor::poll`], which the caller runs next.
    pub fn request_restart(&mut self, name: &str, now: Instant, reply: RestartReply) {
        let Some(p) = self
            .procs
            .iter_mut()
            .find(|p| p.spec.name == name && !p.retiring)
        else {
            reply(Err(RestartError::Unknown(name.to_string())));
            return;
        };
        p.restarts.push(reply);
        p.phase = match std::mem::replace(&mut p.phase, Phase::Stopped) {
            Phase::Running { child, launch, .. } => {
                tracing::info!(
                    name,
                    pid = child.id(),
                    "restart asked for: ending the shell process"
                );
                terminate(&child);
                Phase::Stopping {
                    child,
                    kill_at: Some(now + RESTART_STOP_GRACE),
                    launch,
                }
            }
            stopping @ Phase::Stopping { .. } => stopping,
            Phase::Waiting(_) | Phase::Stopped => {
                tracing::info!(name, "restart asked for: starting the shell process");
                p.backoff = Backoff::default();
                Phase::Waiting(now)
            }
        };
    }

    /// Start due processes and restart exited ones as the policy says. Children get `WAYLAND_DISPLAY=socket` (when the
    /// server listens on one) and the X display `x11` ([`crate::xwayland::set_client_env`]) for the programs they start,
    /// and their own connection from `launcher` (made for the process's roles, or a host connection) as `WAYLAND_SOCKET`.
    pub fn poll(
        &mut self,
        now: Instant,
        socket: Option<&str>,
        x11: Option<&XDisplay>,
        launcher: &mut dyn Launcher,
    ) {
        let env = StartEnv { socket, x11, now };
        let mut refused = false;
        for p in &mut self.procs {
            let failed_start = step(p, &env, launcher, &mut refused);
            answer_restarts(p, failed_start);
        }
        self.procs.retain(|p| !p.gone);
        if refused {
            // a refused host is not tried again: its components run as processes until the program is tried again
            self.host_refused
                .get_or_insert_with(|| "the host program was refused".into());
        }
    }

    /// The pids of the running processes.
    pub fn pids(&self) -> Vec<u32> {
        self.procs
            .iter()
            .filter_map(|p| p.child().map(Child::id))
            .collect()
    }

    /// The pids of the running processes whose connection holds `role`.
    pub fn pids_with_role(&self, role: crate::authorization::ShellRole) -> Vec<u32> {
        self.procs
            .iter()
            .filter(|p| p.spec.roles.contains(&role))
            .filter_map(|p| p.child().map(Child::id))
            .collect()
    }

    /// When the next pass is due: the earliest start of a process waiting out its throttle, or the kill of one that
    /// is late to end for its restart.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.procs
            .iter()
            .filter_map(|p| match p.phase {
                Phase::Waiting(at) => Some(at),
                Phase::Stopping { kill_at, .. } => kill_at,
                Phase::Running { .. } | Phase::Stopped => None,
            })
            .min()
    }

    /// The pid of the running process named `name`; `None` while it is not running.
    pub fn pid_of(&self, name: &str) -> Option<u32> {
        self.procs
            .iter()
            .find(|p| p.spec.name == name)
            .and_then(|p| p.child().map(Child::id))
    }
}

/// End `p` for good: one that does not run is gone at once; a separate host is left to exit once it has an empty list;
/// any other running process is sent SIGTERM and killed after [`RESTART_STOP_GRACE`]. Restarts asked for are answered.
fn retire(p: &mut Supervised, now: Instant) {
    p.retiring = true;
    tracing::info!(
        name = p.spec.name,
        "no longer configured: the shell process ends"
    );
    for reply in p.restarts.drain(..) {
        reply(Err(RestartError::NotStarted(
            "it is no longer configured".into(),
        )));
    }
    p.phase = match std::mem::replace(&mut p.phase, Phase::Stopped) {
        Phase::Waiting(_) | Phase::Stopped => {
            p.gone = true;
            Phase::Stopped
        }
        Phase::Running {
            child,
            started,
            launch,
        } if matches!(p.origin, Origin::Host(HostKind::Separate(_))) => Phase::Running {
            child,
            started,
            launch,
        },
        Phase::Running { child, launch, .. } => {
            terminate(&child);
            Phase::Stopping {
                child,
                kill_at: Some(now + RESTART_STOP_GRACE),
                launch,
            }
        }
        stopping @ Phase::Stopping { .. } => stopping,
    };
}

/// What every start of a pass shares.
struct StartEnv<'a> {
    socket: Option<&'a str>,
    x11: Option<&'a XDisplay>,
    now: Instant,
}

/// One pass over `p`: start it when due, reap it when it exited and decide its next start. Returns why a start that was
/// tried failed; `refused` is set when a host could not start and is not tried again.
fn step(
    p: &mut Supervised,
    env: &StartEnv<'_>,
    launcher: &mut dyn Launcher,
    refused: &mut bool,
) -> Option<String> {
    let now = env.now;
    let mut failed_start = None;
    p.phase = match std::mem::replace(&mut p.phase, Phase::Stopped) {
        Phase::Waiting(at) if at <= now && !p.retiring => {
            let (phase, failure) = start(p, env, launcher, refused);
            failed_start = failure;
            phase
        }
        Phase::Running {
            mut child,
            started,
            launch,
        } => match child
            .try_wait()
            .expect("waiting on a child the supervisor started")
        {
            None => Phase::Running {
                child,
                started,
                launch,
            },
            Some(status) => {
                tracing::info!(name = p.spec.name, %status, "shell process exited");
                let expected = match (p.host(), p.host_id.take()) {
                    (Some(_), Some(id)) => launcher.host_ended(id, status, launch.from_dev_folder),
                    _ => false,
                };
                if p.retiring {
                    p.gone = true;
                    return None;
                }
                let decision = match p.host() {
                    Some(_) => decide_host(expected, started, now, &mut p.backoff),
                    None => decide(status, started, now, &mut p.backoff),
                };
                record_exit(&p.spec, status, decision, now);
                match decision {
                    Decision::RestartNow => {
                        let (phase, failure) = start(p, env, launcher, refused);
                        failed_start = failure;
                        phase
                    }
                    Decision::RestartAt(at) => Phase::Waiting(at),
                    Decision::Stop => Phase::Stopped,
                }
            }
        },
        Phase::Stopping {
            mut child,
            kill_at,
            launch,
        } => match child
            .try_wait()
            .expect("waiting on a child the supervisor started")
        {
            None => {
                let kill_at = match kill_at {
                    Some(at) if at <= now => {
                        tracing::warn!(
                            name = p.spec.name,
                            pid = child.id(),
                            "the shell process did not end in time: killing it"
                        );
                        // it may have exited since the wait: killing it then fails harmlessly
                        let _ = child.kill();
                        None
                    }
                    other => other,
                };
                Phase::Stopping {
                    child,
                    kill_at,
                    launch,
                }
            }
            Some(status) => {
                tracing::info!(name = p.spec.name, %status, "shell process ended for its restart");
                if let (Some(_), Some(id)) = (p.host(), p.host_id.take()) {
                    // asked to end: never a crash
                    launcher.host_ended(id, status, launch.from_dev_folder);
                }
                if p.retiring {
                    p.gone = true;
                    return None;
                }
                p.backoff = Backoff::default();
                let (phase, failure) = start(p, env, launcher, refused);
                failed_start = failure;
                phase
            }
        },
        other => other,
    };
    failed_start
}

/// Ask `child` to end: SIGTERM. A child that already exited and is not reaped yet ignores it.
fn terminate(child: &Child) {
    if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
        // the child is ours and not reaped, so its pid is not anyone else's; one that already exited ignores it
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
}

/// Answer the restarts of `p` once a start was tried: with the process when it runs, else with why it did not start.
fn answer_restarts(p: &mut Supervised, failed_start: Option<String>) {
    if p.restarts.is_empty() {
        return;
    }
    let answer = match (&p.phase, failed_start) {
        (Phase::Running { .. }, _) => Ok(p.info()),
        (_, Some(reason)) => Err(RestartError::NotStarted(reason)),
        // still ending, or not tried yet
        _ => return,
    };
    for reply in p.restarts.drain(..) {
        reply(answer.clone());
    }
}

/// The start time of process `pid` in clock ticks after boot: field 22 of `/proc/PID/stat`.
pub fn start_ticks(pid: u32) -> Option<u64> {
    parse_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The wall-clock time, in seconds since 1970, of `start_ticks` clock ticks after boot: the boot time of `/proc/stat`
/// (`btime`) plus the ticks.
pub fn started_unix(start_ticks: u64) -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let boot: u64 = stat
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    // Safety: sysconf has no preconditions.
    let ticks_per_second = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
        .ok()
        .filter(|t| *t > 0)?;
    Some(boot + start_ticks / ticks_per_second)
}

/// Field 22 of the text of a `/proc/PID/stat`, after the command name in parentheses (which may hold any character).
fn parse_start_ticks(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    // fields 3 and on; the start time is field 22
    after_name.split_whitespace().nth(19)?.parse().ok()
}

/// `server.shell.exited` for a process that exited and is started again, after `restart_in`; `server.shell.gave_up` for
/// one the policy leaves stopped (it ended successfully).
fn record_exit(spec: &ShellProcess, status: ExitStatus, decision: Decision, now: Instant) {
    match decision.restart_in(now) {
        Some(restart_in) => with_exit_status(
            diag::event(
                Severity::Warn,
                "server.shell.exited",
                "a shell process exited and is started again",
            )
            .field("role", Value::Public(&spec.name)),
            status,
        )
        .field("restart_in", Value::Duration(restart_in))
        .emit(),
        None => with_exit_status(
            diag::event(
                Severity::Info,
                "server.shell.gave_up",
                "a shell process exited and is not started again",
            )
            .field("role", Value::Public(&spec.name)),
            status,
        )
        .emit(),
    }
}

/// `server.shell.start_failed`: `program` of the process was not started at `stage` for `error`, and is tried again
/// after `restart_in`.
fn record_start_failed(
    spec: &ShellProcess,
    program: &Path,
    stage: &'static str,
    error: &str,
    restart_in: Duration,
) {
    diag::event(
        Severity::Warn,
        "server.shell.start_failed",
        "a shell process could not be started",
    )
    .field("role", Value::Public(&spec.name))
    .field("program", Value::Path(program))
    .field("stage", Value::Public(stage))
    .field("error", Value::Public(error))
    .field("restart_in", Value::Duration(restart_in))
    .emit();
}

/// Spawn the process; a failed spawn counts as an exit with failure, so it is retried after the throttle, which grows.
/// A checked program ([`check_built_in`]) that is refused is not started, and is checked again then. A process with a
/// development folder starts the folder's program when [`dev_folder_program`] accepts it, else its installed one. A
/// host whose program is refused or cannot be spawned is not retried: [`Launcher::host_refused`] hears it, `refused`
/// is set and the host stays stopped. Returns the new phase and, for a start that failed, why.
fn start(
    p: &mut Supervised,
    env: &StartEnv<'_>,
    launcher: &mut dyn Launcher,
    refused: &mut bool,
) -> (Phase, Option<String>) {
    let now = env.now;
    let spec = p.spec.clone();
    let host = p.host();
    let (installed, args) = spec
        .command
        .split_first()
        .expect("shell process commands are validated non-empty at load");
    let backoff = &mut p.backoff;
    let mut retry = |stage: &'static str, program: &Path, error: String| {
        let retry = backoff.next_interval(Duration::ZERO);
        record_start_failed(&spec, program, stage, &error, retry);
        (
            Phase::Waiting(now + retry),
            Some(format!("{stage}: {error}")),
        )
    };
    let mut refuse =
        |launcher: &mut dyn Launcher, kind: HostKind, id: Option<u64>, reason: String| {
            launcher.host_refused(kind, id, &reason);
            *refused = true;
            (Phase::Stopped, Some(reason))
        };
    if p.checked {
        let program = PathBuf::from(installed);
        let checked = checked_in_time(move || check_built_in(&program));
        if let Err(reason) = checked {
            tracing::warn!(name = spec.name, %reason, "the program is not started");
            if let Some(kind) = host {
                return refuse(launcher, kind, None, format!("program_check: {reason}"));
            }
            return retry("program_check", Path::new(installed), reason);
        }
    }
    let from_dev = p.dev_folder.as_deref().and_then(|folder| {
        let (folder, program) = (folder.to_path_buf(), installed.clone());
        let user = rustix::process::getuid().as_raw();
        match checked_in_time(move || dev_folder_program(&folder, &program, user)) {
            Ok(program) => program,
            Err(reason) => {
                tracing::warn!(name = spec.name, %reason, "the development folder's program is refused: the installed program starts");
                None
            }
        }
    });
    let from_dev_folder = from_dev.is_some();
    let program = from_dev.unwrap_or_else(|| PathBuf::from(installed));
    let connection = match host {
        Some(kind) => launcher
            .connect_host(kind)
            .map(|(stream, id)| (stream, None, Some(id))),
        None => launcher
            .connect(&spec)
            .map(|(stream, state)| (stream, Some(state), None)),
    };
    let (stream, state, host_id) = match connection {
        Ok(connection) => connection,
        Err(e) => {
            tracing::warn!(name = spec.name, error = %e, "creating the shell process's connection failed");
            return retry("connection", &program, e.to_string());
        }
    };
    let mut command = Command::new(&program);
    command
        .args(args)
        .env("XDG_CURRENT_DESKTOP", crate::state::CURRENT_DESKTOP)
        .env("XDG_SESSION_TYPE", "wayland");
    match env.socket {
        Some(socket) => command.env("WAYLAND_DISPLAY", socket),
        None => command.env_remove("WAYLAND_DISPLAY"),
    };
    crate::xwayland::set_client_env(&mut command, env.x11);
    hand_over(&mut command, &stream);
    end_with_server(&mut command);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(name = spec.name, program = %program.display(), error = %e, "starting shell process failed");
            if let Some(kind) = host {
                return refuse(launcher, kind, host_id, format!("spawn: {e}"));
            }
            return retry("spawn", &program, e.to_string());
        }
    };
    // the server keeps no descriptor of the child's end
    drop(stream);
    let bound = match (&state, host_id) {
        (Some(state), _) => bind_holder(state, child.id()),
        (None, Some(id)) => launcher.host_started(id, child.id(), from_dev_folder),
        (None, None) => Err(std::io::Error::other("the connection has no state")),
    };
    if let Err(e) = bound {
        tracing::warn!(name = spec.name, error = %e, "binding the shell process to its connection failed: ending it");
        if let Some(id) = host_id {
            launcher.host_unbound(id, from_dev_folder);
        }
        crate::shell_connection::end_unbound(child);
        return retry("bind", &program, e.to_string());
    }
    p.host_id = host_id;
    let source = if from_dev_folder {
        "development folder"
    } else {
        "installed"
    };
    tracing::info!(name = spec.name, pid = child.id(), program = %program.display(), source, "shell process started");
    let pid = child.id();
    let launch = Launch {
        start_ticks: crate::process::Lookups::shared()
            .run(None, crate::process::CHECK_WAIT, move || start_ticks(pid))
            .flatten(),
        program,
        from_dev_folder,
    };
    (
        Phase::Running {
            child,
            started: now,
            launch,
        },
        None,
    )
}

/// The answer of the program check `check`, run on a thread the loop waits for until [`crate::process::CHECK_WAIT`] has
/// passed: a program on a file system that stalls is refused, not waited for, and checked again at the next start.
fn checked_in_time<T: Send + 'static>(
    check: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    crate::process::Lookups::shared()
        .run(None, crate::process::CHECK_WAIT, check)
        .unwrap_or_else(|| Err("the program was not checked in time".into()))
}

/// Kill the process `command` starts when the server's thread that starts it ends, however it ends: a server killed or
/// crashed never runs [`Supervisor`]'s drop, and its shell processes would run on without a compositor. A server that
/// is already gone when the child sets this up kills the child at once.
fn end_with_server(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    let server = rustix::process::getpid();
    // Safety: the closure runs in the child between fork and exec and calls only prctl, getppid, getpid and kill,
    // which are system calls.
    unsafe {
        command.pre_exec(move || {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))?;
            if rustix::process::getppid() != Some(server) {
                let _ = rustix::process::kill_process(
                    rustix::process::getpid(),
                    rustix::process::Signal::KILL,
                );
            }
            Ok(())
        });
    }
}

/// The event-loop sources that wake the supervision (`crate::cli::start_shell_supervision`): a pidfd per running child,
/// readable when it exits, and one timer at the earliest restart. Nothing wakes it while every child runs.
#[derive(Default)]
pub struct Wakeups {
    /// The pidfd source of each watched child, by pid.
    pub watched: std::collections::HashMap<u32, smithay::reexports::calloop::RegistrationToken>,
    /// The restart timer and when it fires.
    pub timer: Option<(smithay::reexports::calloop::RegistrationToken, Instant)>,
}

impl Drop for Supervisor {
    /// Shell processes do not outlive the server.
    fn drop(&mut self) {
        for p in &mut self.procs {
            if let Phase::Running { child, .. } | Phase::Stopping { child, .. } = &mut p.phase {
                // the child may already have exited: killing it then fails harmlessly
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}
