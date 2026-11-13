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
        Ok(mut child) => {
            if let Err(e) = bind_holder(&state, child.id()) {
                tracing::warn!(name = spec.name, error = %e, "binding the shell process to its connection failed: ending it");
                let _ = child.kill();
                let _ = child.wait();
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

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::config::DevFolder;
    use crate::logging::test_events;
    use crate::restart_policy::RESTART_THROTTLE;
    use crate::state::ClientState;

    /// A process that runs `script` under `sh`, with the empty asset search path a test's processes get.
    fn spec(script: &str) -> ShellProcess {
        ShellProcess {
            name: "test".into(),
            command: vec![
                "env".into(),
                "CUPERTINO_ASSET_SEARCH_PATH=".into(),
                "sh".into(),
                "-c".into(),
                script.into(),
            ],
            roles: Vec::new(),
        }
    }

    /// A connection nobody listens on: these tests run plain scripts.
    fn connect(_: &ShellProcess) -> std::io::Result<BoundConnection> {
        UnixStream::pair().map(|(_, client)| (client, Arc::new(ClientState::default())))
    }

    /// Poll every 20 ms until `done` holds or ten seconds pass.
    fn poll_until(s: &mut Supervisor, mut done: impl FnMut(&Supervisor) -> bool) {
        let mut connect = connect;
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            s.poll(Instant::now(), "wayland-test", None, &mut connect);
            if done(s) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out");
    }

    #[test]
    fn failing_process_restarts_at_most_once_per_second_and_gets_the_socket() {
        let dir = std::env::temp_dir().join(format!("cupertino-supervisor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("starts");
        // each start appends the display it was given and fails
        let script = format!("echo \"$WAYLAND_DISPLAY\" >> {}; exit 1", log.display());
        let mut s = Supervisor::new(&[spec(&script)], Instant::now());
        let begin = Instant::now();
        poll_until(&mut s, |_| {
            std::fs::read_to_string(&log).is_ok_and(|t| t.lines().count() >= 3)
        });
        // three starts need two throttle intervals
        assert!(begin.elapsed() >= 2 * RESTART_THROTTLE);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().lines().next(),
            Some("wayland-test")
        );
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_process_that_keeps_failing_is_restarted_after_longer_and_longer_waits() {
        test_events::memory();
        let mut s = Supervisor::new(&[spec("exit 1")], Instant::now());
        // the start times the supervisor itself recorded, which its throttle counts from: a child that runs late
        // cannot shorten them
        let mut starts: Vec<Instant> = Vec::new();
        poll_until(&mut s, |s| {
            if let Phase::Running { started, .. } = &s.procs[0].phase {
                if starts.last() != Some(started) {
                    starts.push(*started);
                }
            }
            starts.len() >= 3
        });
        let (first, second) = (starts[1] - starts[0], starts[2] - starts[1]);
        assert!(first >= RESTART_THROTTLE, "first wait {first:?}");
        assert!(second >= 2 * RESTART_THROTTLE, "second wait {second:?}");
        // each exit says when the process starts again: within the throttle of its start, which doubles
        let exits = test_events::recorded("server.shell.exited");
        assert!(exits.len() >= 2, "{exits:?}");
        for exit in &exits {
            assert_eq!(exit.severity, diag::Severity::Warn);
            assert_eq!(exit.message, "a shell process exited and is started again");
            assert_eq!(exit.fields.get("role").unwrap(), "test");
            assert_eq!(exit.fields.get("status").unwrap(), 1);
            assert!(exit.fields.get("signal").is_none());
        }
        let restart_in = |i: usize| exits[i].fields.get("restart_in").unwrap().as_u64().unwrap();
        assert!(restart_in(0) <= 1_000, "{exits:?}");
        assert!((1_000..=2_000).contains(&restart_in(1)), "{exits:?}");
        assert!(test_events::recorded("server.shell.gave_up").is_empty());
    }

    #[test]
    fn a_process_ended_by_a_signal_is_recorded_with_the_signal() {
        test_events::memory();
        let mut s = Supervisor::new(&[spec("kill -SEGV $$")], Instant::now());
        poll_until(&mut s, |_| {
            !test_events::recorded("server.shell.exited").is_empty()
        });
        assert!(
            matches!(s.procs[0].phase, Phase::Waiting(_)),
            "it waits out the throttle"
        );
        let exit = &test_events::recorded("server.shell.exited")[0];
        assert_eq!(exit.fields.get("signal").unwrap(), libc::SIGSEGV);
        assert!(exit.fields.get("status").is_none());
    }

    /// A server that ends without dropping its supervisor (killed, crashed) leaves no shell process behind: the thread
    /// that started it ends here with the supervisor forgotten, and the process is killed with it.
    #[test]
    fn a_shell_process_is_killed_when_the_server_ends_without_stopping_it() {
        let starter = std::thread::spawn(|| {
            let mut s = Supervisor::new(&[spec("exec sleep 60")], Instant::now());
            poll_until(&mut s, |s| !s.pids().is_empty());
            let pid = s.pids()[0];
            std::mem::forget(s);
            pid
        });
        let pid = libc::pid_t::try_from(starter.join().unwrap()).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        let mut status = 0;
        loop {
            // Safety: waits for this process's own child, which nothing else reaps
            let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            assert_ne!(reaped, -1, "{}", std::io::Error::last_os_error());
            if reaped == pid {
                break;
            }
            assert!(
                Instant::now() < end,
                "the shell process outlived its server"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(libc::WIFSIGNALED(status), "status {status:#x}");
        assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
    }

    /// The first poll of a process named `dock` running `command`, with its connection from `connect`: the supervisor
    /// after it and the `server.shell.start_failed` events.
    fn start_once(
        command: &[&str],
        connect: &mut dyn FnMut(&ShellProcess) -> std::io::Result<BoundConnection>,
    ) -> (Supervisor, Vec<diag::Record>) {
        test_events::memory();
        let spec = ShellProcess {
            name: "dock".into(),
            command: command.iter().map(|c| c.to_string()).collect(),
            roles: Vec::new(),
        };
        let now = Instant::now();
        let mut s = Supervisor::new(&[spec], now);
        s.poll(now, "wayland-test", None, connect);
        (s, test_events::recorded("server.shell.start_failed"))
    }

    fn assert_retried(s: &Supervisor, failed: &[diag::Record], stage: &str) {
        assert!(
            matches!(s.procs[0].phase, Phase::Waiting(_)),
            "retried later"
        );
        assert_eq!(s.pids(), Vec::<u32>::new());
        assert_eq!(failed.len(), 1, "{failed:?}");
        let failed = &failed[0];
        assert_eq!(failed.severity, diag::Severity::Warn);
        assert_eq!(failed.message, "a shell process could not be started");
        assert_eq!(failed.fields.get("role").unwrap(), "dock");
        assert_eq!(failed.fields.get("stage").unwrap(), stage);
        assert_eq!(
            failed.fields.get("restart_in").unwrap(),
            RESTART_THROTTLE.as_millis() as u64
        );
    }

    #[test]
    fn a_program_that_cannot_be_run_is_recorded_and_tried_again_after_the_throttle() {
        let mut connect = connect;
        let (s, failed) = start_once(&["/nonexistent/cupertino-dock"], &mut connect);
        assert_retried(&s, &failed, "spawn");
        // a program outside the system directories is kept as a hash of its name
        let program = failed[0].fields.get("program").unwrap().as_str().unwrap();
        assert!(program.starts_with("…#"), "{program}");
        assert!(failed[0].redacted.contains(&"program".to_owned()));
        assert_eq!(
            failed[0].fields.get("error").unwrap(),
            "No such file or directory (os error 2)"
        );
    }

    #[test]
    fn a_connection_that_cannot_be_made_is_recorded_and_tried_again_after_the_throttle() {
        let mut refuse = |_: &ShellProcess| -> std::io::Result<BoundConnection> {
            Err(std::io::Error::from(std::io::ErrorKind::OutOfMemory))
        };
        let (s, failed) = start_once(&["/nonexistent/cupertino-dock"], &mut refuse);
        assert_retried(&s, &failed, "connection");
        assert_eq!(failed[0].fields.get("error").unwrap(), "out of memory");
    }

    #[test]
    fn a_connection_that_cannot_be_bound_to_its_process_is_recorded_and_tried_again() {
        // a connection made for a client in the server's own process is never bound to another
        let mut detached = |_: &ShellProcess| -> std::io::Result<BoundConnection> {
            UnixStream::pair().map(|(_, client)| {
                (
                    client,
                    Arc::new(ClientState::detached(
                        crate::authorization::ClientCapabilitySet::NONE,
                    )),
                )
            })
        };
        let (s, failed) = start_once(
            &["env", "CUPERTINO_ASSET_SEARCH_PATH=", "true"],
            &mut detached,
        );
        assert_retried(&s, &failed, "bind");
    }

    #[test]
    fn the_next_deadline_is_the_earliest_restart_and_none_while_all_run() {
        let now = Instant::now();
        let s = Supervisor::new(&[spec("exit 1"), spec("exit 1")], now);
        assert_eq!(s.next_deadline(), Some(now), "both are due to start");
        let mut s = Supervisor::new(&[spec("exec sleep 60")], now);
        let mut connect = connect;
        s.poll(now, "wayland-test", None, &mut connect);
        assert_eq!(
            s.next_deadline(),
            None,
            "a running process waits for its exit"
        );
    }

    #[test]
    fn a_built_in_program_others_can_change_after_it_ran_is_not_started_again() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let log = root.path().join("starts");
        let program = bin.join("cupertino-portal");
        std::fs::write(
            &program,
            format!("#!/bin/sh\necho >> {}\nexit 1\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let starts = || std::fs::read_to_string(&log).map_or(0, |t| t.lines().count());

        let mut s = Supervisor::new(&[], Instant::now());
        s.add_built_ins([(BuiltIn::Portal, Some(program.clone()))], Instant::now());
        poll_until(&mut s, |s| starts() == 1 && s.next_deadline().is_some());
        test_events::memory();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o775)).unwrap();
        let end = Instant::now() + 3 * RESTART_THROTTLE;
        let mut connect = connect;
        while Instant::now() < end {
            s.poll(Instant::now(), "wayland-test", None, &mut connect);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(starts(), 1, "a group-writable program was started again");
        assert!(s.next_deadline().is_some(), "it is checked again later");
        let refused = test_events::recorded("server.shell.start_failed");
        assert!(!refused.is_empty());
        assert_eq!(refused[0].fields.get("role").unwrap(), "portal");
        assert_eq!(refused[0].fields.get("stage").unwrap(), "program_check");
        assert!(
            refused[0]
                .fields
                .get("error")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("group"),
            "{refused:?}"
        );
    }

    #[test]
    fn a_built_in_process_that_is_not_installed_is_never_started() {
        let now = Instant::now();
        let mut s = Supervisor::new(&[], now);
        s.add_built_ins(
            [
                (
                    BuiltIn::Portal,
                    Some(PathBuf::from("/nonexistent/cupertino-portal")),
                ),
                (BuiltIn::PolkitAgent, None),
            ],
            now,
        );
        let mut connect = connect;
        s.poll(now, "wayland-test", None, &mut connect);
        assert!(s.is_empty());
        assert_eq!(s.next_deadline(), None, "no start is ever due");
    }

    /// A program at `path` that exports each `CUPERTINO_ASSET_SEARCH_PATH=…` argument it is given, runs `setup` (a
    /// `trap`), appends `who` and its arguments to `log`, then runs `then`
    /// (`exit 0`, `exec sleep 60`…). It is written beside `path` and renamed into place, so no start sees it half made.
    fn script_with(path: &Path, setup: &str, who: &str, log: &Path, then: &str) {
        use std::os::unix::fs::PermissionsExt;
        let staged = path.with_extension("staged");
        std::fs::write(
            &staged,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do case $a in CUPERTINO_ASSET_SEARCH_PATH=*) export \"$a\";; esac; done\n\
                 {setup}\necho {who} \"$@\" >> {}\n{then}\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&staged, path).unwrap();
    }

    /// [`script_with`] without a setup.
    fn script(path: &Path, who: &str, log: &Path, then: &str) {
        script_with(path, ":", who, log, then);
    }

    /// A process named `dock` running `program` with the empty asset search path as its argument, which the
    /// [`script_with`] programs export, holding `roles`.
    fn dock(program: &Path, roles: Vec<crate::authorization::ShellRole>) -> ShellProcess {
        ShellProcess {
            name: "dock".into(),
            command: vec![
                program.display().to_string(),
                "CUPERTINO_ASSET_SEARCH_PATH=".into(),
            ],
            roles,
        }
    }

    /// An installed directory and a development folder (mode 0700) under one temporary directory.
    fn folders() -> (tempfile::TempDir, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let (installed, dev) = (base.path().join("bin"), base.path().join("dev"));
        for dir in [&installed, &dev] {
            std::fs::create_dir(dir).unwrap();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        (base, installed, dev)
    }

    fn lines(log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .map_or_else(|_| Vec::new(), |t| t.lines().map(String::from).collect())
    }

    #[test]
    fn a_named_process_starts_the_development_folders_program_with_the_same_arguments() {
        use crate::authorization::ShellRole;
        let (base, installed, dev) = folders();
        let log = base.path().join("starts");
        let program = installed.join("cupertino-dock");
        script(&program, "installed", &log, "exit 1");
        let mut s = Supervisor::new(
            &[dock(&program, vec![ShellRole::Session, ShellRole::Dock])],
            Instant::now(),
        );
        let folder = DevFolder {
            path: dev.clone(),
            components: vec!["dock".into()],
        };
        s.set_dev_folder(Some(&folder));
        assert_eq!(
            s.processes()[0].dev_program,
            Some(dev.join("cupertino-dock"))
        );

        // no file in the folder yet: the installed program
        poll_until(&mut s, |_| lines(&log).len() == 1);
        script(
            &dev.join("cupertino-dock"),
            "development",
            &log,
            "exec sleep 60",
        );
        // the next start, after the throttle, finds it
        poll_until(&mut s, |s| {
            s.processes()[0].state == ProcessState::Running && lines(&log).len() == 2
        });
        assert_eq!(
            lines(&log),
            [
                "installed CUPERTINO_ASSET_SEARCH_PATH=",
                "development CUPERTINO_ASSET_SEARCH_PATH="
            ]
        );
        let info = &s.processes()[0];
        assert!(info.from_dev_folder);
        assert_eq!(info.program, Some(dev.join("cupertino-dock")));
        assert_eq!(info.installed, program);
        assert_eq!(info.start_ticks, start_ticks(info.pid.unwrap()));
        assert!(info.start_ticks.is_some());
    }

    #[test]
    fn a_process_the_folder_does_not_apply_to_starts_its_installed_program() {
        use crate::authorization::ShellRole;
        let (base, installed, dev) = folders();
        let log = base.path().join("starts");
        // a stand-in program: what keeps it from the folder is its role, never its name
        let program = installed.join("lock-screen");
        script(&program, "installed", &log, "exec sleep 60");
        script(
            &dev.join("lock-screen"),
            "development",
            &log,
            "exec sleep 60",
        );
        let login = ShellProcess {
            name: "login".into(),
            ..dock(&program, vec![ShellRole::Session, ShellRole::SessionLock])
        };
        let mut s = Supervisor::new(&[login], Instant::now());
        s.add_built_ins([(BuiltIn::Portal, Some(program.clone()))], Instant::now());
        s.set_dev_folder(Some(&DevFolder {
            path: dev,
            components: vec!["login".into(), "portal".into()],
        }));
        assert!(s.processes().iter().all(|p| p.dev_program.is_none()));
        poll_until(&mut s, |_| lines(&log).len() == 2);
        let started = lines(&log);
        assert_eq!(started.len(), 2, "{started:?}");
        assert!(
            started.iter().all(|l| l.starts_with("installed")),
            "{started:?}"
        );
        assert!(s.processes().iter().all(|p| !p.from_dev_folder));
    }

    #[test]
    fn a_development_program_others_can_write_is_refused_for_the_installed_one() {
        use std::os::unix::fs::PermissionsExt;
        let (base, installed, dev) = folders();
        let log = base.path().join("starts");
        let program = installed.join("cupertino-dock");
        script(&program, "installed", &log, "exec sleep 60");
        let shared = dev.join("cupertino-dock");
        script(&shared, "development", &log, "exec sleep 60");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        let mut s = Supervisor::new(&[dock(&program, Vec::new())], Instant::now());
        s.set_dev_folder(Some(&DevFolder {
            path: dev,
            components: vec!["dock".into()],
        }));
        poll_until(&mut s, |_| !lines(&log).is_empty());
        assert_eq!(s.processes()[0].state, ProcessState::Running);
        assert_eq!(lines(&log), ["installed CUPERTINO_ASSET_SEARCH_PATH="]);
    }

    /// The answers a restart got so far.
    fn restart(
        s: &mut Supervisor,
        name: &str,
    ) -> std::sync::mpsc::Receiver<Result<ProcessInfo, RestartError>> {
        let (tx, rx) = std::sync::mpsc::channel();
        s.request_restart(
            name,
            Instant::now(),
            Box::new(move |answer| tx.send(answer).unwrap()),
        );
        rx
    }

    /// Poll until the restart is answered.
    fn answer(
        s: &mut Supervisor,
        rx: &std::sync::mpsc::Receiver<Result<ProcessInfo, RestartError>>,
    ) -> Result<ProcessInfo, RestartError> {
        let mut got = None;
        poll_until(s, |_| {
            got = rx.try_recv().ok();
            got.is_some()
        });
        assert!(rx.try_recv().is_err(), "answered once");
        got.unwrap()
    }

    #[test]
    fn a_restart_ends_the_process_and_starts_it_at_once_even_when_it_exits_successfully() {
        let (base, installed, dev) = folders();
        let log = base.path().join("starts");
        let program = installed.join("cupertino-dock");
        // SIGTERM ends it with status 0, which alone would leave it stopped
        script_with(
            &program,
            "trap 'exit 0' TERM",
            "installed",
            &log,
            "while :; do sleep 0.05; done",
        );
        let mut s = Supervisor::new(&[dock(&program, Vec::new())], Instant::now());
        s.set_dev_folder(Some(&DevFolder {
            path: dev.clone(),
            components: vec!["dock".into()],
        }));
        // the line is written once the trap is set
        poll_until(&mut s, |_| lines(&log).len() == 1);
        let first = s.processes()[0].clone();

        script(
            &dev.join("cupertino-dock"),
            "development",
            &log,
            "exec sleep 60",
        );
        let begin = Instant::now();
        let rx = restart(&mut s, "dock");
        assert_eq!(s.processes()[0].state, ProcessState::Stopping);
        assert_eq!(
            s.pids(),
            [first.pid.unwrap()],
            "it still counts while it ends"
        );
        let info = answer(&mut s, &rx).unwrap();
        assert!(begin.elapsed() < RESTART_STOP_GRACE, "it ended on SIGTERM");
        assert_eq!(info.state, ProcessState::Running);
        assert_ne!(info.pid, first.pid);
        assert!(info.from_dev_folder);
        assert_eq!(info.program, Some(dev.join("cupertino-dock")));
        assert_eq!(s.processes()[0], info);
        poll_until(&mut s, |_| lines(&log).len() == 2);
        assert_eq!(
            lines(&log),
            [
                "installed CUPERTINO_ASSET_SEARCH_PATH=",
                "development CUPERTINO_ASSET_SEARCH_PATH="
            ]
        );

        // back to the installed program: the file goes, and a restart starts the installed one
        std::fs::remove_file(dev.join("cupertino-dock")).unwrap();
        let rx = restart(&mut s, "dock");
        let info = answer(&mut s, &rx).unwrap();
        assert!(!info.from_dev_folder);
        assert_eq!(info.program, Some(program));
        poll_until(&mut s, |_| lines(&log).len() == 3);
        assert_eq!(
            lines(&log).last().unwrap(),
            "installed CUPERTINO_ASSET_SEARCH_PATH="
        );
    }

    #[test]
    fn a_process_that_ignores_the_restart_is_killed_after_the_grace() {
        let (base, installed, _) = folders();
        let log = base.path().join("starts");
        let program = installed.join("cupertino-dock");
        script_with(
            &program,
            "trap '' TERM",
            "installed",
            &log,
            "while :; do sleep 0.05; done",
        );
        let mut s = Supervisor::new(&[dock(&program, Vec::new())], Instant::now());
        // the line is written once the trap is set
        poll_until(&mut s, |_| lines(&log).len() == 1);
        let begin = Instant::now();
        let rx = restart(&mut s, "dock");
        assert!(
            s.next_deadline()
                .is_some_and(|at| at >= begin + RESTART_STOP_GRACE),
            "the kill is due"
        );
        let info = answer(&mut s, &rx).unwrap();
        assert!(begin.elapsed() >= RESTART_STOP_GRACE);
        assert_eq!(info.state, ProcessState::Running);
        poll_until(&mut s, |_| lines(&log).len() == 2);
    }

    #[test]
    fn a_stopped_process_starts_on_a_restart_and_an_unknown_name_is_refused() {
        let (base, installed, _) = folders();
        let log = base.path().join("starts");
        let program = installed.join("cupertino-dock");
        script(&program, "installed", &log, "exit 0");
        let mut s = Supervisor::new(&[dock(&program, Vec::new())], Instant::now());
        poll_until(&mut s, |s| s.processes()[0].state == ProcessState::Stopped);
        script(&program, "again", &log, "exec sleep 60");
        let rx = restart(&mut s, "dock");
        let info = answer(&mut s, &rx).unwrap();
        assert_eq!(info.state, ProcessState::Running);
        poll_until(&mut s, |_| lines(&log).len() == 2);
        assert_eq!(
            lines(&log),
            [
                "installed CUPERTINO_ASSET_SEARCH_PATH=",
                "again CUPERTINO_ASSET_SEARCH_PATH="
            ]
        );

        let rx = restart(&mut s, "nope");
        assert_eq!(
            rx.try_recv().unwrap(),
            Err(RestartError::Unknown("nope".into()))
        );
    }

    #[test]
    fn a_restart_whose_start_fails_says_why_and_the_process_is_tried_again_later() {
        let mut s = Supervisor::new(
            &[dock(Path::new("/nonexistent/cupertino-dock"), Vec::new())],
            Instant::now(),
        );
        let rx = restart(&mut s, "dock");
        let Err(RestartError::NotStarted(reason)) = answer(&mut s, &rx) else {
            panic!("the start failed");
        };
        assert!(reason.starts_with("spawn: "), "{reason}");
        assert_eq!(s.processes()[0].state, ProcessState::Waiting);
    }

    #[test]
    fn the_start_time_is_field_22_of_the_stat_line_whatever_the_command_name() {
        let line =
            "1234 (a ) b (c) S 1 1234 1234 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 100";
        assert_eq!(parse_start_ticks(line), Some(987654));
        assert_eq!(parse_start_ticks("1234 (x) S 1"), None);
        let ticks = start_ticks(std::process::id()).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let started = started_unix(ticks).unwrap();
        assert!(started <= now && now - started < 3600, "{started} {now}");
    }

    #[test]
    fn successful_process_is_started_once() {
        test_events::memory();
        let dir =
            std::env::temp_dir().join(format!("cupertino-supervisor-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("starts");
        let mut s = Supervisor::new(
            &[spec(&format!("echo start >> {}", log.display()))],
            Instant::now(),
        );
        let mut connect = connect;
        poll_until(&mut s, |s| matches!(s.procs[0].phase, Phase::Stopped));
        // well past the throttle: a restart would have happened by now
        let end = Instant::now() + RESTART_THROTTLE * 2;
        while Instant::now() < end {
            s.poll(Instant::now(), "wayland-test", None, &mut connect);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "start\n");
        std::fs::remove_dir_all(dir).unwrap();
        let stopped = test_events::recorded("server.shell.gave_up");
        assert_eq!(stopped.len(), 1, "{stopped:?}");
        assert_eq!(stopped[0].severity, diag::Severity::Info);
        assert_eq!(stopped[0].fields.get("role").unwrap(), "test");
        assert_eq!(stopped[0].fields.get("status").unwrap(), 0);
        assert!(stopped[0].fields.get("restart_in").is_none());
        assert!(test_events::recorded("server.shell.exited").is_empty());
    }
}
