//! The real Smithay session against a private bus peer. The peer can act on a call and withhold
//! its reply; it never opens a seat or changes virtual terminals.

use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind};
use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use cupertino_seat::{Error, LogindSession};
use rustix::fs::OFlags;
use smithay::backend::session::{Event, Session};
use smithay::reexports::calloop::EventLoop;
use zbus::blocking::{connection::Builder, Connection, MessageIterator};
use zbus::message::Type;
use zbus::zvariant::{Fd, ObjectPath, Value};
use zbus::{MatchRule, Message};

const SESSION: &str = "/org/freedesktop/login1/session/test";
const SEAT: &str = "/org/freedesktop/login1/seat/seat0";
/// Outer watchdog for a missing call or a stuck compositor; longer than its actual bus deadline.
const WATCHDOG: Duration = Duration::from_secs(6);

struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Peer {
    conn: Connection,
    incoming: Receiver<Message>,
}

impl Peer {
    fn next(&self, member: &str) -> Message {
        let call = self.incoming.recv_timeout(WATCHDOG).expect(member);
        assert_eq!(call.header().member().unwrap().as_str(), member);
        call
    }

    fn ok(&self, member: &str) {
        self.conn.reply(&self.next(member).header(), &()).unwrap();
    }

    fn device(&self, member: &str) -> Message {
        let call = self.next(member);
        assert_eq!(call.body().deserialize::<(u32, u32)>().unwrap(), (1, 3));
        call
    }

    fn take(&self, call: &Message, path: &str) {
        let fd: OwnedFd = File::open(path).unwrap().into();
        self.conn
            .reply(&call.header(), &(Fd::from(&fd), false))
            .unwrap();
    }

    fn not_taken(&self, call: Message) {
        self.conn
            .reply_error(
                &call.header(),
                "org.freedesktop.login1.DeviceNotTaken",
                &("already released",),
            )
            .unwrap();
    }

    fn accept_controller(&self) {
        self.ok("TakeControl");
        for name in ["Seat", "Active"] {
            let call = self.next("Get");
            assert_eq!(
                call.body().deserialize::<(&str, &str)>().unwrap(),
                ("org.freedesktop.login1.Session", name)
            );
            let value = if name == "Seat" {
                Value::from(("seat0", ObjectPath::try_from(SEAT).unwrap()))
            } else {
                Value::from(true)
            };
            self.conn.reply(&call.header(), &value).unwrap();
        }
    }

    fn locate(&self) {
        self.conn
            .reply(
                &self.next("GetSession").header(),
                &ObjectPath::try_from(SESSION).unwrap(),
            )
            .unwrap();
    }
}

fn timed_out<T: std::fmt::Debug>(result: Result<T, Error>, method: &str) {
    let error = result.unwrap_err();
    assert!(
        matches!(&error, Error::Bus { what, source } if *what == method
        && matches!(&**source, zbus::Error::InputOutput(e) if e.kind() == ErrorKind::TimedOut)),
        "{method}: {error}"
    );
}

