//! The supervisor starts the shell's hosts (`docs/architecture/shell-host.md`, *Processes and connections*), with this
//! test binary as `cupertino-shell`: started with [`STAND_IN`] it runs the stand-in host of `host-spawning/stand-in.rs`:
//! - the components of the list run in the shared host, a component of `alone` in a separate host
//!   (`cupertino-shell --only NAME`), each on the host connection the server passed; a component the host was built
//!   without does not run, and nothing else starts for it;
//! - without the host program, or with one that cannot start, no component runs, a restart of one is refused as unknown,
//!   and the host is not tried again;
//! - a host that exits is started again no sooner than 1 s after its last start, then 2 s, and an exit the server
//!   expected (`cupertino shell restart host`) is not counted;
//! - a change of the `shell` section moves a component to a separate host and turns it off, at once;
//! - `cupertino shell restart NAME` of a hosted component goes through its host and is answered once it is ready;
//! - a component turned off holds nothing any more: what was held for it, its reserved space too, ends at once;
//! - a host whose connection cannot be bound to its process never ran: the server hears it as such, not as an exit.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_assets::Assets;
use cupertino_defaults::compositor::CompositorSettings;
use cupertino_server::config::ShellProcess;
use cupertino_server::host::{HostKind, HostView, COMPONENTS};
use cupertino_server::shell_connection::BoundConnection;
use cupertino_server::state::new_output;
use cupertino_server::supervisor::{
    HostProgram, Launcher, ProcessInfo, ProcessState, RestartError, Supervisor,
};
use cupertino_server::{Config, Cupertino};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;

#[path = "host-spawning/stand-in.rs"]
mod stand_in;

/// How long a test waits for the stand-in host to do what it is asked.
const WAIT: Duration = Duration::from_secs(20);

/// The argument that makes this test binary the stand-in host: a filter after `--` that names no test.
const STAND_IN: &str = "stand-in";

/// The stand-in host itself, when the supervisor started this binary with [`STAND_IN`]; a test run does nothing here.
#[test]
fn stand_in_host() {
    if std::env::args().any(|arg| arg == STAND_IN) {
        stand_in::run();
    }
}

/// This test binary, running [`stand_in_host`] alone: the host program and its leading arguments.
fn stand_in() -> (PathBuf, Vec<String>) {
    let args = ["--exact", "stand_in_host", "--nocapture", "--", STAND_IN];
    (
        std::env::current_exe().unwrap(),
        args.map(String::from).to_vec(),
    )
}

struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    output_id: cupertino_wm::OutputId,
    /// When the first supervision pass ran: the start of what it started.
    started: Instant,
}

impl Rig {
    /// A server with one output whose hosts start `host`, its plan applied and its first supervision pass run.
    fn new(config: Config, host: Option<(PathBuf, Vec<String>)>) -> Self {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .expect("server starts");
        let output_id = state.add_output(new_output(
            "headless-1",
            PhysicalProperties {
                size: (300, 200).into(),
                subpixel: Subpixel::Unknown,
                make: "Cupertino".into(),
                model: "Test".into(),
            },
            OutputMode {
                size: (800, 600).into(),
                refresh: 60_000,
            },
            1.0,
            Transform::Normal,
            (0, 0),
        ));
        state
            .supervisor
            .set_host_program(host.map(|(path, args)| HostProgram {
                path,
                args,
                checked: false,
            }));
        state.apply_shell_plan();
        let started = Instant::now();
        state.supervise_shell(started);
        Self {
            event_loop,
            state,
            output_id,
            started,
        }
    }

    /// One turn of the server's loop: the hosts' requests are read and answered.
    fn turn(&mut self) {
        let _ = self
            .event_loop
            .dispatch(Some(Duration::from_millis(10)), &mut self.state);
        self.state.tick();
        let _ = self.state.display_handle.flush_clients();
    }

    /// Run the server until `done` holds.
    fn until(&mut self, what: &str, done: impl Fn(&Cupertino) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.state) {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            self.turn();
        }
    }

    fn process(&self, name: &str) -> Option<ProcessInfo> {
        process(&self.state, name)
    }

    fn host(&self, kind: HostKind) -> Option<HostView> {
        host(&self.state, kind)
    }
}

fn process(state: &Cupertino, name: &str) -> Option<ProcessInfo> {
    state
        .supervisor
        .processes()
        .into_iter()
        .find(|p| p.name == name)
}

