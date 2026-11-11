//! One controller connection holding a session per agent it serves (version 5): each run names its own bridge and is
//! decided by that app's remembered answer, holds its own keys like a device of its own, owns the capture sources it
//! selected, and ends alone. Runs that must ask the person are asked one at a time.
use super::permissions::{this_program, Bridge, Store, ALLOW_ALL};
use super::*;
use cupertino_defaults::desktop_control::{self as remembered, Decision, Service};

/// The first version whose connection holds several sessions.
const CONCURRENT: u32 = 5;
/// Sessions a connection may hold at once; the controller serves as many agents.
const RUNS: usize = cupertino_control_model::MAX_RUNS_PER_CONTROLLER;

impl Dispatch<Session, usize> for Capturer {
    fn event(
        s: &mut Self,
        _: &Session,
        e: session::Event,
        slot: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if s.control_runs.len() <= *slot {
            s.control_runs.resize_with(*slot + 1, Seen::default);
        }
        let s = &mut s.control_runs[*slot];
        match e {
            session::Event::Created {
                epoch_hi, epoch_lo, ..
            } => s.epoch = (u64::from(epoch_hi) << 32) | u64::from(epoch_lo),
            session::Event::Status { phase, .. } => s.phase = phase.into(),
            session::Event::Geometry { revision, .. } => s.revision = revision,
            session::Event::Completed { serial } => s.completed.push(serial),
            session::Event::Refused { serial, reason } => {
                s.refused.push((serial, reason.into_result().unwrap()))
            }
            _ => {}
        }
    }
}

/// A bridge of another app than [`Bridge`]'s: a shell the test starts runs it, so the app is the shell's program.
struct ShellBridge {
    pid: u32,
    start: u64,
    app: remembered::App,
    shell: std::process::Child,
}