#[test]
fn unanswered_calls_leave_no_orphan_device_and_pause_is_acknowledged_only_after_the_callback() {
    // This test binary has one test; only its new controller uses this bus as the system bus.
    let mut bus = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap());
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", address.trim());
    std::env::set_var("XDG_SESSION_ID", "test");
    let conn = Builder::address(address.trim()).unwrap().build().unwrap();
    let messages = MessageIterator::from(&conn);
    conn.request_name("org.freedesktop.login1").unwrap();
    let (left, departures) = mpsc::channel();
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.DBus")
        .unwrap()
        .interface("org.freedesktop.DBus")
        .unwrap()
        .member("NameOwnerChanged")
        .unwrap()
        .build();
    let changes = MessageIterator::for_match_rule(rule, &conn, None).unwrap();
    thread::spawn(move || {
        for change in changes {
            let Ok(change) = change else { break };
            let (name, _, new): (String, String, String) = change.body().deserialize().unwrap();
            if new.is_empty() && left.send(name).is_err() {
                break;
            }
        }
    });
    let (incoming, calls) = mpsc::channel();
    thread::spawn(move || {
        for message in messages {
            let Ok(call) = message else { break };
            if call.message_type() == Type::MethodCall && incoming.send(call).is_err() {
                break;
            }
        }
    });
    let peer = Peer {
        conn,
        incoming: calls,
    };
    // A lost TakeControl is released by disconnecting the private controller, even though
    // no Logind handle (whose Drop would send ReleaseControl) was ever returned.
    let (failed, failure) = mpsc::channel();
    thread::spawn(move || {
        failed.send(LogindSession::new().map(|_| ())).unwrap();
    });
    peer.locate();
    let control = peer.next("TakeControl");
    timed_out(failure.recv_timeout(WATCHDOG).unwrap(), "TakeControl");
    assert_eq!(
        departures.recv_timeout(WATCHDOG).unwrap(),
        control.header().sender().unwrap().as_str()
    );
    // A rendezvous gates the peer's late replies on what the *caller* actually observed.
    let (observed, event) = mpsc::sync_channel(0);
    let (allow_retry, retry) = mpsc::channel();
    let client = thread::spawn(move || {
        let (mut session, notifier) = LogindSession::new().unwrap();
        let null = Path::new("/dev/null");
        let open = |s: &mut LogindSession| s.open(null, OFlags::RDWR);
        timed_out(open(&mut session), "TakeDevice");
        observed.send("open timed out").unwrap();
        retry.recv_timeout(WATCHDOG).unwrap();
        let fd = open(&mut session).unwrap();
        assert_eq!(
            rustix::fs::fstat(&fd).unwrap().st_rdev,
            rustix::fs::stat(null).unwrap().st_rdev
        );
        assert!(matches!(open(&mut session), Err(Error::AlreadyTaken(_))));
        let mut loop_ = EventLoop::try_new().unwrap();
        let callback = observed.clone();
        loop_
            .handle()
            .insert_source(notifier, move |event, _, _: &mut ()| {
                callback
                    .send(match event {
                        Event::PauseSession => "pause callback",
                        Event::ActivateSession => "resume callback",
                    })
                    .unwrap();
            })
            .unwrap();
        loop_.dispatch(WATCHDOG, &mut ()).unwrap();
        assert!(!session.is_active());
        observed.send("still paused").unwrap();
        loop_.dispatch(WATCHDOG, &mut ()).unwrap();
        assert!(session.is_active());
        timed_out(session.close(fd), "ReleaseDevice");
        observed.send("close timed out").unwrap();
        let _fd = open(&mut session).unwrap();
        timed_out(session.change_vt(7), "SwitchTo");
        observed.send("switch timed out").unwrap();
        drop(loop_);
        assert!(matches!(open(&mut session), Err(Error::SessionLost)));
        observed.send("notifier dropped").unwrap();
    });
    peer.locate();
    peer.accept_controller();
    let abandoned = peer.device("TakeDevice");
    assert_eq!(event.recv_timeout(WATCHDOG).unwrap(), "open timed out");
    // logind takes the device but withholds its reply. Neither a late descriptor nor a subsequent
    // open is needed to trigger the compensating release.
    let release = peer.device("ReleaseDevice");
    peer.conn
        .reply_error(
            &release.header(),
            "org.freedesktop.DBus.Error.Failed",
            &("busy",),
        )
        .unwrap();
    allow_retry.send(()).unwrap();
    peer.conn
        .reply(&peer.device("ReleaseDevice").header(), &())
        .unwrap();
    let take = peer.device("TakeDevice");
    // A very late descriptor on the original serial cannot satisfy the second open.
    peer.take(&abandoned, "/dev/zero");
    // Ordinary delayed success still hands out the usable descriptor.
    thread::sleep(Duration::from_millis(100));
    peer.take(&take, "/dev/null");
    peer.conn
        .emit_signal(
            take.header().sender().cloned(),
            SESSION,
            "org.freedesktop.login1.Session",
            "PauseDevice",
            &(1u32, 3u32, "pause"),
        )
        .unwrap();
    assert_eq!(event.recv_timeout(WATCHDOG).unwrap(), "pause callback");
    let _unanswered = peer.device("PauseDeviceComplete");
    assert_eq!(event.recv_timeout(WATCHDOG).unwrap(), "still paused");
    let fd = File::open("/dev/null").unwrap();
    peer.conn
        .emit_signal(
            take.header().sender().cloned(),
            SESSION,
            "org.freedesktop.login1.Session",
            "ResumeDevice",
            &(1u32, 3u32, Fd::from(&fd)),
        )
        .unwrap();
    assert_eq!(event.recv_timeout(WATCHDOG).unwrap(), "resume callback");
    let close = peer.device("ReleaseDevice");
    assert_eq!(event.recv_timeout(WATCHDOG).unwrap(), "close timed out");
    peer.conn.reply(&close.header(), &()).unwrap(); // acted already, next release finds nothing
    peer.not_taken(peer.device("ReleaseDevice"));
    peer.take(&peer.device("TakeDevice"), "/dev/null");
    let switch = peer.next("SwitchTo");
    assert_eq!(switch.body().deserialize::<u32>().unwrap(), 7);
    assert_eq!(event.recv_timeout(WATCHDOG).unwrap(), "switch timed out");
    let _control = peer.next("ReleaseControl"); // never answer, even during teardown
    assert_eq!(event.recv_timeout(WATCHDOG).unwrap(), "notifier dropped");
    client.join().unwrap();
    assert_eq!(
        departures.recv_timeout(WATCHDOG).unwrap(),
        take.header().sender().unwrap().as_str()
    );
}
