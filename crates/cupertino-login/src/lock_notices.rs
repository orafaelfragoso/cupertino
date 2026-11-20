//! The notifications the lock screen shows: what the notifications component lists for it, and where the cards sit.
//!
//! The notifications component (`notifications`, run by `cupertino-shell`) decides what may show: while the session is
//! locked, its `org.cupertino.NotificationCenter1.LockScreenNotifications` lists the notifications that arrived since the
//! lock from applications allowed on the lock screen, each as its application, a title and a body. The title is
//! "Notification" and the body empty unless the application's previews are always shown, so the rest of a notification
//! never reaches this process, its windows or the tree assistive technology reads. The component says when the list
//! changed with the signal `LockScreenChanged`, which carries nothing; [`follow`] then has the list asked for again, and again
//! when the bus name changes owner. Each text is cut again here to its cap, whatever the component sent.
//!
//! Any process of the user could own the bus name while the component is not running, so the name alone is not trusted
//! (D20: checks are per component, never by process relations or program paths): [`fetch`] asks the bus for the name's
//! owner and asks that very connection only when the server resolves it to the component [`NOTIFICATIONS_COMPONENT`]
//! ([`serves_lock_screen`]: `cupertino_peer::component`, the server's `ShellHosts` and `ComponentOf`). Any other owner,
//! another component of the same host included, gets no question and the list is empty, so no other program can put
//! words on the lock screen.

use cupertino_geometry::Rect;

/// One notification on the lock screen, as the notifications component gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub id: u32,
    pub app: String,
    pub title: String,
    /// Empty when the notification shows without its preview.
    pub body: String,
}

/// The most notices kept from a list.
pub const MAX_NOTICES: usize = 20;
/// Longest application name, title and body kept, in characters; a longer one ends in an ellipsis.
pub const MAX_APP_CHARS: usize = 64;
pub const MAX_TITLE_CHARS: usize = 256;
pub const MAX_BODY_CHARS: usize = 256;
/// What ends a text that was cut.
const ELLIPSIS: char = '\u{2026}';

/// How many cards the lock screen shows at most, newest first; a line under them counts the rest.
pub const SHOWN: usize = 3;
/// A card: its width, its distance from the screen's right edge and from the top, the space between two cards, its
/// padding and corner radius.
pub const CARD_WIDTH: f64 = 344.0;
pub const CARD_MARGIN: f64 = 16.0;
pub const CARD_TOP: f64 = 44.0;
pub const CARD_GAP: f64 = 8.0;
pub const CARD_PADDING: f64 = 12.0;
pub const CARD_RADIUS: f64 = 16.0;
/// Type sizes of the application line, the title and the body, the lines the body may take and the space between
/// the lines.
pub const APP_SIZE: f64 = 12.0;
pub const TITLE_SIZE: f64 = 13.0;
pub const BODY_SIZE: f64 = 13.0;
pub const BODY_LINES: u32 = 2;
pub const LINE_GAP: f64 = 2.0;

/// `text` with at most `max` characters, a longer one cut between user-perceived characters (grapheme clusters, so a
/// letter keeps its marks and an emoji sequence stays whole) and ending in an ellipsis; control characters become
/// spaces.
pub fn cap(text: &str, max: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() <= max {
        return clean;
    }
    let room = max.saturating_sub(1);
    let (mut end, mut used) = (0, 0);
    for (at, cluster) in clean.grapheme_indices(true) {
        let chars = cluster.chars().count();
        if used + chars > room {
            break;
        }
        used += chars;
        end = at + cluster.len();
    }
    let mut out = clean[..end].trim_end().to_owned();
    out.push(ELLIPSIS);
    out
}

/// The notices of a `LockScreenNotifications` reply, at most [`MAX_NOTICES`], each text cut to its cap.
pub fn from_rows(rows: Vec<(u32, String, String, String)>) -> Vec<Notice> {
    rows.into_iter()
        .take(MAX_NOTICES)
        .map(|(id, app, title, body)| Notice {
            id,
            app: cap(&app, MAX_APP_CHARS),
            title: cap(&title, MAX_TITLE_CHARS),
            body: cap(&body, MAX_BODY_CHARS),
        })
        .collect()
}

impl Notice {
    /// What assistive technology hears of the card: "app: title", then the body.
    pub fn speech(&self) -> String {
        let head = format!("{}: {}", self.app, self.title);
        match self.body.trim() {
            "" => head,
            body => format!(
                "{head}, {}",
                body.split_whitespace().collect::<Vec<_>>().join(" ")
            ),
        }
    }
}

/// Where the column of cards sits on a screen `width` by `height`: at the top right.
pub fn column(width: f64, height: f64) -> Rect {
    Rect::new(
        width - CARD_MARGIN - CARD_WIDTH,
        CARD_TOP,
        CARD_WIDTH,
        (height - CARD_TOP).max(0.0),
    )
}

/// The height of a card whose body takes `body_lines` lines (0 without a body, at most [`BODY_LINES`]), in a font with
/// `ascent` and `descent` per em.
pub fn card_height(body_lines: u32, ascent: f64, descent: f64) -> f64 {
    let line = |size: f64| (ascent + descent) * size;
    let mut height = 2.0 * CARD_PADDING + line(APP_SIZE) + LINE_GAP + line(TITLE_SIZE);
    if body_lines > 0 {
        height += LINE_GAP + f64::from(body_lines.min(BODY_LINES)) * line(BODY_SIZE);
    }
    height.ceil()
}