fn running(state: &Cupertino, name: &str) -> bool {
    process(state, name).is_some_and(|p| p.state == ProcessState::Running)
}

fn host(state: &Cupertino, kind: HostKind) -> Option<HostView> {
    state
        .hosts
        .host_views()
        .into_iter()
        .find(|h| h.kind == kind)
}

/// The host of `kind` runs, bound to its process, and bound `cupertino_host_v1`.
fn host_up(state: &Cupertino, kind: HostKind) -> bool {
    host(state, kind).is_some_and(|h| h.bound && h.pid.is_some())
}

/// The arguments of process `pid`.
fn arguments(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap()
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

/// Kill process `pid` (a child of the server's supervisor) and wait until it has exited, without reaping it.
fn kill_and_wait(pid: u32) {
    // Safety: plain system calls on a child of this process that is not reaped yet
    unsafe {
        assert_eq!(libc::kill(pid as i32, libc::SIGKILL), 0);
        let mut info: libc::siginfo_t = std::mem::zeroed();
        assert_eq!(
            libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT),
            0
        );
    }
}

/// Every component of the table but the ones named.
fn components_but(names: &[&str]) -> Vec<&'static str> {
    COMPONENTS
        .iter()
        .map(|c| c.name)
        .filter(|name| !names.contains(name))
        .collect()
}

/// A restart's answer, through a channel the reply sends on.
fn restart(rig: &mut Rig, name: &str) -> mpsc::Receiver<Result<ProcessInfo, RestartError>> {
    let (tx, rx) = mpsc::channel();
    rig.state.restart_shell_process(
        name,
        Box::new(move |answer| {
            let _ = tx.send(answer);
        }),
    );
    rx
}

fn answer(
    rig: &mut Rig,
    rx: &mpsc::Receiver<Result<ProcessInfo, RestartError>>,
) -> Result<ProcessInfo, RestartError> {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(answer) = rx.try_recv() {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the restart's answer"
        );
        rig.turn();
    }
}

#[test]
fn hosted_components_run_in_the_shared_host_and_alone_ones_in_a_separate_host() {
    let config = Config {
        shell_alone: vec!["dock".into()],
        ..Config::default()
    };
    let mut rig = Rig::new(config, Some(stand_in()));

    rig.until("both hosts bound", |s| {
        host_up(s, HostKind::Shared) && host_up(s, HostKind::Separate("dock"))
    });
    let shared = rig.process("host").expect("the shared host is supervised");
    assert_eq!(shared.state, ProcessState::Running);
    assert_eq!(rig.host(HostKind::Shared).unwrap().pid, shared.pid);
    let alone = rig
        .process("host-dock")
        .expect("the separate host is supervised");
    assert_eq!(rig.host(HostKind::Separate("dock")).unwrap().pid, alone.pid);
    assert!(
        arguments(alone.pid.unwrap()).ends_with(&["--only".to_string(), "dock".to_string()]),
        "a separate host is `cupertino-shell --only NAME`"
    );
    assert_eq!(
        rig.host(HostKind::Separate("dock")).unwrap().components,
        vec!["dock"]
    );

    assert_eq!(
        rig.host(HostKind::Shared).unwrap().components,
        components_but(&["dock"]),
        "the shared host runs the rest of the table, the session agent `agent` among them"
    );
    rig.until("every component the host has is connected", |s| {
        components_but(&["dialogs"])
            .iter()
            .all(|c| s.hosts.component_connected(c))
    });
    assert!(rig.state.hosts.component_connected("agent"));
    // the stand-in was built without `dialogs`: it does not run, and the server starts nothing else for it
    assert!(!rig.state.hosts.component_connected("dialogs"));
    let names: Vec<String> = rig
        .state
        .supervisor
        .processes()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(
        names,
        ["host", "host-dock"],
        "the hosts alone are supervised"
    );

    // `cupertino shell restart menubar` goes through the shared host and is answered once it is ready
    let rx = restart(&mut rig, "menubar");
    let info = answer(&mut rig, &rx).expect("the restarted component is ready");
    assert_eq!(info.name, "menubar");
    assert_eq!(info.pid, shared.pid, "the shared host runs it again");
    assert_eq!(rig.state.hosts.component_instance("menubar"), 2);
    assert_eq!(
        rig.process("host").unwrap().pid,
        shared.pid,
        "the host itself was not restarted"
    );

    // a component of a separate host: its host starts again
    let rx = restart(&mut rig, "dock");
    let info = answer(&mut rig, &rx).expect("the restarted component is ready");
    assert_eq!(info.name, "dock");
    assert_ne!(info.pid, alone.pid, "a fresh separate host runs it");
    assert_eq!(rig.process("host-dock").unwrap().pid, info.pid);
    assert_eq!(rig.state.hosts.component_instance("dock"), 2);
}