impl ShellBridge {
    fn spawn() -> Self {
        use std::io::BufRead;
        let mut shell = std::process::Command::new("sh")
            .args(["-c", "sleep 60 & echo $!; wait"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(shell.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pid: u32 = line.trim().parse().unwrap();
        let (parent, start) = cupertino_server::process::parent_and_start(pid).unwrap();
        assert_eq!(parent, shell.id(), "the shell started the bridge");
        let program = std::fs::read_link(format!("/proc/{}/exe", shell.id())).unwrap();
        Self {
            pid,
            start,
            app: remembered::App::Executable(program),
            shell,
        }
    }
}

impl Drop for ShellBridge {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .arg(self.pid.to_string())
            .status();
        let _ = self.shell.kill();
        let _ = self.shell.wait();
    }
}

/// One session of [`Runs`].
struct Run {
    session: Session,
    hi: u32,
    lo: u32,
}

/// A controller connection with several sessions, each run's events kept apart by its slot.
struct Runs {
    client: TestClient<Capturer>,
    manager: Manager,
    output: wl_output::WlOutput,
    runs: Vec<Run>,
    next: u32,
}

impl Runs {
    fn connect(h: &mut Harness, version: u32) -> Self {
        let stream =
            shell_process_connection(&h.state.display_handle, &[ShellRole::DesktopControl])
                .unwrap();
        let client = TestClient::connect(h, stream, Capturer::default());
        let qh = client.qh();
        let manager: Manager = client.globals.bind(&qh, version..=version, ()).unwrap();
        let output = client.globals.bind(&qh, 1..=4, ()).unwrap();
        Self {
            client,
            manager,
            output,
            runs: Vec::new(),
            next: 1,
        }
    }

    /// A new session naming the process `bridge` (pid and start) serves; its slot.
    fn open(&mut self, h: &mut Harness, bridge: (u32, u64)) -> usize {
        let slot = self.runs.len();
        let session = self.manager.get_session(&self.client.qh(), slot);
        h.pump_until(&mut self.client, "the run epoch", |s| {
            s.control_runs.get(slot).is_some_and(|r| r.epoch != 0)
        });
        let epoch = self.client.state.control_runs[slot].epoch;
        session.identify(bridge.0, (bridge.1 >> 32) as u32, bridge.1 as u32);
        self.runs.push(Run {
            session,
            hi: (epoch >> 32) as u32,
            lo: epoch as u32,
        });
        slot
    }

    fn seen(&self, slot: usize) -> &Seen {
        &self.client.state.control_runs[slot]
    }

    fn epoch(&self, slot: usize) -> u64 {
        self.seen(slot).epoch
    }

    fn serial(&mut self) -> u32 {
        let n = self.next;
        self.next += 1;
        n
    }

    fn ack(&mut self, h: &mut Harness, slot: usize, serial: u32) -> Result<(), session::Refusal> {
        h.pump_until(&mut self.client, "the control reply", |s| {
            let r = &s.control_runs[slot];
            r.completed.contains(&serial) || r.refused.iter().any(|(n, _)| *n == serial)
        });
        self.seen(slot)
            .refused
            .iter()
            .find(|(n, _)| *n == serial)
            .map_or(Ok(()), |(_, e)| Err(*e))
    }

    fn start(&mut self, h: &mut Harness, slot: usize, bits: u32) -> Result<(), session::Refusal> {
        let n = self.serial();
        let r = &self.runs[slot];
        r.session.start(
            n,
            r.hi,
            r.lo,
            &self.output,
            session::Capability::from_bits_retain(bits),
        );
        self.ack(h, slot, n)
    }

    fn key(
        &mut self,
        h: &mut Harness,
        slot: usize,
        code: u32,
        pressed: bool,
    ) -> Result<(), session::Refusal> {
        let n = self.serial();
        let r = &self.runs[slot];
        r.session.key(
            n,
            r.hi,
            r.lo,
            self.client.state.control_runs[slot].revision,
            code,
            if pressed {
                session::KeyState::Pressed
            } else {
                session::KeyState::Released
            },
        );
        self.ack(h, slot, n)
    }

    fn phase(&mut self, h: &mut Harness, slot: usize, phase: u32) {
        h.pump_until(&mut self.client, "the control phase", |s| {
            s.control_runs[slot].phase == phase
        });
    }

    /// An output capture source of the run in `slot`, which selects it first.
    fn source(&self, slot: usize) -> ext_image_capture_source_v1::ExtImageCaptureSourceV1 {
        self.runs[slot].session.select_capture();
        let qh = self.client.qh();
        let manager:ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1=self.client.globals.bind(&qh,1..=1,()).unwrap();
        manager.create_source(&self.output, &qh, ())
    }

    /// A capture session on `source` is stopped at once: the source does not capture.
    fn stopped_source(
        &mut self,
        h: &mut Harness,
        source: &ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ) {
        self.client.state.session = SessionSeen::default();
        let qh = self.client.qh();
        let manager: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 =
            self.client.globals.bind(&qh, 1..=1, ()).unwrap();
        let s = manager.create_session(
            source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &qh,
            (),
        );
        h.pump_until(&mut self.client, "a source that does not capture", |s| {
            s.session.stopped
        });
        assert_eq!(self.client.state.session.size, None);
        s.destroy();
    }

    /// A frame of `capture` arrives intact.
    fn frame(&mut self, h: &mut Harness, capture: &Capture, what: &str) {
        let frame = self.client.request_frame(capture);
        h.state.request_redraw();
        h.pump_until(&mut self.client, what, |s| {
            s.frame.ready || s.frame.failed.is_some()
        });
        assert!(
            self.client.state.frame.failed.is_none(),
            "{what}: {:?}",
            self.client.state.frame.failed
        );
        frame.destroy();
    }
}

/// A fresh serial of the person's own input on the consent panel.
fn consent_serial(h: &mut Harness, app: &mut TestClient<App>) -> u32 {
    app.state.control_serial = 0;
    let window = app.state.control_window.unwrap();
    let surface = h.state.entry(window).unwrap().surface().clone();
    h.state.set_keyboard_focus(Some(surface));
    for pressed in [true, false] {
        h.state
            .process_input_event(common::absolute::key(57, pressed));
    }
    h.pump_until(app, "a physical consent serial", |s| s.control_serial != 0);
    app.state.control_serial
}

/// A window of its own that takes the keyboard and records the keys it gets.
fn keyboard_target(h: &mut Harness) -> TestClient<App> {
    let (mut target, _) = mapped_app_named(h, "org.example.controlled", None);
    let qh = target.qh();
    let seat: wl_seat::WlSeat = target.globals.bind(&qh, 1..=7, ()).unwrap();
    let _keyboard = seat.get_keyboard(&qh, ());
    h.idle(&mut target, 3);
    target
}

#[test]
fn two_bridges_of_different_apps_share_the_desktop_on_one_controller_connection() {
    let Some(mut h) = Harness::new() else { return };
    let mut target = keyboard_target(&mut h);
    let (first, second) = (Bridge::spawn(), ShellBridge::spawn());
    assert_ne!(second.app, this_program());
    // the second app may control but not capture yet; no consent panel is connected at all
    let store = Store::read_by(&mut h, &this_program(), &ALLOW_ALL);
    store.change(|conf| {
        conf.set(Service::Control, &second.app, Decision::Allow)
            .unwrap();
        conf.set(Service::Capture, &second.app, Decision::Deny)
            .unwrap();
    });
    h.state.reload_desktop_control(&store.path());
    let mut runs = Runs::connect(&mut h, CONCURRENT);
    let a = runs.open(&mut h, (first.pid, first.start));
    let b = runs.open(&mut h, (second.pid, second.start));
    assert_ne!(runs.epoch(a), runs.epoch(b));
    assert_eq!(runs.start(&mut h, a, 15), Ok(()));
    runs.phase(&mut h, a, 2);
    // each run is decided by its own app's answer, beside a run already active
    assert_eq!(runs.start(&mut h, b, 15), Err(session::Refusal::Denied));
    assert_eq!(runs.seen(b).phase, 0);
    store.change(|conf| {
        conf.set(Service::Capture, &second.app, Decision::Allow)
            .unwrap()
    });
    h.state.reload_desktop_control(&store.path());
    assert_eq!(runs.start(&mut h, b, 15), Ok(()));
    runs.phase(&mut h, b, 2);
    assert_eq!(runs.seen(a).phase, 2);
    // each run captures through the sources it selected
    let (source_a, source_b) = (runs.source(a), runs.source(b));
    let capture_a = runs.client.session(&mut h, &source_a, false);
    runs.frame(&mut h, &capture_a, "a frame of the first run");
    let capture_b = runs.client.session(&mut h, &source_b, false);
    runs.frame(&mut h, &capture_b, "a frame of the second run");
    assert!(capture_b.pixels().contains(&RED));
    // from here on only the second run's capture session is open, so a stop the connection is told of is its own
    capture_a.session.destroy();
    runs.client.state.session = SessionSeen::default();
    // input interleaves like two devices: a key both runs hold is down until the last of them lets go
    for (slot, code, pressed) in [
        (a, 42, true),
        (a, 29, true),
        (b, 29, true),
        (b, 30, true),
        (b, 30, false),
    ] {
        assert_eq!(runs.key(&mut h, slot, code, pressed), Ok(()));
    }
    h.pump_until(&mut target, "the interleaved keys", |s| {
        s.control_keys.contains(&(30, false))
    });
    assert_eq!(
        target.state.control_keys,
        [(42, true), (29, true), (30, true), (30, false)]
    );
    // stopping one run releases what only it held and leaves the other running
    runs.runs[a].session.stop();
    runs.phase(&mut h, a, 4);
    h.pump_until(&mut target, "the stopped run's own release", |s| {
        s.control_keys.contains(&(42, false))
    });
    h.idle(&mut runs.client, 5);
    assert_eq!(runs.seen(b).phase, 2);
    assert!(
        !target.state.control_keys.contains(&(29, false)),
        "the other run still holds 29: {:?}",
        target.state.control_keys
    );
    assert!(h.state.control_capturing());
    assert!(
        !runs.client.state.session.stopped,
        "the other run's capture session goes on"
    );
    let after = runs.client.session(&mut h, &source_b, false);
    runs.frame(&mut h, &after, "the other run's capture after the stop");
    runs.stopped_source(&mut h, &source_a);
    assert_eq!(
        runs.key(&mut h, a, 48, true),
        Err(session::Refusal::NotGranted)
    );
    assert_eq!(runs.key(&mut h, b, 29, false), Ok(()));
    assert_eq!(runs.key(&mut h, b, 48, true), Ok(()));
    assert_eq!(runs.key(&mut h, b, 48, false), Ok(()));
    h.pump_until(&mut target, "the remaining run's keys", |s| {
        s.control_keys.contains(&(48, false))
    });
    assert_eq!(
        target.state.control_keys,
        [
            (42, true),
            (29, true),
            (30, true),
            (30, false),
            (42, false),
            (29, false),
            (48, true),
            (48, false)
        ]
    );
    // the run that ended leaves its place to the next run of its bridge
    let again = runs.open(&mut h, (first.pid, first.start));
    assert_eq!(runs.start(&mut h, again, 15), Ok(()));
    runs.phase(&mut h, again, 2);
    runs.runs[b].session.stop();
    runs.phase(&mut h, b, 4);
    assert_eq!(runs.seen(again).phase, 2);
}

#[test]
fn runs_that_must_ask_the_person_are_asked_one_at_a_time() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, authority) = authority(&mut h);
    let (first, second) = (Bridge::spawn(), ShellBridge::spawn());
    let _store = Store::read_by(&mut h, &this_program(), &[]);
    let mut runs = Runs::connect(&mut h, CONCURRENT);
    let a = runs.open(&mut h, (first.pid, first.start));
    let b = runs.open(&mut h, (second.pid, second.start));
    assert_eq!(runs.start(&mut h, a, 15), Ok(()));
    assert_eq!(runs.start(&mut h, b, 15), Ok(()));
    runs.phase(&mut h, a, 1);
    runs.phase(&mut h, b, 1);
    h.pump_until(&mut app, "the first request", |s| {
        s.control_request.is_some()
    });
    assert_eq!(app.state.control_request.unwrap().0, runs.epoch(a));
    h.idle(&mut app, 5);
    assert_eq!(
        app.state.control_request.unwrap().0,
        runs.epoch(a),
        "the second request waits for the first"
    );
    // a run not asked yet cannot be answered, even with the person's own input
    let serial = consent_serial(&mut h, &mut app);
    let (hi, lo) = (runs.runs[b].hi, runs.runs[b].lo);
    authority.answer(hi, lo, 1, serial);
    app.pump();
    h.idle(&mut runs.client, 5);
    assert_eq!(runs.seen(b).phase, 1);
    let (hi, lo) = (runs.runs[a].hi, runs.runs[a].lo);
    authority.answer(hi, lo, 1, serial);
    app.pump();
    runs.phase(&mut h, a, 2);
    h.pump_until(&mut app, "the second request after the first answer", |s| {
        s.control_request
            .is_some_and(|(epoch, _, _)| epoch != runs.epoch(a))
    });
    let (epoch, _, _) = app.state.control_request.unwrap();
    assert_eq!(epoch, runs.epoch(b));
    assert_eq!(
        app.state.control_app.as_ref().unwrap().2,
        second.app_identity(),
        "the panel names the second run's own app"
    );
    let serial = consent_serial(&mut h, &mut app);
    let (hi, lo) = (runs.runs[b].hi, runs.runs[b].lo);
    authority.answer(hi, lo, 1, serial);
    app.pump();
    runs.phase(&mut h, b, 2);
    assert_eq!(runs.seen(a).phase, 2);
}

