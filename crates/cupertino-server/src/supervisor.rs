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
//! Each exit and each start that fails is also a diagnostic event: `server.shell.exited` (restarted),
//! `server.shell.gave_up` (not restarted) and `server.shell.start_failed`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::{self as diag, Severity, Value};

use crate::config::{check_built_in, dev_folder_program, BuiltIn, DevFolder, ShellProcess};
use crate::restart_policy::{decide, with_exit_status, Backoff, Decision};
use crate::shell_connection::{bind_holder, hand_over, BoundConnection};
use crate::xwayland::XDisplay;

/// How long a process asked to end for a restart has after SIGTERM before it is killed.
pub const RESTART_STOP_GRACE: Duration = Duration::from_secs(5);

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
}

/// Where the answer to a restart goes: called once, from [`Supervisor::poll`] or [`Supervisor::request_restart`].
pub type RestartReply = Box<dyn FnOnce(Result<ProcessInfo, RestartError>) + Send>;

struct Supervised {
    spec: ShellProcess,
    phase: Phase,
    backoff: Backoff,
    /// A built-in process: its program is checked again before every start ([`check_built_in`]).
    built_in: bool,
    /// The development folder the process may start from; `None` when it starts its installed program alone.
    dev_folder: Option<PathBuf>,
    /// The restarts asked for and not answered yet.
    restarts: Vec<RestartReply>,
}

