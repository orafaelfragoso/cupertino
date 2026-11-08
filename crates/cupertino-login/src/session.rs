//! The server's `org.cupertino.Session1` on the session bus, which quits the applications before a log out: the calls
//! and the `ApplicationQuit` signal that [`crate::logout`] times. An `ApplicationQuit` counts only from the server, the
//! owner of the name when it arrives: another program could otherwise hurry the log out along.

use cupertino_peer::signal::sent_by_owner;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::MatchRule;

const BUS_NAME: &str = "org.cupertino.Session1";
const OBJECT_PATH: &str = "/org/cupertino/Session1";
const INTERFACE: &str = "org.cupertino.Session1";

/// Ask the most recently used application with windows to quit; `("", 0)` when none is left.
pub fn quit_next_application(session: &Connection) -> zbus::Result<(String, u32)> {
    session
        .call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            "QuitNextApplication",
            &(),
        )?
        .body()
        .deserialize()
}

/// Kill the processes of `app_id`'s windows; returns how many.
pub fn force_quit_application(session: &Connection, app_id: &str) -> zbus::Result<u32> {
    session
        .call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            "ForceQuitApplication",
            &app_id,
        )?
        .body()
        .deserialize()
}

/// Pass each `ApplicationQuit` to `notify` from a reader thread; it gets an error once when the stream breaks, and
/// nothing after that.
pub fn watch_quit(
    session: &Connection,
    notify: impl Fn(zbus::Result<String>) + Send + 'static,
) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(BUS_NAME)?
        .interface(INTERFACE)?
        .member("ApplicationQuit")?
        .path(OBJECT_PATH)?
        .build();
    let messages = MessageIterator::for_match_rule(rule, session, None)?;
    let connection = session.clone();
    std::thread::Builder::new()
        .name("session-quit".into())
        .spawn(move || {
            for message in messages {
                if message
                    .as_ref()
                    .is_ok_and(|m| !sent_by_owner(&connection, m, BUS_NAME))
                {
                    eprintln!(
                        "cupertino-login: an ApplicationQuit not sent by the server was ignored"
                    );
                    continue;
                }
                match message.and_then(|m| m.body().deserialize::<String>()) {
                    Ok(app) => notify(Ok(app)),
                    Err(e) => return notify(Err(e)),
                }
            }
            notify(Err(zbus::Error::Failure(
                "the ApplicationQuit stream ended".into(),
            )));
        })
        .map(drop)
        .map_err(|e| zbus::Error::Failure(format!("starting the ApplicationQuit reader: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn only_the_server_says_an_application_quit() {
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let server = bus.connect();
        server.request_name(BUS_NAME).unwrap();
        let agent = bus.connect();
        let (tx, rx) = mpsc::channel();
        watch_quit(&agent, move |quit| {
            let _ = tx.send(quit.map_err(|e| e.to_string()));
        })
        .unwrap();
        let to = agent.unique_name().unwrap().to_string();
        let quit = |from: &Connection, to: Option<&str>, app: &str| {
            from.emit_signal(to, OBJECT_PATH, INTERFACE, "ApplicationQuit", &app)
                .unwrap()
        };
        let peer = bus.connect();
        for to in [Some(to.as_str()), None] {
            quit(&peer, to, "org.example.Forged");
        }
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "a forged quit is ignored"
        );
        quit(&server, None, "org.example.Editor");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Ok("org.example.Editor".to_owned()))
        );
    }
}
