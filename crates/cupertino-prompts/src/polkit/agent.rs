//! `org.freedesktop.PolicyKit1.AuthenticationAgent` on the system bus, registered with polkitd for this session.
//!
//! Only polkitd itself may start or cancel an authentication: every call is checked against the current owner of
//! `org.freedesktop.PolicyKit1`, so another program on the bus can neither raise the prompt with its own text nor
//! cancel one. The agent never reports success: polkitd learns it from the helper, and the reply to
//! `BeginAuthentication` only says that the agent is done.
//!
//! The same goes for the signals the agent acts on: the bus delivers a signal addressed to the agent whatever its match
//! rules say, and zbus compares a rule's sender with a message only when the rule names a unique name, so each
//! `NameOwnerChanged` is taken only when the bus daemon sent it ([`sent_by_bus`]).
//!
//! Everything here runs on the system bus connection Kit's loop ticks: the interface's calls, the registration and the
//! future that follows polkitd's restarts.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cupertino_diagnostic_events::{self as diag, Severity, Value as Field};
use cupertino_peer::signal::{sent_by_bus, BUS_DAEMON};
use futures_lite::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::message::{Header, Type};
use zbus::names::BusName;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{interface, Connection, MatchRule, MessageStream};

use crate::polkit::{Event, Outcome, Reply, Request, User};
use crate::text::shown;

/// polkitd's bus name.
pub const POLKIT: &str = "org.freedesktop.PolicyKit1";
pub const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
pub const AUTHORITY_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";
const BUS_DAEMON_PATH: &str = "/org/freedesktop/DBus";
/// Where the agent's object lives.
pub const AGENT_PATH: &str = "/org/cupertino/PolkitAgent";
/// The most authentications that may wait at once, the one on screen included; more are refused at once, so a flood
/// of requests can't stack prompts.
pub const MAX_PENDING: usize = 4;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    #[zbus(name = "PolicyKit1.Error.Cancelled")]
    Cancelled(String),
    #[zbus(name = "PolicyKit1.Error.Failed")]
    Failed(String),
    #[zbus(name = "DBus.Error.AccessDenied")]
    AccessDenied(String),
}

/// The object on the bus.
struct Agent {
    /// Hands an event to the prompt; false when it is not shown.
    events: Box<dyn Fn(Event) -> bool + Send + Sync>,
    pending: Arc<AtomicUsize>,
}

/// Counts one waiting authentication for as long as it lives.
struct Pending(Arc<AtomicUsize>);

impl Pending {
    fn take(count: &Arc<AtomicUsize>) -> Option<Pending> {
        count
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_PENDING).then_some(n + 1)
            })
            .ok()
            .map(|_| Pending(Arc::clone(count)))
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Refuses a call that does not come from the current owner of polkitd's name.
async fn from_polkit(header: &Header<'_>, connection: &Connection) -> Result<(), AgentError> {
    let denied = || AgentError::AccessDenied("only polkitd may ask the agent".to_owned());
    let sender = header.sender().ok_or_else(denied)?;
    let owner = DBusProxy::new(connection)
        .await?
        .get_name_owner(BusName::try_from(POLKIT).map_err(zbus::Error::from)?)
        .await
        .map_err(|_| denied())?;
    if *sender == *owner {
        Ok(())
    } else {
        Err(denied())
    }
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    #[allow(clippy::too_many_arguments)]
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        _icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), AgentError> {
        from_polkit(&header, connection).await?;
        let Some(_pending) = Pending::take(&self.pending) else {
            return Err(AgentError::Failed(
                "too many authentications are waiting".to_owned(),
            ));
        };
        let users: Vec<User> = identities
            .iter()
            .filter(|(kind, _)| kind == "unix-user")
            .filter_map(|(_, details)| u32::try_from(details.get("uid")?).ok())
            .map(User)
            .collect();
        if users.is_empty() {
            return Err(AgentError::Failed(
                "no user that can be authenticated".to_owned(),
            ));
        }
        let mut details: Vec<(String, String)> = details
            .into_iter()
            .map(|(key, value)| (shown(&key), shown(&value)))
            .collect();
        details.sort();
        let (reply, outcome) = Reply::new();
        let request = Request {
            cookie,
            action: shown(&action_id),
            message: shown(&message),
            details,
            users,
        };
        if !(self.events)(Event::Begin(request, reply)) {
            return Err(AgentError::Failed("the prompt is not shown".to_owned()));
        }
        match outcome.recv().await {
            Ok(Outcome::Done) => Ok(()),
            Ok(Outcome::Cancelled) | Err(_) => Err(AgentError::Cancelled(
                "the authentication was dismissed".to_owned(),
            )),
        }
    }

    async fn cancel_authentication(
        &self,
        cookie: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), AgentError> {
        from_polkit(&header, connection).await?;
        (self.events)(Event::Cancel(cookie));
        Ok(())
    }
}

