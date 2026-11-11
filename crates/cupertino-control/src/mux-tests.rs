//! The loop against a compositor of the test's own: each run's requests are answered as a compositor that remembers
//! an allow for every app would, and every session request is logged.
use super::*;
use cupertino_wayland_client::{OutputTransform, Scale};
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::sync::{Arc, Mutex};

/// The name of the one output.
const OUTPUT: &str = "test-1";
/// Long enough for any response the loop gives at once, also on a loaded machine.
const PROMPT: Duration = Duration::from_secs(5);

/// What the test compositor saw, by run epoch.
#[derive(Debug, Default)]
struct Log {
    /// Requests in arrival order: epoch and what was asked.
    requests: Vec<(u64, String)>,
    stopped: Vec<u64>,
    closed: Vec<u64>,
    identified: Vec<(u64, Option<(u32, u64)>)>,
}

struct Fake {
    capacity: usize,
    next_epoch: u64,
    phases: Vec<(u64, u32)>,
    events: Vec<ControlEvent>,
    woken: Receiver<()>,
    log: Arc<Mutex<Log>>,
}
impl Fake {
    fn phase(&mut self, epoch: u64, phase: u32) {
        if let Some(entry) = self.phases.iter_mut().find(|(e, _)| *e == epoch) {
            entry.1 = phase;
        }
        self.events.push(ControlEvent::Status {
            epoch,
            serial: 0,
            phase,
        });
    }
}
impl Desktop for Fake {
    type Session = u64;
    type Frame = ();
    fn capacity(&self) -> usize {
        self.capacity
    }
    fn open(&mut self) -> u64 {
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        self.phases.push((epoch, 0));
        self.events.push(ControlEvent::Created {
            epoch,
            available: 31,
        });
        self.phase(epoch, 0);
        epoch
    }
    fn epoch(&self, session: &u64) -> u64 {
        *session
    }
    fn identify(&mut self, session: &u64, peer: Option<(u32, u64)>) {
        self.log.lock().unwrap().identified.push((*session, peer));
    }
    fn send(&mut self, session: &u64, serial: u32, command: Command) -> Result<(), String> {
        let epoch = *session;
        let phase = self.phases.iter().find(|(e, _)| *e == epoch).unwrap().1;
        let what = match command {
            Command::Start { .. } => "start".to_owned(),
            Command::Key { code, pressed, .. } => format!("key {code} {pressed}"),
            Command::Button { code, pressed, .. } => format!("button {code} {pressed}"),
            Command::Motion { .. } => "motion".to_owned(),
            Command::Axis { .. } => "axis".to_owned(),
            Command::Observe => "observe".to_owned(),
            Command::Perform(_) => "perform".to_owned(),
            Command::Resume => "resume".to_owned(),
        };
        self.log
            .lock()
            .unwrap()
            .requests
            .push((epoch, what.clone()));
        if what == "start" {
            // the app is allowed: the run is active at once
            self.phase(epoch, 2);
            self.events.push(ControlEvent::Geometry {
                epoch,
                revision: 1,
                x: 0,
                y: 0,
                width: 800,
                height: 600,
                scale: 1.0,
                name: OUTPUT.into(),
            });
        } else if phase != 2 {
            self.events.push(ControlEvent::Refused {
                epoch,
                serial,
                reason: 1,
            });
            return Ok(());
        }
        self.events.push(ControlEvent::Completed { epoch, serial });
        Ok(())
    }
    fn stop(&mut self, session: &u64) {
        self.log.lock().unwrap().stopped.push(*session);
        self.phase(*session, 4);
    }
    fn close(&mut self, session: u64) {
        self.log.lock().unwrap().closed.push(session);
    }
    fn outputs(&self) -> Vec<OutputInfo> {
        vec![OutputInfo {
            id: OutputId::from_registry_name(1),
            name: Some(OUTPUT.into()),
            description: None,
            make: None,
            model: None,
            physical_size_mm: None,
            transform: OutputTransform::default(),
            scale: Scale::ONE,
            mode: Some((800, 600)),
            refresh_mhz: None,
            geometry: None,
        }]
    }
    fn capture(&mut self, _: &u64, _: OutputId) -> Result<(), String> {
        Err("this compositor takes no images".into())
    }
    fn progress(&self, _: &()) -> (bool, bool, bool) {
        (false, false, true)
    }
    fn prepare(&mut self, _: &mut ()) -> Result<(), String> {
        Err("this compositor takes no images".into())
    }
    fn pixels(&mut self, _: ()) -> Result<Pixels, String> {
        Err("this compositor takes no images".into())
    }
    fn wait(&mut self, timeout: Option<Duration>) -> Result<Vec<ControlEvent>, String> {
        if self.events.is_empty() {
            let _ = match timeout {
                Some(timeout) => self.woken.recv_timeout(timeout).ok(),
                None => self.woken.recv().ok(),
            };
        }
        while self.woken.try_recv().is_ok() {}
        Ok(std::mem::take(&mut self.events))
    }
}

