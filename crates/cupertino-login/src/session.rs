//! The server's `org.cupertino.Session1` on the session bus (D13): it carries out the log out, restart, shut down and
//! sleep the person confirmed, quitting the applications first (sleep quits none), and this agent answers its interrupt:
//! `QuitInterrupted` brings the interrupt panel ([`crate::interrupt`]), whose Try Again, Cancel and Force Quit are
//! `RetrySessionAction`, `CancelSessionAction` and `ForceQuitApplication`, also for an action this agent did not start
//! (the server sends an interrupt once and waits). A signal counts only from the server, the owner of the name when it
//! arrives: another program could otherwise bring the panel up or take it away. An interrupt sent before this agent
//! listened (it restarted) is asked for once the signals are subscribed ([`pending_interrupt`]).

use std::collections::HashMap;

use cupertino_peer::signal::from_owner;
use zbus::blocking::Connection;
use zbus::message::Type;
use zbus::zvariant::Value;
use zbus::MatchRule;

const BUS_NAME: &str = "org.cupertino.Session1";
const OBJECT_PATH: &str = "/org/cupertino/Session1";
const INTERFACE: &str = "org.cupertino.Session1";

/// Call `method` of `org.cupertino.Session1` without arguments, for its answer.
fn call<T>(session: &Connection, method: &str) -> zbus::Result<T>
where
    T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    session
        .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, &())?
        .body()
        .deserialize()
}

/// Ask `org.cupertino.Session1` to quit applications and end the session.
pub fn log_out(session: &Connection) -> zbus::Result<()> {
    call(session, "LogOut")
}

/// Ask `org.cupertino.Session1` to quit applications and restart the computer.
pub fn restart(session: &Connection) -> zbus::Result<()> {
    call(session, "Restart")
}

/// Ask `org.cupertino.Session1` to quit applications and shut down the computer.
pub fn shut_down(session: &Connection) -> zbus::Result<()> {
    call(session, "ShutDown")
}

/// Ask `org.cupertino.Session1` to put the computer to sleep; no application quits.
pub fn sleep(session: &Connection) -> zbus::Result<()> {
    call(session, "Sleep")
}

/// The identities of the applications that have windows, in the order a log out quits them.
pub fn open_applications(session: &Connection) -> zbus::Result<Vec<String>> {
    call(session, "OpenApplications")
}

/// An interrupt the server's quit sequence waits on (`PendingInterrupt`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingInterrupt {
    /// The interrupted action: `LogOut`, `Restart` or `ShutDown`.
    pub action: String,
    /// The application that did not quit.
    pub app: String,
    /// Its quit status, as in [`Interrupt::Interrupted`].
    pub reason: String,
}

/// The interrupt the quit sequence waits on, if any: `QuitInterrupted` is sent once, so an agent that was not there
/// for it, or does not know the action it belongs to, asks.
pub fn pending_interrupt(session: &Connection) -> zbus::Result<Option<PendingInterrupt>> {
    let (pending, action, app, reason): (bool, String, String, String) =
        call(session, "PendingInterrupt")?;
    Ok(pending.then_some(PendingInterrupt {
        action,
        app,
        reason,
    }))
}

/// The interrupt panel's Try Again: ask the applications to quit again.
pub fn retry_session_action(session: &Connection) -> zbus::Result<()> {
    call(session, "RetrySessionAction")
}

/// The interrupt panel's Cancel: end the sequence, the session goes on.
pub fn cancel_session_action(session: &Connection) -> zbus::Result<()> {
    call(session, "CancelSessionAction")
}

/// Kill the processes of `app_id`'s windows; returns how many. The server gives it another full timeout to go.
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

/// What the server says about the applications it is quitting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Interrupt {
    /// `QuitInterrupted`: `app` did not quit in time, for `reason` (its quit status, see
    /// [`crate::interrupt::Reason::of_status`]).
    Interrupted { app: String, reason: String },
    /// `QuitInterruptClosed`: the application quit after all; the sequence goes on.
    Closed,
    /// `SessionActionFailed`: the final step of `action` (`LogOut`, `Restart`, `ShutDown` or `Sleep`) was refused or
    /// failed for `reason`; Session1 is idle and the session goes on.
    ActionFailed { action: String, reason: String },
}

/// Pass each `QuitInterrupted`, `QuitInterruptClosed` and `SessionActionFailed` to `notify` from a reader thread; it gets an error once when
/// the stream breaks, and nothing after that.
pub fn watch_interrupts(
    session: &Connection,
    notify: impl Fn(zbus::Result<Interrupt>) + Send + 'static,
) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(BUS_NAME)?
        .interface(INTERFACE)?
        .path(OBJECT_PATH)?
        .build();
    let messages = from_owner(session, BUS_NAME, rule, |_| {
        eprintln!("cupertino-login: a quit signal not sent by the server was ignored")
    })?;
    let asking = session.clone();
    std::thread::Builder::new()
        .name("session-quit".into())
        .spawn(move || {
            // subscribed above, so an interrupt is either in the answer or comes as a signal; one seen twice is
            // the same interrupt
            match pending_interrupt(&asking) {
                Ok(Some(pending)) => notify(Ok(Interrupt::Interrupted {
                    app: pending.app,
                    reason: pending.reason,
                })),
                Ok(None) => {}
                Err(e) => eprintln!(
                    "cupertino-login: no word on an interrupt from before this start: {e}"
                ),
            }
            for message in messages {
                let interrupt = message.and_then(|m| {
                    let member = m.header().member().map(|n| n.to_string());
                    Ok(match member.as_deref() {
                        Some("QuitInterrupted") => {
                            let (app, reason) = m.body().deserialize::<(String, String)>()?;
                            Some(Interrupt::Interrupted { app, reason })
                        }
                        Some("QuitInterruptClosed") => Some(Interrupt::Closed),
                        Some("SessionActionFailed") => {
                            let (action, reason) = m.body().deserialize::<(String, String)>()?;
                            Some(Interrupt::ActionFailed { action, reason })
                        }
                        // `ApplicationQuit`: the sequence is the server's
                        _ => None,
                    })
                });
                match interrupt {
                    Ok(Some(interrupt)) => notify(Ok(interrupt)),
                    Ok(None) => {}
                    Err(e) => return notify(Err(e)),
                }
            }
            notify(Err(zbus::Error::Failure(
                "the Session1 signal stream ended".into(),
            )));
        })
        .map(drop)
        .map_err(|e| zbus::Error::Failure(format!("starting the Session1 signal reader: {e}")))
}

/// Name the session agent's notices go under, as the server's own.
const APP_NAME: &str = "Cupertino";
/// Urgency hint of a failure notice: normal.
const FAILURE_URGENCY: u8 = 1;

/// Tell the person, through the notification service, that what they asked for did not happen: `summary` says what,
/// `body` why. Sent without waiting for an answer: a notification service that never answers holds nothing up.
pub fn report_failure(session: &Connection, summary: &str, body: &str) -> zbus::Result<()> {
    let hints = HashMap::from([("urgency", Value::U8(FAILURE_URGENCY))]);
    let actions: Vec<&str> = Vec::new();
    // -1: the notification service decides how long the banner stays
    let message = zbus::message::Message::method_call("/org/freedesktop/Notifications", "Notify")?
        .destination("org.freedesktop.Notifications")?
        .interface("org.freedesktop.Notifications")?
        .with_flags(zbus::message::Flags::NoReplyExpected)?
        .build(&(APP_NAME, 0u32, "", summary, body, actions, hints, -1i32))?;
    session.send(&message)
}