/// The notices that get a card, newest first, and how many more there are.
pub fn shown(notices: &[Notice]) -> (&[Notice], usize) {
    let cut = notices.len().min(SHOWN);
    (&notices[..cut], notices.len() - cut)
}

/// What the line under the cards says of the `n` notices they do not show.
pub fn more_text(n: usize) -> String {
    match n {
        1 => "1 more notification".to_owned(),
        n => format!("{n} more notifications"),
    }
}

/// The shell component whose bus connection alone may answer the lock screen's list, by its name in the server's
/// component table (`ComponentOf`).
pub const NOTIFICATIONS_COMPONENT: &str = "notifications";

#[cfg(feature = "dbus")]
pub use bus::{fetch, follow, serves_lock_screen};

#[cfg(feature = "dbus")]
mod bus {
    use super::{from_rows, Notice, NOTIFICATIONS_COMPONENT};
    use cupertino_peer::component::{resolve_component, ComponentCaller};
    use futures_lite::stream::{self, StreamExt};
    use zbus::fdo::DBusProxy;
    use zbus::names::{BusName, UniqueName};
    use zbus::proxy::CacheProperties;
    use zbus::{Connection, Proxy};

    const BUS_NAME: &str = "org.cupertino.NotificationCenter1";
    const OBJECT_PATH: &str = "/org/cupertino/NotificationCenter1";
    const INTERFACE: &str = "org.cupertino.NotificationCenter1";

    /// Whether the server resolves the connection `owner` to the component [`NOTIFICATIONS_COMPONENT`] (D20). The
    /// server itself, another component of a shell host and any other connection are not.
    pub async fn serves_lock_screen(session: &Connection, owner: &UniqueName<'_>) -> bool {
        let caller = resolve_component(session, owner).await;
        matches!(caller, ComponentCaller::Component(name) if name == NOTIFICATIONS_COMPONENT)
    }

    /// The list the notifications component gives the lock screen now, asked of the owner of its bus name only when
    /// that connection is the component's ([`serves_lock_screen`]); any other owner is an error and is not asked. No
    /// owner is an empty list.
    pub async fn fetch(session: &Connection) -> zbus::Result<Vec<Notice>> {
        let dbus = DBusProxy::new(session).await?;
        let name = BusName::try_from(BUS_NAME)?;
        let owner = match dbus.get_name_owner(name).await {
            Ok(owner) => owner,
            Err(zbus::fdo::Error::NameHasNoOwner(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        if !serves_lock_screen(session, &owner).await {
            return Err(zbus::Error::Failure(format!(
                "{BUS_NAME} is owned by {owner}, which is not the {NOTIFICATIONS_COMPONENT} component"
            )));
        }
        // the owner's own connection, so a new owner of the name cannot answer in its place
        let reply = session
            .call_method(
                Some(owner.as_str()),
                OBJECT_PATH,
                Some(INTERFACE),
                "LockScreenNotifications",
                &(),
            )
            .await;
        match reply {
            Ok(reply) => Ok(from_rows(reply.body().deserialize()?)),
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown"
                    || name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" =>
            {
                Ok(Vec::new())
            }
            Err(e) => Err(e),
        }
    }

    /// Call `due` now, at every `LockScreenChanged` from the owner of the component's name and whenever the name
    /// changes owner, until a stream ends; returns why. `due` must not wait: it only notes that the list is to be
    /// read again ([`fetch`]), so a burst of signals costs one read. The proxy follows the owner of the name and
    /// passes on what that connection sent alone, so a forged `LockScreenChanged` is dropped as it arrives; a new
    /// owner counts from the bus alone.
    pub async fn follow(session: &Connection, due: impl Fn()) -> String {
        let center = match zbus::proxy::Builder::<Proxy<'_>>::new(session)
            .destination(BUS_NAME)
            .and_then(|b| b.path(OBJECT_PATH))
            .and_then(|b| b.interface(INTERFACE))
        {
            Ok(builder) => builder.cache_properties(CacheProperties::No).build().await,
            Err(e) => Err(e),
        };
        let center = match center {
            Ok(center) => center,
            Err(e) => return format!("reaching the notifications component: {e}"),
        };
        let changes = match center.receive_signal("LockScreenChanged").await {
            Ok(changes) => changes.map(drop),
            Err(e) => return format!("subscribing to LockScreenChanged: {e}"),
        };
        let owners = match DBusProxy::new(session).await {
            Ok(bus) => match bus
                .receive_name_owner_changed_with_args(&[(0, BUS_NAME)])
                .await
            {
                Ok(owners) => owners.map(drop),
                Err(e) => return format!("following the notifications component's name: {e}"),
            },
            Err(e) => return format!("reaching the bus: {e}"),
        };
        // the first read, after both streams are subscribed, so no change between them and it is missed
        due();
        let mut news = stream::or(changes, owners);
        while news.next().await.is_some() {
            due();
        }
        "the notifications component's signal streams ended".into()
    }
}