/// The loop on a thread of its own, with what its compositor saw and a way to connect peers.
struct Controller {
    connect: Option<SyncSender<io::Result<UnixStream>>>,
    wake: Wake,
    log: Arc<Mutex<Log>>,
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
}
impl Controller {
    fn start(capacity: usize) -> Self {
        let (woke, woken) = mpsc::channel();
        let woke: Mutex<Sender<()>> = Mutex::new(woke);
        let wake: Wake = Arc::new(move || {
            let _ = woke.lock().unwrap().send(());
        });
        let log = Arc::new(Mutex::new(Log::default()));
        let (connect, incoming) = mpsc::sync_channel(MAX_RUNS_PER_CONTROLLER + 1);
        let mut fake = Fake {
            capacity,
            next_epoch: 1,
            phases: Vec::new(),
            events: Vec::new(),
            woken,
            log: log.clone(),
        };
        let loop_wake = wake.clone();
        let thread = std::thread::spawn(move || run(&mut fake, &incoming, &loop_wake));
        Self {
            connect: Some(connect),
            wake,
            log,
            thread: Some(thread),
        }
    }
    /// A peer that has not sent anything yet.
    fn connect(&self) -> Client {
        let (ours, theirs) = UnixStream::pair().unwrap();
        self.connect.as_ref().unwrap().send(Ok(theirs)).unwrap();
        (self.wake)();
        ours.set_read_timeout(Some(Duration::from_secs(40)))
            .unwrap();
        Client {
            reader: BufReader::new(ours.try_clone().unwrap()),
            stream: ours,
        }
    }
    /// A peer past the handshake.
    fn greeted(&self) -> Client {
        let mut client = self.connect();
        assert_eq!(
            client.ok(json!({"op":"hello","version":1}))["schema"],
            "cupertino.control.v1"
        );
        client
    }
    fn wait_for(&self, what: &str, done: impl Fn(&Log) -> bool) {
        let deadline = Instant::now() + PROMPT;
        while !done(&self.log.lock().unwrap()) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        drop(self.connect.take());
        (self.wake)();
        let ended = self.thread.take().unwrap().join().unwrap();
        if !std::thread::panicking() {
            assert_eq!(ended, Err("local listener ended".into()));
        }
    }
}

struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}
impl Client {
    fn send(&mut self, command: Value) {
        let mut line = serde_json::to_vec(&command).unwrap();
        line.push(b'\n');
        self.stream.write_all(&line).unwrap();
    }
    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("bad response {line:?}"))
    }
    fn call(&mut self, command: Value) -> Value {
        self.send(command);
        self.read()
    }
    #[track_caller]
    fn ok(&mut self, command: Value) -> Value {
        let response = self.call(command.clone());
        assert_eq!(response["ok"], true, "{command}: {response}");
        response["data"].clone()
    }
    fn start(&mut self) -> u64 {
        let state = self.ok(json!({"op":"request","output":OUTPUT,"capabilities":15}));
        assert_eq!(state["phase"], 2);
        state["epoch"].as_u64().unwrap()
    }
    fn key(&mut self, code: u32, pressed: bool) {
        self.ok(json!({"op":"key","code":code,"pressed":pressed}));
    }
}

#[test]
fn two_peers_interleave_requests_each_on_a_run_of_its_own() {
    let controller = Controller::start(MAX_RUNS_PER_CONTROLLER);
    let mut a = controller.greeted();
    let mut b = controller.greeted();
    let a_epoch = a.start();
    let before = b.ok(json!({"op":"status"}));
    assert_eq!(before["phase"], 0, "the other peer's run did not start");
    let b_epoch = b.start();
    assert_ne!(a_epoch, b_epoch);
    a.key(42, true);
    b.key(30, true);
    b.key(30, false);
    a.key(42, false);
    assert_eq!(a.ok(json!({"op":"status"}))["epoch"], a_epoch);
    assert_eq!(b.ok(json!({"op":"status"}))["epoch"], b_epoch);
    let log = controller.log.lock().unwrap();
    assert_eq!(
        log.requests,
        [
            (a_epoch, "start".to_owned()),
            (b_epoch, "start".to_owned()),
            (a_epoch, "key 42 true".to_owned()),
            (b_epoch, "key 30 true".to_owned()),
            (b_epoch, "key 30 false".to_owned()),
            (a_epoch, "key 42 false".to_owned()),
        ]
    );
    // each run names the peer it serves, here this test's own process
    let me = std::process::id();
    assert_eq!(log.identified.len(), 2);
    assert!(log
        .identified
        .iter()
        .all(|(_, peer)| peer.is_some_and(|(pid, _)| pid == me)));
}

