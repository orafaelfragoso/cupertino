//! Force Quit against a headless compositor and a client on its socket. The test clients are threads of this process, so
//! the signals are recorded instead of sent. The model clock stands still in these tests and moves where the test moves
//! it. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::cell::RefCell;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::Client;
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::caller::{Caller, Denial, Identified, Sandbox};
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::force_quit::ProcessHandle;
use cupertino_server::supervisor::Supervisor;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    now: f64,
    /// The roundtrips of the test's clients, for [`Harness::settle`].
    clients: Vec<common::Rounds>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        match headless::init(&mut state, (1000, 700), 1.0) {
            Ok(()) => Some(Harness {
                state,
                event_loop,
                now: 100.0,
                clients: Vec::new(),
            }),
            Err(e) => {
                common::egl_unavailable(format_args!("{e:#}"));
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.state.fixed_time = Some(self.now);
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Step at the model time `now` until each client has every event sent before and the server handled their
    /// answers ([`Client::barrier`]).
    fn settle(&mut self) {
        let barriers: Vec<_> = self.clients.iter().map(common::Rounds::barrier).collect();
        self.wait_until("the clients' roundtrips", move |_| {
            barriers.iter().all(|b| b())
        });
    }

    /// `client`, which [`Harness::settle`] waits for from now on.
    fn track(&mut self, client: Client) -> Client {
        self.clients.push(client.rounds());
        client
    }

    /// Move the model clock on by `seconds`, a few steps so that every tick sees it.
    fn advance(&mut self, seconds: f64) {
        common::set_time(&mut self.state, &mut self.event_loop, self.now);
        self.advance_without_pong(seconds);
    }

    /// Deliberately let unanswered pings time out when exercising an unresponsive client.
    fn advance_without_pong(&mut self, seconds: f64) {
        self.now += seconds;
        self.settle();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }
}

fn mapped_window(h: &mut Harness) -> (Client, cupertino_wm::WindowId) {
    let client = h.track(Client::toplevel(h.socket_path()));
    h.wait_until("the window to map", |s| s.windows.iter().any(|e| e.mapped));
    let id = h.state.windows.iter().find(|e| e.mapped).unwrap().id;
    h.settle();
    (client, id)
}

const APP: &str = "org.example.locked-out";

/// Record the signals instead of sending them. The test clients are threads of this process, which the server would refuse
/// to signal, so the server is told its process id is another one.
fn record_signals(h: &mut Harness) -> Rc<RefCell<Vec<(u32, i32)>>> {
    h.state.force_quits.own_pid = u32::MAX;
    let sent = Rc::new(RefCell::new(Vec::new()));
    let recorder = sent.clone();
    h.state.send_signal =
        Box::new(move |process, signal| recorder.borrow_mut().push((process.pid, signal)));
    sent
}

#[test]
fn an_application_that_answers_is_sent_sigterm_and_sigkill_three_seconds_later() {
    let Some(mut h) = Harness::new() else { return };
    let (_client, _id) = mapped_window(&mut h);
    let sent = record_signals(&mut h);
    let pid = std::process::id();

    h.state.force_quit_app(APP);
    assert_eq!(*sent.borrow(), vec![(pid, 15)]);
    assert_eq!(h.state.force_quits.pending(), 1);

    h.advance(2.5);
    assert_eq!(*sent.borrow(), vec![(pid, 15)], "2.5 s: still in the grace");
    h.advance(1.0);
    assert_eq!(*sent.borrow(), vec![(pid, 15), (pid, 9)], "3.5 s");
    assert_eq!(h.state.force_quits.pending(), 0);
}

#[test]
fn log_out_force_quits_through_the_same_rules() {
    let Some(mut h) = Harness::new() else { return };
    let (_client, _id) = mapped_window(&mut h);
    let sent = record_signals(&mut h);

    assert_eq!(h.state.force_quit_application(APP), 1);
    assert_eq!(*sent.borrow(), vec![(std::process::id(), 9)]);

    sent.borrow_mut().clear();
    h.state.force_quits.own_pid = std::process::id();
    assert_eq!(
        h.state.force_quit_application(APP),
        0,
        "the server's own process"
    );
    assert_eq!(*sent.borrow(), vec![]);
}

/// A caller the bus daemon named: the process `pid` on the host.
fn host_caller(pid: u32) -> Caller {
    Caller::Process(Identified {
        process: ProcessHandle::open(pid).unwrap(),
        sandbox: None,
        exe: None,
    })
}

#[test]
fn log_out_force_quit_over_the_bus_is_refused_to_every_caller_but_the_session_agent() {
    let Some(mut h) = Harness::new() else { return };
    let (_client, _id) = mapped_window(&mut h);
    let sent = record_signals(&mut h);
    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();

    assert_eq!(
        h.state
            .force_quit_application_for(&host_caller(other.id()), APP),
        Err(Denial::Untrusted)
    );
    assert_eq!(
        h.state.force_quit_application_for(&Caller::Anonymous, APP),
        Err(Denial::Anonymous)
    );
    let Caller::Process(mut flatpak) = host_caller(other.id()) else {
        unreachable!()
    };
    flatpak.sandbox = Some(Sandbox::Flatpak);
    assert_eq!(
        h.state
            .force_quit_application_for(&Caller::Process(flatpak), APP),
        Err(Denial::Sandboxed(Sandbox::Flatpak))
    );
    assert_eq!(*sent.borrow(), vec![]);
    other.kill().unwrap();
    other.wait().unwrap();
}

#[test]
fn log_out_force_quit_over_the_bus_is_done_for_the_session_agent() {
    let Some(mut h) = Harness::new() else { return };
    let (_client, _id) = mapped_window(&mut h);
    let sent = record_signals(&mut h);
    let agent = ShellProcess {
        name: "agent".into(),
        command: vec!["sleep".into(), "30".into()],
        roles: vec![ShellRole::SessionLock],
    };
    h.state.supervisor = Supervisor::new(&[agent], Instant::now());
    h.state
        .supervisor
        .poll(Instant::now(), "unused", None, &mut |_| {
            UnixStream::pair().map(|(ours, _)| (ours, std::sync::Arc::new(Default::default())))
        });
    let pid = h.state.supervisor.pid_of("agent").unwrap();

    let answer = h.state.force_quit_application_for(&host_caller(pid), APP);
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
    assert_eq!(answer, Ok(1));
    assert_eq!(*sent.borrow(), vec![(std::process::id(), 9)]);
}

#[test]
fn an_application_that_does_not_answer_is_killed_at_once() {
    let Some(mut h) = Harness::new() else { return };
    let (client, id) = mapped_window(&mut h);
    let sent = record_signals(&mut h);
    client.mute(true);
    h.advance_without_pong(1.1);
    h.advance_without_pong(2.5);
    assert!(h.state.window_is_unresponsive(id));

    h.state.force_quit_app(APP);
    assert_eq!(*sent.borrow(), vec![(std::process::id(), 9)]);
    assert_eq!(h.state.force_quits.pending(), 0);
}

#[test]
fn an_application_that_is_not_running_is_left_alone() {
    let Some(mut h) = Harness::new() else { return };
    let (_client, _id) = mapped_window(&mut h);
    let sent = record_signals(&mut h);

    h.state.force_quit_app("org.example.other");
    assert_eq!(*sent.borrow(), vec![]);
}

#[test]
fn the_servers_own_process_is_never_signalled() {
    let Some(mut h) = Harness::new() else { return };
    let (_client, _id) = mapped_window(&mut h);
    let sent = Rc::new(RefCell::new(Vec::new()));
    let recorder = sent.clone();
    h.state.send_signal =
        Box::new(move |process, signal| recorder.borrow_mut().push((process.pid, signal)));

    h.state.force_quit_app(APP);
    assert_eq!(
        *sent.borrow(),
        vec![],
        "the client is a thread of the server's process"
    );
    assert_eq!(h.state.force_quits.pending(), 0);
}
