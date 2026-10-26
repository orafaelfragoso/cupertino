//! The server's `org.cupertino.Session1` on the session bus, which quits the applications before a log out: the calls
//! and the `ApplicationQuit` signal that [`crate::logout`] times.

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
        .interface(INTERFACE)?
        .member("ApplicationQuit")?
        .path(OBJECT_PATH)?
        .build();
    let messages = MessageIterator::for_match_rule(rule, session, None)?;
    std::thread::Builder::new()
        .name("session-quit".into())
        .spawn(move || {
            for message in messages {
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
