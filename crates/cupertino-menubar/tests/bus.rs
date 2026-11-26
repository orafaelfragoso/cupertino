//! A private session bus for the tests that talk to the worker over D-Bus, and waits bounded in time.

use std::future::Future;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use async_io::Timer;
use futures_util::future::{select, Either};
use futures_util::StreamExt;
use zbus::connection::Builder;
use zbus::message::Type;
use zbus::{Connection, MatchRule, Message, MessageStream};

/// The longest a test waits for anything the bus brings.
pub const ARRIVAL: Duration = Duration::from_secs(10);
/// How long a test waits to be sure something does not come.
pub const QUIET: Duration = Duration::from_millis(300);

/// A `dbus-daemon` of the test's own; dropped, it ends.
pub struct Bus {
    process: Child,
    pub address: String,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Bus {
    // a test owns its thread: it looks up the daemon's binary
    #[allow(clippy::disallowed_methods)]
    pub fn start() -> Bus {
        let daemon = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("dbus-daemon"))
            .find(|p| p.is_file())
            .expect("dbus-daemon is installed");
        let mut process = Command::new(daemon)
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Bus {
            process,
            address: address.trim().to_owned(),
        }
    }

    pub async fn connect(&self) -> Connection {
        Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}

/// `future`'s output, or a panic naming `what` when it takes longer than [`ARRIVAL`].
pub async fn within<T>(what: &str, future: impl Future<Output = T>) -> T {
    match select(Box::pin(future), Timer::after(ARRIVAL)).await {
        Either::Left((value, _)) => value,
        Either::Right(_) => panic!("{what}: nothing within {ARRIVAL:?}"),
    }
}

/// `future`'s output if it comes within [`QUIET`].
pub async fn soon<T>(future: impl Future<Output = T>) -> Option<T> {
    match select(Box::pin(future), Timer::after(QUIET)).await {
        Either::Left((value, _)) => Some(value),
        Either::Right(_) => None,
    }
}

/// The signals of `interface` on the bus, as `conn` hears them.
pub async fn signals(conn: &Connection, interface: &str) -> MessageStream {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(interface)
        .unwrap()
        .build();
    MessageStream::for_match_rule(rule, conn, None)
        .await
        .unwrap()
}

/// The next signal of `stream` named `member`, the others skipped.
pub async fn next_signal(stream: &mut MessageStream, member: &str) -> Message {
    within(member, async {
        loop {
            let msg = stream.next().await.unwrap().unwrap();
            if msg.header().member().is_some_and(|m| m.as_str() == member) {
                break msg;
            }
        }
    })
    .await
}
