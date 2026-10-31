//! `org.freedesktop.PolicyKit1.AuthenticationAgent` on the system bus, registered with polkitd for this session.
//!
//! Only polkitd itself may start or cancel an authentication: every call is checked against the current owner of
//! `org.freedesktop.PolicyKit1`, so another program on the bus can neither raise the prompt with its own text nor
//! cancel one. The agent never reports success: polkitd learns it from the helper, and the reply to
//! `BeginAuthentication` only says that the agent is done.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{blocking, interface, Connection};

use crate::text::shown;

/// polkitd's bus name.
pub const POLKIT: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";
/// Where the agent's object lives.
pub const AGENT_PATH: &str = "/org/cupertino/PolkitAgent";
/// The most authentications that may wait at once, the one on screen included; more are refused at once, so a flood
/// of requests can't stack prompts.
pub const MAX_PENDING: usize = 4;

/// A user polkitd will accept a password of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct User(pub u32);

/// One authentication polkitd asks for. Every text is already made safe to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub cookie: String,
    pub action: String,
    pub message: String,
    pub details: Vec<(String, String)>,
    /// The users polkitd accepts, in its order; identities other than users are left out.
    pub users: Vec<User>,
}

/// How an authentication ended for polkitd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The helper ran and told polkitd its result, or nobody could be authenticated.
    Done,
    /// The person dismissed the prompt, or polkitd cancelled it.
    Cancelled,
}

/// What the agent hands to whoever shows the prompt.
pub enum Event {
    /// Show `Request`; answer with `Reply` exactly once.
    Begin(Request, Reply),
    /// polkitd no longer wants the authentication of this cookie.
    Cancel(String),
}

/// The answer to one [`Event::Begin`]. Dropping it unanswered counts as [`Outcome::Cancelled`].
pub struct Reply(async_channel::Sender<Outcome>);

impl Reply {
    pub fn send(self, outcome: Outcome) {
        let _ = self.0.try_send(outcome);
    }
}

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
pub struct Agent {
    events: Box<dyn Fn(Event) + Send + Sync>,
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
        let (reply, outcome) = async_channel::bounded(1);
        (self.events)(Event::Begin(
            Request {
                cookie,
                action: shown(&action_id),
                message: shown(&message),
                details,
                users,
            },
            Reply(reply),
        ));
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
pub fn register(
    connection: &blocking::Connection,
    session_id: &str,
    locale: &str,
) -> zbus::Result<()> {
    let subject: (&str, HashMap<&str, Value<'_>>) = (
        "unix-session",
        HashMap::from([("session-id", Value::from(session_id))]),
    );
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    connection.call_method(
        Some(POLKIT),
        AUTHORITY_PATH,
        Some(AUTHORITY_INTERFACE),
        "RegisterAuthenticationAgentWithOptions",
        &(subject, locale, AGENT_PATH, options),
    )?;
    Ok(())
}

/// Serves the agent on `connection` and registers it with polkitd for `session_id`, then again whenever polkitd
/// starts anew; `events` receives what polkitd asks. Returns once the first registration is done, with the thread that
/// follows polkitd's restarts running.
pub fn serve(
    connection: &blocking::Connection,
    session_id: &str,
    locale: &str,
    events: impl Fn(Event) + Send + Sync + 'static,
) -> zbus::Result<()> {
    connection.object_server().at(
        AGENT_PATH,
        Agent {
            events: Box::new(events),
            pending: Arc::new(AtomicUsize::new(0)),
        },
    )?;
    let owners = blocking::fdo::DBusProxy::new(connection)?
        .receive_name_owner_changed_with_args(&[(0, POLKIT)])?;
    register(connection, session_id, locale)?;
    let (connection, session_id, locale) =
        (connection.clone(), session_id.to_owned(), locale.to_owned());
    std::thread::Builder::new()
        .name("polkit-owner".to_owned())
        .spawn(move || {
            for change in owners {
                let Ok(args) = change.args() else { continue };
                if args.new_owner().is_some() {
                    if let Err(error) = register(&connection, &session_id, &locale) {
                        eprintln!(
                            "cupertino-polkit-agent: registering again with polkitd: {error}"
                        );
                    }
                }
            }
        })
        .map_err(|error| zbus::Error::Failure(error.to_string()))?;
    Ok(())
}
