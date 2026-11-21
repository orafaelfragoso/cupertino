//! `kit::bus`: a connection does its work on the UI thread, polled by the runtime, with no executor thread of its own; a
//! call's reply comes back as a message, and the application hears when the connection closes.

use crate::support;

use cupertino_kit::bus::{self, Bus};
use cupertino_kit::core::{Application, Command, Window};
use std::io::{BufRead, BufReader};
use std::process::{Command as Process, Stdio};

#[derive(Clone, Debug)]
enum Message {
    Connected(Result<zbus::Connection, String>),
    Id(Result<String, String>),
    Closed,
}

#[derive(Default)]
struct Model {
    connection: Option<zbus::Connection>,
    id: Option<Result<String, String>>,
    /// The names of the process's threads when the reply came.
    threads: Vec<String>,
    closed: bool,
}

/// The names of this process's threads, as the kernel keeps them.
fn threads() -> Vec<String> {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
        .map(|name| name.trim_end().to_owned())
        .collect()
}

struct Caller;

impl Application for Caller {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (Model, Vec<Command<Message>>) {
        let connected =
            |c: zbus::Result<zbus::Connection>| Message::Connected(c.map_err(|e| e.to_string()));
        (
            Model::default(),
            bus::connect(Bus::Session, connected, Message::Closed),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Connected(Ok(connection)) => {
                model.connection = Some(connection.clone());
                let call = async move {
                    let reply = connection
                        .call_method(
                            Some("org.freedesktop.DBus"),
                            "/org/freedesktop/DBus",
                            Some("org.freedesktop.DBus"),
                            "GetId",
                            &(),
                        )
                        .await;
                    Message::Id(
                        reply
                            .and_then(|m| m.body().deserialize::<String>())
                            .map_err(|e| e.to_string()),
                    )
                };
                return vec![Command::Perform(Box::pin(call))];
            }
            Message::Connected(Err(e)) => model.id = Some(Err(e)),
            Message::Id(id) => {
                model.id = Some(id);
                model.threads = threads();
                // the last clone goes: the connection closes
                model.connection = None;
            }
            Message::Closed => model.closed = true,
        }
        Vec::new()
    }

    fn view(_: &Model) -> Vec<Window<Message>> {
        Vec::new()
    }
}

#[test]
fn a_call_is_answered_on_the_ui_thread_with_no_bus_thread_and_the_close_is_heard() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("kit-bus");
    std::fs::create_dir_all(&dir).unwrap();
    let mut daemon = Process::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .arg(format!("--address=unix:dir={}", dir.display()))
        .stdout(Stdio::piped())
        .spawn()
        .expect("dbus-daemon runs");
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", address.trim());
    let mut driver = support::driver::<Caller>(support::text_system());
    support::run_until(&mut driver, |m| m.id.is_some());
    let model = driver.runtime().model();
    let id = model.id.clone().unwrap().unwrap();
    assert!(
        id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()),
        "{id}"
    );
    assert!(
        !model.threads.iter().any(|t| t.starts_with("zbus")),
        "{:?}",
        model.threads
    );
    support::run_until(&mut driver, |m| m.closed);
    daemon.kill().unwrap();
    daemon.wait().unwrap();
}