/// Registers the agent at [`AGENT_PATH`] with polkitd for the logind session `session_id`.
pub async fn register(connection: &Connection, session_id: &str, locale: &str) -> zbus::Result<()> {
    let subject: (&str, HashMap<&str, Value<'_>>) = (
        "unix-session",
        HashMap::from([("session-id", Value::from(session_id))]),
    );
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    connection
        .call_method(
            Some(POLKIT),
            AUTHORITY_PATH,
            Some(AUTHORITY_INTERFACE),
            "RegisterAuthenticationAgentWithOptions",
            &(subject, locale, AGENT_PATH, options),
        )
        .await?;
    Ok(())
}

/// Records `polkit.register.failed`: polkitd did not take the agent's registration, when the agent `start`s or `again`
/// after polkitd started anew.
fn register_failed(when: &'static str, error: &zbus::Error) {
    let mut event = diag::event(
        Severity::Error,
        "polkit.register.failed",
        "the agent could not register with polkitd",
    )
    .field("when", Field::Public(when));
    if let zbus::Error::MethodError(name, _, _) = error {
        event = event.field("error_name", Field::Public(name.as_str()));
    }
    event
        .field("error", Field::Public(&error.to_string()))
        .emit();
}

/// Exports the agent on `connection`; every call polkitd makes goes to `events`, which says whether it took it.
pub async fn export(
    connection: &Connection,
    events: impl Fn(Event) -> bool + Send + Sync + 'static,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(
            AGENT_PATH,
            Agent {
                events: Box::new(events),
                pending: Arc::new(AtomicUsize::new(0)),
            },
        )
        .await?;
    Ok(())
}

/// Registers the exported agent with polkitd for `session_id`, then again whenever polkitd starts anew, until the
/// connection closes; returns why it stopped. `registered` hears whether the first registration was taken. Only the bus
/// daemon's `NameOwnerChanged` counts as a restart, checked on each message ([`sent_by_bus`]). A polkitd that is not
/// running yet is registered with once it starts.
pub async fn follow(
    connection: &Connection,
    session_id: &str,
    locale: &str,
    registered: impl FnOnce(Result<(), String>),
) -> String {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(BUS_DAEMON)
        .and_then(|r| r.path(BUS_DAEMON_PATH))
        .and_then(|r| r.interface(BUS_DAEMON))
        .and_then(|r| r.member("NameOwnerChanged"))
        .and_then(|r| r.arg(0, POLKIT))
        .map(|r| r.build());
    let owners = match rule {
        Ok(rule) => MessageStream::for_match_rule(rule, connection, None).await,
        Err(e) => Err(e),
    };
    let mut owners = match owners {
        Ok(owners) => owners,
        Err(e) => return format!("following polkitd: {e}"),
    };
    let first = register(connection, session_id, locale).await;
    if let Err(error) = &first {
        register_failed("start", error);
    }
    registered(first.map_err(|e| e.to_string()));
    while let Some(change) = owners.next().await {
        let Ok(change) = change else { continue };
        if !sent_by_bus(&change) {
            eprintln!("cupertino-prompts: ignored a NameOwnerChanged the bus did not send");
            continue;
        }
        let Ok((_, _, new_owner)) = change.body().deserialize::<(String, String, String)>() else {
            continue;
        };
        if !new_owner.is_empty() {
            if let Err(error) = register(connection, session_id, locale).await {
                eprintln!("cupertino-prompts: registering again with polkitd: {error}");
                register_failed("again", &error);
            }
        }
    }
    "the system bus closed".to_owned()
}