impl Supervised {
    fn new(spec: ShellProcess, now: Instant, built_in: bool) -> Self {
        Self {
            spec,
            phase: Phase::Waiting(now),
            backoff: Backoff::default(),
            built_in,
            dev_folder: None,
            restarts: Vec::new(),
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
}

impl Supervisor {
    /// Processes start at the first [`Supervisor::poll`] at or after `now`.
    pub fn new(specs: &[ShellProcess], now: Instant) -> Self {
        Self {
            procs: specs
                .iter()
                .map(|spec| Supervised::new(spec.clone(), now, false))
                .collect(),
        }
    }

    /// Let the processes `folder` names start from it ([`DevFolder::eligible`]); `None` takes the folder away. A name the
    /// folder may not apply to is logged and ignored, and a built-in process never starts from it. It applies from each
    /// process's next start.
    pub fn set_dev_folder(&mut self, folder: Option<&DevFolder>) {
        let configured: Vec<ShellProcess> = self
            .procs
            .iter()
            .filter(|p| !p.built_in)
            .map(|p| p.spec.clone())
            .collect();
        let eligible = match folder {
            Some(folder) => {
                let (eligible, warnings) = folder.eligible(&configured);
                for warning in warnings {
                    tracing::warn!(folder = %folder.path.display(), "{warning}: ignored");
                }
                eligible
            }
            None => Vec::new(),
        };
        for p in &mut self.procs {
            let applies = !p.built_in && eligible.contains(&p.spec.name);
            p.dev_folder = folder.filter(|_| applies).map(|f| f.path.clone());
        }
        if let Some(folder) = folder {
            tracing::info!(folder = %folder.path.display(), components = ?eligible, "development folder");
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
                Some(program) => {
                    self.procs
                        .push(Supervised::new(built_in.process(&program), now, true))
                }
                None => {
                    tracing::info!(
                        name = built_in.name(),
                        "not installed: the process is not started"
                    )
                }
            }
        }
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

    /// Restart the process named `name`: a running one is sent SIGTERM and killed after [`RESTART_STOP_GRACE`], and once
    /// it ended it starts again at once, whatever its exit status; one that is not running starts at once. `reply` gets
    /// the process once it runs, or why not. The starts happen in [`Supervisor::poll`], which the caller runs next.
    pub fn request_restart(&mut self, name: &str, now: Instant, reply: RestartReply) {
        let Some(p) = self.procs.iter_mut().find(|p| p.spec.name == name) else {
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

    /// Start due processes and restart exited ones as the policy says. Children get `WAYLAND_DISPLAY=socket` and the X
    /// display `x11` ([`crate::xwayland::set_client_env`]) for the programs they start, and their own connection from
    /// `connect` (made for the process's roles) as `WAYLAND_SOCKET`.
    pub fn poll(
        &mut self,
        now: Instant,
        socket: &str,
        x11: Option<&XDisplay>,
        connect: &mut dyn FnMut(&ShellProcess) -> std::io::Result<BoundConnection>,
    ) {
        for p in &mut self.procs {
            let mut start_now = |p: &mut Supervised| {
                start(
                    &p.spec,
                    p.built_in,
                    p.dev_folder.as_deref(),
                    socket,
                    x11,
                    connect,
                    now,
                    &mut p.backoff,
                )
            };
            let mut failed_start = None;
            p.phase = match std::mem::replace(&mut p.phase, Phase::Stopped) {
                Phase::Waiting(at) if at <= now => {
                    let (phase, failure) = start_now(p);
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
                        let decision = decide(status, started, now, &mut p.backoff);
                        record_exit(&p.spec, status, decision, now);
                        match decision {
                            Decision::RestartNow => {
                                let (phase, failure) = start_now(p);
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
                                tracing::warn!(name = p.spec.name, pid = child.id(), "the shell process did not end for its restart in time: killing it");
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
                        p.backoff = Backoff::default();
                        let (phase, failure) = start_now(p);
                        failed_start = failure;
                        phase
                    }
                },
                other => other,
            };
            answer_restarts(p, failed_start);
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
/// The program of a `built_in` process that [`check_built_in`] refuses is not started, and is checked again then. A
/// process with a `dev_folder` starts the folder's program when [`dev_folder_program`] accepts it, else its installed
/// one. Returns the new phase and, for a start that failed, why.
#[allow(clippy::too_many_arguments)]
fn start(
    spec: &ShellProcess,
    built_in: bool,
    dev_folder: Option<&Path>,
    socket: &str,
    x11: Option<&XDisplay>,
    connect: &mut dyn FnMut(&ShellProcess) -> std::io::Result<BoundConnection>,
    now: Instant,
    backoff: &mut Backoff,
) -> (Phase, Option<String>) {
    let (installed, args) = spec
        .command
        .split_first()
        .expect("shell process commands are validated non-empty at load");
    let mut retry = |stage: &'static str, program: &Path, error: String| {
        let retry = backoff.next_interval(Duration::ZERO);
        record_start_failed(spec, program, stage, &error, retry);
        (
            Phase::Waiting(now + retry),
            Some(format!("{stage}: {error}")),
        )
    };
    if built_in {
        if let Err(reason) = check_built_in(Path::new(installed)) {
            tracing::warn!(name = spec.name, %reason, "the built-in program is not started");
            return retry("program_check", Path::new(installed), reason);
        }
    }
    let from_dev = dev_folder.and_then(|folder| {
        match dev_folder_program(folder, installed, rustix::process::getuid().as_raw()) {
            Ok(program) => program,
            Err(reason) => {
                tracing::warn!(name = spec.name, %reason, "the development folder's program is refused: the installed program starts");
                None
            }
        }
    });
    let from_dev_folder = from_dev.is_some();
    let program = from_dev.unwrap_or_else(|| PathBuf::from(installed));
    let (stream, state) = match connect(spec) {
        Ok(connection) => connection,
        Err(e) => {
            tracing::warn!(name = spec.name, error = %e, "creating the shell process's connection failed");
            return retry("connection", &program, e.to_string());
        }
    };
    let mut command = Command::new(&program);
    command
        .args(args)
        .env("WAYLAND_DISPLAY", socket)
        .env("XDG_CURRENT_DESKTOP", crate::state::CURRENT_DESKTOP)
        .env("XDG_SESSION_TYPE", "wayland");
    crate::xwayland::set_client_env(&mut command, x11);
    hand_over(&mut command, &stream);
    end_with_server(&mut command);
    match command.spawn() {
        Ok(child) => {
            if let Err(e) = bind_holder(&state, child.id()) {
                tracing::warn!(name = spec.name, error = %e, "binding the shell process to its connection failed: ending it");
                crate::shell_connection::end_unbound(child);
                return retry("bind", &program, e.to_string());
            }
            let source = if from_dev_folder {
                "development folder"
            } else {
                "installed"
            };
            tracing::info!(name = spec.name, pid = child.id(), program = %program.display(), source, "shell process started");
            let launch = Launch {
                start_ticks: start_ticks(child.id()),
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
        Err(e) => {
            tracing::warn!(name = spec.name, program = %program.display(), error = %e, "starting shell process failed");
            retry("spawn", &program, e.to_string())
        }
    }
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