#[test]
fn without_a_host_program_no_component_runs() {
    // not installed
    let mut rig = Rig::new(Config::default(), None);
    assert!(rig.state.supervisor.processes().is_empty());
    assert!(rig.state.hosts.host_views().is_empty());
    assert!(
        rig.state
            .shell_components()
            .iter()
            .all(|(_, runs_in, _, connected)| runs_in == "none" && !connected),
        "{:?}",
        rig.state.shell_components()
    );
    let rx = restart(&mut rig, "dock");
    assert!(
        matches!(answer(&mut rig, &rx), Err(RestartError::Unknown(name)) if name == "dock"),
        "a component no host runs is no process to restart"
    );

    // installed, but it cannot start: refused once, then never tried again
    let missing = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("no-such-cupertino-shell");
    let mut rig = Rig::new(Config::default(), Some((missing, Vec::new())));
    let start = rig.started;
    assert!(rig.state.supervisor.host_refused().is_some());
    rig.state.supervise_shell(start + Duration::from_secs(120));
    assert!(
        rig.state.supervisor.processes().is_empty(),
        "no restart loop, and nothing runs in the host's place"
    );
    assert!(rig.state.hosts.host_views().is_empty());
}

#[test]
fn a_host_that_exits_waits_longer_each_time_and_an_expected_exit_is_not_counted() {
    let config = Config {
        shell_components: vec!["menubar".into()],
        ..Config::default()
    };
    let mut rig = Rig::new(config, Some(stand_in()));
    let t0 = rig.started;
    // only the passes this test runs, at the times it gives
    if let Some((token, _)) = rig.state.supervision_wakeups.timer.take() {
        rig.state.loop_handle.remove(token);
    }
    rig.until("the shared host is bound", |s| host_up(s, HostKind::Shared));

    // first exit: started again 1 s after its last start
    kill_and_wait(rig.process("host").unwrap().pid.unwrap());
    rig.state.supervise_shell(t0 + Duration::from_millis(100));
    assert_eq!(rig.process("host").unwrap().state, ProcessState::Waiting);
    assert_eq!(
        rig.state.supervisor.next_deadline(),
        Some(t0 + Duration::from_secs(1))
    );
    rig.state.supervise_shell(t0 + Duration::from_millis(900));
    assert_eq!(rig.process("host").unwrap().state, ProcessState::Waiting);
    let second = t0 + Duration::from_secs(1);
    rig.state.supervise_shell(second);
    assert_eq!(rig.process("host").unwrap().state, ProcessState::Running);
    rig.until("the shared host is bound again", |s| {
        host_up(s, HostKind::Shared)
    });

    // second exit: the wait doubles
    kill_and_wait(rig.process("host").unwrap().pid.unwrap());
    rig.state
        .supervise_shell(second + Duration::from_millis(100));
    assert_eq!(rig.process("host").unwrap().state, ProcessState::Waiting);
    assert_eq!(
        rig.state.supervisor.next_deadline(),
        Some(second + Duration::from_secs(2))
    );
    rig.state.supervise_shell(second + Duration::from_secs(2));
    rig.until("the shared host is bound a third time", |s| {
        host_up(s, HostKind::Shared)
    });
    assert!(!rig.state.hosts.all_separate_for_session);

    // `cupertino shell restart host`: expected, so started again at once and not counted as the third crash
    let before = rig.process("host").unwrap().pid;
    let rx = restart(&mut rig, "host");
    let info = answer(&mut rig, &rx).expect("the host runs again");
    assert_ne!(info.pid, before);
    rig.until("the restarted host is bound", |s| {
        host_up(s, HostKind::Shared)
    });
    assert!(
        !rig.state.hosts.all_separate_for_session,
        "the expected exit is not the third crash"
    );
}