#[test]
fn a_long_wait_of_one_peer_does_not_delay_another() {
    let controller = Controller::start(MAX_RUNS_PER_CONTROLLER);
    let mut waiting = controller.greeted();
    let mut other = controller.greeted();
    waiting.start();
    let asked = Instant::now();
    // a phase the run never reaches: the wait holds until its deadline
    waiting.send(json!({"op":"wait","phase":3,"timeout_ms":1500}));
    let status = other.ok(json!({"op":"status"}));
    assert_eq!(status["phase"], 0);
    other.start();
    other.key(30, true);
    other.key(30, false);
    assert!(
        asked.elapsed() < Duration::from_millis(1500),
        "the other peer was served while the wait held"
    );
    let timed_out = waiting.read();
    assert!(asked.elapsed() >= Duration::from_millis(1500));
    assert_eq!(timed_out["ok"], false);
    assert_eq!(timed_out["error"], "operation timed out");
    // the waiting peer goes on afterwards
    assert_eq!(waiting.ok(json!({"op":"status"}))["phase"], 2);
}

#[test]
fn a_peer_beyond_the_bound_is_turned_away_at_once() {
    let controller = Controller::start(MAX_RUNS_PER_CONTROLLER);
    let mut peers: Vec<_> = (0..MAX_RUNS_PER_CONTROLLER)
        .map(|_| controller.greeted())
        .collect();
    let mut extra = controller.connect();
    let refused = extra.read();
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], turned_away(MAX_RUNS_PER_CONTROLLER));
    // the connection ends with the error
    let mut rest = String::new();
    assert_eq!(extra.reader.read_line(&mut rest).unwrap(), 0);
    // the peers already served go on, and a place freed is taken by the next peer
    for peer in &mut peers {
        assert_eq!(peer.ok(json!({"op":"status"}))["phase"], 0);
    }
    drop(peers.pop());
    controller.wait_for("the departed peer's run to close", |log| {
        log.closed.len() == 1
    });
    let mut next = controller.greeted();
    assert_eq!(next.ok(json!({"op":"status"}))["phase"], 0);
}

#[test]
fn a_compositor_without_concurrent_runs_serves_one_peer_and_turns_away_the_next() {
    let controller = Controller::start(1);
    let mut first = controller.greeted();
    let mut second = controller.connect();
    assert_eq!(second.read()["error"], turned_away(1));
    first.start();
}

#[test]
fn a_disconnect_ends_only_the_run_of_that_peer() {
    let controller = Controller::start(MAX_RUNS_PER_CONTROLLER);
    let mut leaving = controller.greeted();
    let mut staying = controller.greeted();
    let gone = leaving.start();
    let kept = staying.start();
    leaving.key(42, true);
    staying.key(30, true);
    // gone with a wait outstanding: the disconnect, not its deadline, ends the run
    leaving.send(json!({"op":"wait","phase":3,"timeout_ms":30000}));
    let left = Instant::now();
    drop(leaving);
    controller.wait_for("the departed peer's run to end", |log| log.closed == [gone]);
    assert!(left.elapsed() < Duration::from_secs(5));
    assert_eq!(controller.log.lock().unwrap().stopped, [gone]);
    let state = staying.ok(json!({"op":"status"}));
    assert_eq!(state["epoch"], kept);
    assert_eq!(state["phase"], 2);
    staying.key(30, false);
    staying.ok(json!({"op":"stop"}));
    assert_eq!(controller.log.lock().unwrap().stopped, [gone, kept]);
}

#[test]
fn a_peer_must_greet_first_and_may_send_only_typed_commands() {
    let controller = Controller::start(MAX_RUNS_PER_CONTROLLER);
    let mut peer = controller.connect();
    let early = peer.call(json!({"op":"status"}));
    assert_eq!(early["error"], "local protocol handshake required");
    peer.ok(json!({"op":"hello","version":1}));
    assert_eq!(
        peer.call(json!({"op":"exec","command":"x"}))["error"],
        "invalid local command"
    );
    assert_eq!(
        peer.call(json!({"op":"wait","phase":2,"timeout_ms":MAX_WAIT_MS + 1}))["error"],
        "state wait exceeds deadline limit"
    );
    assert_eq!(
        peer.call(json!({"op":"capture"}))["error"],
        "desktop control is not active"
    );
}
