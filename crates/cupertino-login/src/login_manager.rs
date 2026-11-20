//! The lock screen's calls to the session manager's bus interface `org.cupertino.LoginManager1` (system bus): the
//! announcement that the session is up, and the manager's `Unlock` after a fast user switch and its restarts. The
//! manager authorises every call by the caller's process. Everything here runs on the system bus connection the lock
//! screen's loop ticks.

use futures_lite::stream::{self, StreamExt};
use zbus::fdo::DBusProxy;
use zbus::proxy::CacheProperties;
use zbus::{Connection, Proxy};

const BUS_NAME: &str = "org.cupertino.LoginManager1";
const OBJECT_PATH: &str = "/org/cupertino/LoginManager1";

/// What the manager tells the lock screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum News {
    /// `Unlock`: the person at the login screen proved this session's password.
    Unlock,
    /// The manager's name has an owner again: it started or restarted, and its table of agents is gone, so the session
    /// announces itself again.
    Restarted,
}

async fn manager(system: &Connection) -> zbus::Result<Proxy<'static>> {
    zbus::proxy::Builder::new(system)
        .destination(BUS_NAME)?
        .path(OBJECT_PATH)?
        .interface(BUS_NAME)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// Tell the manager this session's agent is up (`SessionReady`). A session the manager did not start (one on a
/// getty, a test's) is not known to it: the caller reports the error.
pub async fn session_ready(system: &Connection) -> zbus::Result<()> {
    manager(system)
        .await?
        .call_method("SessionReady", &())
        .await
        .map(drop)
}

/// Hand the manager's [`News`] to `notify` until a stream ends; returns why. `following` runs once both are
/// subscribed to, so news the manager sends after it is heard. An `Unlock` counts from the owner of the manager's name
/// alone (only root may own it), which the proxy follows; a new owner from the bus alone.
pub async fn follow(
    system: &Connection,
    following: impl FnOnce(),
    notify: impl Fn(News),
) -> String {
    let manager = match manager(system).await {
        Ok(manager) => manager,
        Err(e) => return format!("reaching the session manager: {e}"),
    };
    let unlocks = match manager.receive_signal("Unlock").await {
        Ok(unlocks) => unlocks.map(|_| Some(News::Unlock)),
        Err(e) => return format!("subscribing to the session manager's Unlock: {e}"),
    };
    let owners = match DBusProxy::new(system).await {
        Ok(bus) => match bus
            .receive_name_owner_changed_with_args(&[(0, BUS_NAME)])
            .await
        {
            Ok(owners) => owners.map(|signal| {
                signal
                    .args()
                    .is_ok_and(|args| args.new_owner.is_some())
                    .then_some(News::Restarted)
            }),
            Err(e) => return format!("following the session manager's name: {e}"),
        },
        Err(e) => return format!("reaching the bus: {e}"),
    };
    following();
    let mut news = stream::or(unlocks, owners);
    while let Some(news) = news.next().await {
        if let Some(news) = news {
            notify(news);
        }
    }
    "the session manager's signal streams ended".into()
}
