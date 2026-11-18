//! Session announcements and switches over LoginManager1, with a stand-in for logind and the
//! manager loop. The test neither starts a greeter nor opens a PAM session or a console.
#![cfg(feature = "app")]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use cupertino_sessiond::caller::CallerSession;
use cupertino_sessiond::service::{self, Authorizer, Call, MachineFiles, ReadyRefusal};
use zbus::blocking::{connection::Builder, Proxy};

struct PrivateBus(Child);
impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Logind(Arc<Mutex<CallerSession>>);
impl Authorizer for Logind {
    fn session_of(&self, pid: u32) -> Result<Option<CallerSession>, String> {
        assert_eq!(pid, std::process::id());
        Ok(Some(self.0.lock().unwrap().clone()))
    }
    fn is_administrator(&self, _: u32) -> Result<bool, String> {
        Ok(false)
    }
}

#[test]
fn the_background_agent_can_announce_its_own_session_but_only_the_foreground_can_switch() {
    let mut bus = PrivateBus(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim();
    let client = Builder::address(address)
        .unwrap()
        .method_timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let id = client.unique_name().unwrap().as_str().to_owned();
    let session = Arc::new(Mutex::new(CallerSession {
        id: "c7".into(),
        uid: rustix::process::getuid().as_raw(),
        class: "user".into(),
        seat: "seat0".into(),
        active: false,
        remote: false,
        state: "online".into(),
    }));
    let (send, calls) = mpsc::channel();
    let _manager = service::serve(
        address.parse().unwrap(),
        Arc::new(Logind(session.clone())),
        Arc::new(move |call| {
            let (method, session) = match call {
                Call::Prepared { session, reply } => {
                    reply.send(Ok(())).unwrap();
                    ("SessionPrepared", session)
                }
                Call::Ready {
                    session,
                    agent,
                    reply,
                } => {
                    assert_eq!(agent, id);
                    reply.send(Ok(())).unwrap();
                    ("SessionReady", session)
                }
                Call::SwitchToLoginWindow { session, reply } => {
                    reply.send(Ok(())).unwrap();
                    ("SwitchToLoginWindow", session)
                }
                Call::LogOut { session, reply } => {
                    reply.send(Err(ReadyRefusal::UnknownSession)).unwrap();
                    ("LogOut", session)
                }
                other => panic!("unexpected call: {other:?}"),
            };
            send.send((method, session.id)).is_ok()
        }),
        MachineFiles {
            dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")),
            settings_program: Arc::new(|_, _| false),
        },
    )
    .unwrap();
    let proxy = Proxy::new(
        &client,
        service::BUS_NAME,
        service::OBJECT_PATH,
        service::BUS_NAME,
    )
    .unwrap();
    for (active, method, expected, forwarded) in [
        (false, "SessionPrepared", None, true),
        (false, "SessionReady", None, true),
        (
            false,
            "SwitchToLoginWindow",
            Some("org.cupertino.LoginManager1.Error.AccessDenied"),
            false,
        ),
        (true, "SwitchToLoginWindow", None, true),
        (
            true,
            "LogOut",
            Some("org.cupertino.LoginManager1.Error.UnknownSession"),
            true,
        ),
    ] {
        let mut current = session.lock().unwrap();
        current.active = active;
        current.state = if active { "active" } else { "online" }.into();
        drop(current);
        let result = proxy.call::<_, _, ()>(method, &());
        if let Some(expected) = expected {
            assert!(
                matches!(result, Err(zbus::Error::MethodError(ref name,_,_)) if name.as_str() == expected),
                "{method}: {result:?}"
            );
        } else {
            result.unwrap();
        }
        if forwarded {
            assert_eq!(
                calls.recv_timeout(Duration::from_secs(1)).unwrap(),
                (method, "c7".into())
            );
        } else {
            assert!(matches!(calls.try_recv(), Err(mpsc::TryRecvError::Empty)));
        }
    }
}
