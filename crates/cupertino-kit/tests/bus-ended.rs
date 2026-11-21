//! `kit::bus` at an instance's end: a connection that follows signals, with a stream the model keeps and a task on the
//! connection's executor that never ends by itself, is closed and released when the runtime that made it ends, so the
//! bus daemon sees it go and the process keeps nothing of it.

use crate::support;

use cupertino_kit::bus::{self, Bus};
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::Instance;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::process::{Command as Process, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use zbus::export::futures_core::Stream;
use zbus::{MatchRule, MessageStream};

/// The private bus's socket, for the instances that connect on a socket of their own.
static SOCKET: OnceLock<std::path::PathBuf> = OnceLock::new();

/// A stream the model keeps, shared so the message that carries it can be cloned.
#[derive(Clone)]
struct Kept(#[allow(dead_code)] Arc<Mutex<Option<MessageStream>>>);

#[derive(Clone)]
enum Message {
    Connected(Result<zbus::Connection, String>),
    Subscribed(Result<(String, Kept), String>),
    Closed,
}

#[derive(Default)]
struct Model {
    connection: Option<zbus::Connection>,
    /// The connection's unique name once it follows its signals.
    name: Option<Result<String, String>>,
    kept: Option<Kept>,
}

/// The signals the instance follows: every name's owner changing.
fn owner_changes() -> MatchRule<'static> {
    MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .unwrap()
        .member("NameOwnerChanged")
        .unwrap()
        .build()
}

/// Follows `stream` until it ends.
async fn follow(mut stream: MessageStream) {
    while std::future::poll_fn(|cx| std::pin::Pin::new(&mut stream).poll_next(cx))
        .await
        .is_some()
    {}
}

/// An instance that connects, on the private bus's socket when `ON_SOCKET`, and follows a signal.
struct Follower<const ON_SOCKET: bool>;

impl<const ON_SOCKET: bool> Application for Follower<ON_SOCKET> {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (Model, Vec<Command<Message>>) {
        let connected =
            |c: zbus::Result<zbus::Connection>| Message::Connected(c.map_err(|e| e.to_string()));
        let commands = if ON_SOCKET {
            let stream = UnixStream::connect(SOCKET.get().unwrap()).unwrap();
            bus::connect_on(
                stream,
                move |c| async move { connected(c) },
                Message::Closed,
            )
        } else {
            bus::connect(Bus::Session, connected, Message::Closed)
        };
        (Model::default(), commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Connected(Ok(connection)) => {
                model.connection = Some(connection.clone());
                let subscribe = async move {
                    let subscribed = async {
                        let kept =
                            MessageStream::for_match_rule(owner_changes(), &connection, None)
                                .await?;
                        let followed =
                            MessageStream::for_match_rule(owner_changes(), &connection, Some(8))
                                .await?;
                        // a task on the connection's executor that keeps the connection and ends only with the stream
                        connection
                            .executor()
                            .spawn(follow(followed), "follow owner changes")
                            .detach();
                        let name = connection.unique_name().unwrap().to_string();
                        Ok::<_, zbus::Error>((name, Kept(Arc::new(Mutex::new(Some(kept))))))
                    };
                    Message::Subscribed(subscribed.await.map_err(|e| e.to_string()))
                };
                return vec![Command::Perform(Box::pin(subscribe))];
            }
            Message::Connected(Err(e)) => model.name = Some(Err(e)),
            Message::Subscribed(Ok((name, kept))) => {
                model.name = Some(Ok(name));
                model.kept = Some(kept);
            }
            Message::Subscribed(Err(e)) => model.name = Some(Err(e)),
            Message::Closed => {}
        }
        Vec::new()
    }

    fn view(_: &Model) -> Vec<Window<Message>> {
        Vec::new()
    }
}

/// Runs `A` in an instance of its own until it follows its signal, ends it, and returns the connection's unique name.
fn once<A: Application<Model = Model>>(text: &Arc<cupertino_kit::text::TextSystem>) -> String {
    let instance = Instance::new("bus-ended");
    let _entered = instance.enter();
    let mut driver = support::driver::<A>(Arc::clone(text));
    support::run_until(&mut driver, |m| m.name.is_some());
    let name = driver.runtime().model().name.clone().unwrap().unwrap();
    drop(driver);
    name
}

/// The process's anonymous memory in KiB.
fn anonymous_kib() -> u64 {
    let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap();
    rollup
        .lines()
        .find_map(|line| line.strip_prefix("Anonymous:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

/// Names whose owner left the bus at `address`, as a connection of its own hears them, until the bus goes; the first
/// value sent says it follows them.
fn names_lost(address: String) -> (mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
    let (sent, lost) = mpsc::channel();
    let observer = std::thread::spawn(move || {
        zbus::block_on(async move {
            let connection = zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .build()
                .await
                .unwrap();
            let mut stream = MessageStream::for_match_rule(owner_changes(), &connection, None)
                .await
                .unwrap();
            sent.send(String::new()).unwrap();
            while let Some(Ok(message)) =
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut stream).poll_next(cx)).await
            {
                let Ok((name, _old, new)) =
                    message.body().deserialize::<(String, String, String)>()
                else {
                    continue;
                };
                if name.starts_with(':') && new.is_empty() && sent.send(name).is_err() {
                    return;
                }
            }
        });
    });
    (lost, observer)
}

/// The private bus daemon, stopped however the test ends.
struct Daemon(std::process::Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// How many instances end in the measured run.
const RUNS: usize = 50;
/// The ones that run before it, so what the process sets up once (pools, caches, the allocator's arenas) is there.
const WARM_UP: usize = 6;
/// How much the anonymous memory may grow over the measured run: well under the dozen KiB each ended connection kept
/// before, times [`RUNS`].
const GROWTH_KIB: u64 = 128;

#[test]
fn an_ended_instance_closes_its_connection_and_keeps_nothing_of_it() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("kit-bus-ended");
    std::fs::create_dir_all(&dir).unwrap();
    let mut daemon = Daemon(
        Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:dir={}", dir.display()))
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs"),
    );
    let mut address = String::new();
    BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim().to_owned();
    let path = address
        .strip_prefix("unix:path=")
        .and_then(|rest| rest.split(',').next())
        .expect("a socket path");
    SOCKET.set(path.into()).unwrap();
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
    let (lost, observer) = names_lost(address);
    assert_eq!(lost.recv_timeout(support::PATIENCE).unwrap(), "");

    let text = support::text_system();
    let mut run = |i: usize| {
        let name = if i.is_multiple_of(2) {
            once::<Follower<false>>(&text)
        } else {
            once::<Follower<true>>(&text)
        };
        // the daemon hears the connection go: nothing of the instance keeps it open
        loop {
            let gone = lost
                .recv_timeout(support::PATIENCE)
                .unwrap_or_else(|_| panic!("run {i}: {name} still on the bus"));
            if gone == name {
                break;
            }
        }
    };
    (0..WARM_UP).for_each(&mut run);
    let before = anonymous_kib();
    (WARM_UP..WARM_UP + RUNS).for_each(&mut run);
    let after = anonymous_kib();
    assert!(
        after <= before + GROWTH_KIB,
        "anonymous memory grew from {before} KiB to {after} KiB over {RUNS} ended instances"
    );

    drop(daemon);
    observer.join().unwrap();
}