#[test]
fn a_connection_holds_as_many_sessions_as_the_controller_serves() {
    let Some(mut h) = Harness::new() else { return };
    let bridge = Bridge::spawn();
    let mut runs = Runs::connect(&mut h, CONCURRENT);
    for _ in 0..RUNS {
        runs.open(&mut h, (bridge.pid, bridge.start));
    }
    let epochs: std::collections::BTreeSet<_> = (0..RUNS).map(|slot| runs.epoch(slot)).collect();
    assert_eq!(epochs.len(), RUNS, "every run has an epoch of its own");
    runs.manager.get_session(&runs.client.qh(), RUNS);
    protocol_error(&mut h, &mut runs.client, "cupertino_control_v1");
    // an older manager keeps one session per connection
    let mut older = Runs::connect(&mut h, 4);
    older.open(&mut h, (bridge.pid, bridge.start));
    older.manager.get_session(&older.client.qh(), 1);
    protocol_error(&mut h, &mut older.client, "cupertino_control_v1");
}

/// Step until the server closes `client` with a protocol error on `interface`, which must be the session limit.
fn protocol_error(h: &mut Harness, client: &mut TestClient<Capturer>, interface: &str) {
    let end = Instant::now() + common::HANG_GUARD;
    while client.conn.protocol_error().is_none() {
        assert!(Instant::now() < end, "the extra session was accepted");
        let _ = client.conn.flush();
        h.step();
        if let Some(guard) = client.queue.prepare_read() {
            let _ = guard.read();
        }
        let _ = client.queue.dispatch_pending(&mut client.state);
    }
    let error = client.conn.protocol_error().unwrap();
    assert_eq!(error.object_interface, interface);
    assert_eq!(
        error.code,
        cupertino_protocols::client::control::cupertino_control_v1::Error::SessionExists as u32
    );
}

impl ShellBridge {
    /// How the consent panel names the app: its program's path.
    fn app_identity(&self) -> String {
        match &self.app {
            remembered::App::Executable(path) => path.to_str().unwrap().to_owned(),
            remembered::App::Desktop(id) => id.clone(),
        }
    }
}
