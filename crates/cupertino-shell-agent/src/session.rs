//! The server's `org.cupertino.Session1` on the session bus (D13): it carries out the log out, restart, shut down and
//! sleep the person confirmed, quitting the applications first (sleep quits none), and this agent answers its interrupt:
//! `QuitInterrupted` brings the interrupt panel ([`crate::interrupt`]), whose Try Again, Cancel and Force Quit are
//! `RetrySessionAction`, `CancelSessionAction` and `ForceQuitApplication`, also for an action this agent did not start
//! (the server sends an interrupt once and waits). A signal counts only from the owner of the name when it arrives (the
//! proxy's signal stream follows the owner): another program could otherwise bring the panel up or take it away. An
//! interrupt sent before this agent listened (it restarted) is asked for once the signals are subscribed
//! ([`pending_interrupt`]).

use std::collections::HashMap;

use futures_util::StreamExt;
use zbus::proxy::CacheProperties;
use zbus::zvariant::Value;
use zbus::Connection;

pub const BUS_NAME: &str = "org.cupertino.Session1";
pub const OBJECT_PATH: &str = "/org/cupertino/Session1";
pub const INTERFACE: &str = "org.cupertino.Session1";

/// Call `method` of `org.cupertino.Session1` with `body`, for its answer.
async fn call<B, T>(session: &Connection, method: &str, body: &B) -> zbus::Result<T>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    session
        .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, body)
        .await?
        .body()
        .deserialize()
}

/// Ask `org.cupertino.Session1` to quit applications and end the session.
pub async fn log_out(session: &Connection) -> zbus::Result<()> {
    call(session, "LogOut", &()).await
}

/// Ask `org.cupertino.Session1` to quit applications and restart the computer.
pub async fn restart(session: &Connection) -> zbus::Result<()> {
    call(session, "Restart", &()).await
}

/// Ask `org.cupertino.Session1` to quit applications and shut down the computer.
pub async fn shut_down(session: &Connection) -> zbus::Result<()> {
    call(session, "ShutDown", &()).await
}

/// Ask `org.cupertino.Session1` to put the computer to sleep; no application quits.
pub async fn sleep(session: &Connection) -> zbus::Result<()> {
    call(session, "Sleep", &()).await
}

/// The identities of the applications that have windows, in the order a log out quits them.
pub async fn open_applications(session: &Connection) -> zbus::Result<Vec<String>> {
    call(session, "OpenApplications", &()).await
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
pub async fn pending_interrupt(session: &Connection) -> zbus::Result<Option<PendingInterrupt>> {
    let (pending, action, app, reason): (bool, String, String, String) =
        call(session, "PendingInterrupt", &()).await?;
    Ok(pending.then_some(PendingInterrupt {
        action,
        app,
        reason,
    }))
}

/// The interrupt panel's Try Again: ask the applications to quit again.
pub async fn retry_session_action(session: &Connection) -> zbus::Result<()> {
    call(session, "RetrySessionAction", &()).await
}

/// The interrupt panel's Cancel: end the sequence, the session goes on.
pub async fn cancel_session_action(session: &Connection) -> zbus::Result<()> {
    call(session, "CancelSessionAction", &()).await
}

/// Kill the processes of `app_id`'s windows; returns how many. The server gives it another full timeout to go.
pub async fn force_quit_application(session: &Connection, app_id: &str) -> zbus::Result<u32> {
    call(session, "ForceQuitApplication", &app_id).await
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

/// The interrupt a signal of Session1 carries, if it is one of the three.
fn interrupt_of(message: &zbus::Message) -> zbus::Result<Option<Interrupt>> {
    let header = message.header();
    Ok(match header.member().map(|m| m.as_str()) {
        Some("QuitInterrupted") => {
            let (app, reason) = message.body().deserialize::<(String, String)>()?;
            Some(Interrupt::Interrupted { app, reason })
        }
        Some("QuitInterruptClosed") => Some(Interrupt::Closed),
        Some("SessionActionFailed") => {
            let (action, reason) = message.body().deserialize::<(String, String)>()?;
            Some(Interrupt::ActionFailed { action, reason })
        }
        // `ApplicationQuit`: the sequence is the server's
        _ => None,
    })
}

/// Hand every `QuitInterrupted`, `QuitInterruptClosed` and `SessionActionFailed` of the name's owner to `notify`,
/// starting with an interrupt pending from before the subscription, until the stream ends; returns why it ended. Runs
/// for as long as the future is polled: the component holds it, so it ends with the component.
pub async fn follow_interrupts(session: &Connection, notify: impl Fn(Interrupt)) -> String {
    let proxy = match zbus::proxy::Builder::<zbus::Proxy<'_>>::new(session)
        .destination(BUS_NAME)
        .and_then(|b| b.path(OBJECT_PATH))
        .and_then(|b| b.interface(INTERFACE))
    {
        Ok(builder) => builder.cache_properties(CacheProperties::No).build().await,
        Err(e) => Err(e),
    };
    let mut signals = match proxy {
        Ok(proxy) => match proxy.receive_all_signals().await {
            Ok(signals) => signals,
            Err(e) => return format!("subscribing to the Session1 signals: {e}"),
        },
        Err(e) => return format!("reaching Session1: {e}"),
    };
    // subscribed above, so an interrupt is either in the answer or comes as a signal; one seen twice is the same
    // interrupt
    match pending_interrupt(session).await {
        Ok(Some(pending)) => notify(Interrupt::Interrupted {
            app: pending.app,
            reason: pending.reason,
        }),
        Ok(None) => {}
        Err(e) => {
            eprintln!("cupertino-shell-agent: no word on an interrupt from before this start: {e}")
        }
    }
    while let Some(message) = signals.next().await {
        match interrupt_of(&message) {
            Ok(Some(interrupt)) => notify(interrupt),
            Ok(None) => {}
            Err(e) => eprintln!("cupertino-shell-agent: a Session1 signal could not be read: {e}"),
        }
    }
    "the Session1 signal stream ended".into()
}

/// Name the session agent's notices go under, as the server's own.
const APP_NAME: &str = "Cupertino";
/// Urgency hint of a failure notice: normal.
const FAILURE_URGENCY: u8 = 1;

/// Tell the person, through the notification service, that what they asked for did not happen: `summary` says what,
/// `body` why. Sent without waiting for an answer: a notification service that never answers holds nothing up.
pub async fn report_failure(session: &Connection, summary: &str, body: &str) -> zbus::Result<()> {
    let hints = HashMap::from([("urgency", Value::U8(FAILURE_URGENCY))]);
    let actions: Vec<&str> = Vec::new();
    // -1: the notification service decides how long the banner stays
    let message = zbus::message::Message::method_call("/org/freedesktop/Notifications", "Notify")?
        .destination("org.freedesktop.Notifications")?
        .interface("org.freedesktop.Notifications")?
        .with_flags(zbus::message::Flags::NoReplyExpected)?
        .build(&(APP_NAME, 0u32, "", summary, body, actions, hints, -1i32))?;
    session.send(&message).await
}