#[test]
fn a_change_of_the_shell_section_moves_and_stops_a_component_at_once() {
    let mut rig = Rig::new(Config::default(), Some(stand_in()));
    rig.until("menubar runs in the shared host", |s| {
        host_up(s, HostKind::Shared) && s.hosts.component_connected("menubar")
    });
    let bar = f64::from(stand_in::BAR_HEIGHT);
    let output = rig.output_id;
    rig.until("the menu bar reserves its space", |s| {
        s.usable_area(output).y == bar
    });
    let settings = |json: &str| CompositorSettings::parse(json, None).0;
    let none = CompositorSettings::default();

    // `alone` in the person's file: the menu bar leaves the shared host for one of its own
    let alone = settings(r#"{"shell": {"alone": ["menubar"]}}"#);
    rig.state
        .apply_compositor_layers((&none, &none), (&none, &alone));
    rig.until("menubar runs alone", |s| {
        host_up(s, HostKind::Separate("menubar")) && s.hosts.component_connected("menubar")
    });
    assert!(running(&rig.state, "host-menubar"));
    rig.until("the separate host's menu bar took the bar over", |s| {
        s.hosts.held_surfaces().is_empty() && s.usable_area(output).y == bar
    });
    assert!(!rig
        .host(HostKind::Shared)
        .unwrap()
        .components
        .contains(&"menubar"));

    // its host ends: the bar is held, the space it reserves too (no pass starts the host again meanwhile)
    if let Some((token, _)) = rig.state.supervision_wakeups.timer.take() {
        rig.state.loop_handle.remove(token);
    }
    kill_and_wait(rig.process("host-menubar").unwrap().pid.unwrap());
    rig.until("the menu bar is held", |s| {
        s.hosts.held_surfaces().len() == 1
    });
    assert_eq!(rig.state.usable_area(output).y, bar);

    // `off`: what was held for it ends at once; its separate host is not started again
    let off = settings(r#"{"shell": {"off": ["menubar"]}}"#);
    rig.state
        .apply_compositor_layers((&none, &alone), (&none, &off));
    assert!(
        rig.state.hosts.held_surfaces().is_empty(),
        "a component turned off holds nothing"
    );
    assert_eq!(
        rig.state.usable_area(output).y,
        0.0,
        "its reserved space ends with it"
    );
    rig.until("the separate host is gone", |s| {
        process(s, "host-menubar").is_none() && host(s, HostKind::Separate("menubar")).is_none()
    });
    assert!(!rig.state.hosts.component_connected("menubar"));
    assert!(running(&rig.state, "host"), "the shared host runs on");
}

/// A launcher whose hosts' connections cannot be bound: it records what the supervisor tells it.
#[derive(Default)]
struct BindRefused {
    ended: Vec<u64>,
    unbound: Vec<u64>,
}

/// The id [`BindRefused`] gives every host connection.
const UNBOUND_HOST: u64 = 7;

impl Launcher for BindRefused {
    fn connect(&mut self, _: &ShellProcess) -> std::io::Result<BoundConnection> {
        Err(std::io::Error::other("no built-in programs here"))
    }

    fn connect_host(&mut self, _: HostKind) -> std::io::Result<(UnixStream, u64)> {
        let (client_end, _server_end) = UnixStream::pair()?;
        Ok((client_end, UNBOUND_HOST))
    }

    fn host_started(&mut self, _: u64, _: u32, _: bool) -> std::io::Result<()> {
        Err(std::io::Error::other("binding refused"))
    }

    fn host_ended(&mut self, host_id: u64, _: ExitStatus, _: bool) -> bool {
        self.ended.push(host_id);
        false
    }

    fn host_unbound(&mut self, host_id: u64, _: bool) {
        self.unbound.push(host_id);
    }

    fn host_refused(&mut self, kind: HostKind, _: Option<u64>, reason: &str) {
        panic!("the host {kind:?} was refused: {reason}");
    }
}

#[test]
fn a_host_whose_connection_cannot_be_bound_never_ran() {
    let now = Instant::now();
    let mut supervisor = Supervisor::default();
    let (program, args) = stand_in();
    supervisor.set_host_program(Some(HostProgram {
        path: program,
        args,
        checked: false,
    }));
    supervisor.reconcile(&[(HostKind::Shared, None)], now);
    let mut launcher = BindRefused::default();
    supervisor.poll(now, None, None, &mut launcher);
    assert_eq!(
        launcher.unbound,
        vec![UNBOUND_HOST],
        "the server hears that the host never ran"
    );
    assert!(
        launcher.ended.is_empty(),
        "and no exit it would count as a crash"
    );
}
