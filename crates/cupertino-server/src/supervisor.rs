//! Supervision of the shell processes named in the configuration and of the built-in ones ([`BuiltIn`], their program checked before every start): started with the server,
//! restarted by [`crate::restart_policy`], and killed when the server goes away. A pass of
//! [`Supervisor::poll`] runs when a child's pidfd becomes readable (it exited) or at the earliest
//! restart ([`Supervisor::next_deadline`]), see [`Wakeups`]; `try_wait` reaps it there. The server
//! installs no SIGCHLD handler, and nothing wakes the loop while every child runs.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use crate::config::{check_built_in, BuiltIn, ShellProcess};
use crate::restart_policy::{decide, Backoff, Decision};
use crate::shell_connection::{bind_holder, hand_over, BoundConnection};
use crate::xwayland::XDisplay;

enum Phase {
    /// Not running; start at this time.
    Waiting(Instant),
    Running {
        child: Child,
        started: Instant,
    },
    /// Exited successfully: never restarted.
    Stopped,
}

struct Supervised {
    spec: ShellProcess,
    phase: Phase,
    backoff: Backoff,
    /// A built-in process: its program is checked again before every start ([`check_built_in`]).
    built_in: bool,
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
                .map(|spec| Supervised {
                    spec: spec.clone(),
                    phase: Phase::Waiting(now),
                    backoff: Backoff::default(),
                    built_in: false,
                })
                .collect(),
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
                Some(program) => self.procs.push(Supervised {
                    spec: built_in.process(&program),
                    phase: Phase::Waiting(now),
                    backoff: Backoff::default(),
                    built_in: true,
                }),
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
            p.phase = match std::mem::replace(&mut p.phase, Phase::Stopped) {
                Phase::Waiting(at) if at <= now => start(
                    &p.spec,
                    p.built_in,
                    socket,
                    x11,
                    connect,
                    now,
                    &mut p.backoff,
                ),
                Phase::Running { mut child, started } => match child
                    .try_wait()
                    .expect("waiting on a child the supervisor started")
                {
                    None => Phase::Running { child, started },
                    Some(status) => {
                        tracing::info!(name = p.spec.name, %status, "shell process exited");
                        match decide(status, started, now, &mut p.backoff) {
                            Decision::RestartNow => start(
                                &p.spec,
                                p.built_in,
                                socket,
                                x11,
                                connect,
                                now,
                                &mut p.backoff,
                            ),
                            Decision::RestartAt(at) => Phase::Waiting(at),
                            Decision::Stop => Phase::Stopped,
                        }
                    }
                },
                other => other,
            };
        }
    }

    /// The pids of the running processes.
    pub fn pids(&self) -> Vec<u32> {
        self.procs
            .iter()
            .filter_map(|p| match &p.phase {
                Phase::Running { child, .. } => Some(child.id()),
                Phase::Waiting(_) | Phase::Stopped => None,
            })
            .collect()
    }

    /// The pids of the running processes whose connection holds `role`.
    pub fn pids_with_role(&self, role: crate::authorization::ShellRole) -> Vec<u32> {
        self.procs
            .iter()
            .filter(|p| p.spec.roles.contains(&role))
            .filter_map(|p| match &p.phase {
                Phase::Running { child, .. } => Some(child.id()),
                Phase::Waiting(_) | Phase::Stopped => None,
            })
            .collect()
    }

    /// When the next restart is due: the earliest start of a process waiting out its throttle.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.procs
            .iter()
            .filter_map(|p| match p.phase {
                Phase::Waiting(at) => Some(at),
                Phase::Running { .. } | Phase::Stopped => None,
            })
            .min()
    }

    /// The pid of the running process named `name`; `None` while it is not running.
    pub fn pid_of(&self, name: &str) -> Option<u32> {
        self.procs
            .iter()
            .find(|p| p.spec.name == name)
            .and_then(|p| match &p.phase {
                Phase::Running { child, .. } => Some(child.id()),
                Phase::Waiting(_) | Phase::Stopped => None,
            })
    }
}

/// Spawn the process; a failed spawn counts as an exit with failure, so it is retried after the throttle, which grows.
/// The program of a `built_in` process that [`check_built_in`] refuses is not started, and is checked again then.
fn start(
    spec: &ShellProcess,
    built_in: bool,
    socket: &str,
    x11: Option<&XDisplay>,
    connect: &mut dyn FnMut(&ShellProcess) -> std::io::Result<BoundConnection>,
    now: Instant,
    backoff: &mut Backoff,
) -> Phase {
    let (program, args) = spec
        .command
        .split_first()
        .expect("shell process commands are validated non-empty at load");
    if built_in {
        if let Err(reason) = check_built_in(Path::new(program)) {
            tracing::warn!(name = spec.name, %reason, "the built-in program is not started");
            return Phase::Waiting(now + backoff.next_interval(Duration::ZERO));
        }
    }
    let (stream, state) = match connect(spec) {
        Ok(connection) => connection,
        Err(e) => {
            tracing::warn!(name = spec.name, error = %e, "creating the shell process's connection failed");
            return Phase::Waiting(now + backoff.next_interval(Duration::ZERO));
        }
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .env("WAYLAND_DISPLAY", socket)
        .env("XDG_CURRENT_DESKTOP", crate::state::CURRENT_DESKTOP)
        .env("XDG_SESSION_TYPE", "wayland");
    crate::xwayland::set_client_env(&mut command, x11);
    hand_over(&mut command, &stream);
    match command.spawn() {
        Ok(mut child) => {
            if let Err(e) = bind_holder(&state, child.id()) {
                tracing::warn!(name = spec.name, error = %e, "binding the shell process to its connection failed: ending it");
                let _ = child.kill();
                let _ = child.wait();
                return Phase::Waiting(now + backoff.next_interval(Duration::ZERO));
            }
            tracing::info!(name = spec.name, pid = child.id(), "shell process started");
            Phase::Running {
                child,
                started: now,
            }
        }
        Err(e) => {
            tracing::warn!(name = spec.name, program, error = %e, "starting shell process failed");
            Phase::Waiting(now + backoff.next_interval(Duration::ZERO))
        }
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
            if let Phase::Running { child, .. } = &mut p.phase {
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
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o775)).unwrap();
        let end = Instant::now() + 3 * RESTART_THROTTLE;
        let mut connect = connect;
        while Instant::now() < end {
            s.poll(Instant::now(), "wayland-test", None, &mut connect);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(starts(), 1, "a group-writable program was started again");
        assert!(s.next_deadline().is_some(), "it is checked again later");
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

    #[test]
    fn successful_process_is_started_once() {
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
    }
}
