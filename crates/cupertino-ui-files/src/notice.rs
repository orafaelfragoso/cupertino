//! Failures a person caused, reported as notifications on the session bus (feature `app`). A [`Notice`] is worded by
//! the action that failed ([`failure`]) or by the removable media calls; [`Notices`] posts each in order from one future
//! the program runs on its UI loop, on the bus connection it gives, so the window never waits on the bus. A notice that
//! can't be posted is logged instead, once. A notice posted with a tag ([`Notices::post_clickable`]) can be clicked:
//! the click comes back as a [`Click`] ([`Notices::clicks`]).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::future::Future;
use std::path::Path;

use futures_lite::StreamExt;
use zbus::message::Type;
use zbus::zvariant::Value;
use zbus::{Connection, MatchRule, Message, MessageStream};

use crate::operations::DropAction;
use crate::rename::{NameError, MAX_NAME_BYTES};

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const APP_NAME: &str = "Files";
const DESKTOP_ENTRY: &str = "cupertino-files";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
/// The action a click on a notice's body invokes, and its label.
const DEFAULT_ACTION: &str = "default";
const DEFAULT_LABEL: &str = "Show";
/// The most clickable notices whose clicks are still heard; an older one's click is ignored.
const MAX_CLICKABLE: usize = 64;

/// The most characters of a name or a message that Files shows; longer text ends in an ellipsis.
const MAX_CHARS: usize = 256;

/// `text` as shown: a name or an error message is someone else's text, so it loses the control and bidirectional
/// formatting characters that would break the line or reorder the sentence around it, and is cut to [`MAX_CHARS`].
pub fn shown(text: &str) -> String {
    let mut out = String::new();
    for (count, c) in text
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
        })
        .enumerate()
    {
        if count == MAX_CHARS {
            out.pop();
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

/// `text` as literal notification body text: the body is read as markup, where a link would open.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A notification's text and icon (empty: the application's icon).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
    pub icon: &'static str,
}

/// What a person did that failed, which words the notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    NewFolder,
    MoveIntoNewFolder,
    Duplicate,
    MakeAlias,
    ShowOriginal,
    PutBack,
    Tag,
    Rename,
    Copy,
    Paste,
    Drop(DropAction),
    Trash,
}

/// The notice for `action` failing on `items` (none for an action on no item) with `error`.
pub fn failure(action: Action, items: &[&Path], error: &dyn Display) -> Notice {
    let subject = match items {
        [item] => format!("“{}”", shown(&name(item))),
        items => format!("{} items", items.len()),
    };
    let summary = match action {
        Action::Open => format!("{subject} couldn't be opened"),
        Action::NewFolder => "A new folder couldn't be made".to_string(),
        Action::MoveIntoNewFolder => format!("{subject} couldn't be moved into a new folder"),
        Action::Duplicate => format!("{subject} couldn't be duplicated"),
        Action::MakeAlias => format!("An alias of {subject} couldn't be made"),
        Action::ShowOriginal => format!("The original of {subject} can't be found"),
        Action::PutBack => format!("{subject} couldn't be put back"),
        Action::Tag => format!("The tags of {subject} couldn't be changed"),
        Action::Rename => format!("{subject} couldn't be renamed"),
        Action::Copy => "The text couldn't be copied".to_string(),
        Action::Paste => "The text couldn't be pasted".to_string(),
        Action::Drop(DropAction::Move) => format!("{subject} couldn't be moved"),
        Action::Drop(DropAction::Copy) => format!("{subject} couldn't be copied"),
        Action::Trash => format!("{subject} couldn't be moved to the Trash"),
    };
    Notice {
        summary,
        body: escape(&shown(&reason(error))),
        icon: "",
    }
}

/// The notice that a file operation ended, as `summary` ("Copied 3 items to “Documents”"), with the reason of its
/// first failure, if any, as its body.
pub fn finished(summary: &str, reason: Option<&str>) -> Notice {
    Notice {
        summary: shown(summary),
        body: escape(&shown(reason.unwrap_or_default())),
        icon: "",
    }
}

/// `error` as a person reads it in a window: a sentence without the system's error number, shown as text.
pub fn explained(error: &dyn Display) -> String {
    shown(&reason(error))
}

/// The notice for `folder`, a folder of the window's history such as "the previous folder", failing to open.
pub fn unopened(folder: &str, error: &dyn Display) -> Notice {
    Notice {
        summary: format!("{} couldn't be opened", sentence(folder)),
        body: escape(&shown(&reason(error))),
        icon: "",
    }
}

/// The notice for typed `text` that can't name an item, and why.
pub fn bad_name(text: &str, error: &NameError) -> Notice {
    let summary = match error {
        NameError::Empty => "A name can't be empty".to_string(),
        _ => format!("“{}” can't be used as a name", shown(text)),
    };
    let body = match error {
        NameError::Empty => "Type a name.".to_string(),
        NameError::ContainsSlash => "A name can't contain “/”.".to_string(),
        NameError::ContainsNul => "A name can't contain a null character.".to_string(),
        NameError::TooLong => format!("Use a name of {MAX_NAME_BYTES} bytes or fewer."),
        NameError::Dot => "“.” and “..” are reserved.".to_string(),
    };
    Notice {
        summary,
        body,
        icon: "",
    }
}

/// The name of `path` as a person knows it: its last component, else the whole path.
fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// `error` as a sentence: without the system's error number, which a person can't act on.
fn reason(error: &dyn Display) -> String {
    let text = error.to_string();
    let plain = match text.rfind(" (os error ") {
        Some(at) if text.ends_with(')') => &text[..at],
        _ => &text,
    };
    let mut out = sentence(plain);
    if !out.is_empty() && !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// `text` with its first letter capitalized.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    out.push_str(chars.as_str());
    out
}

/// A click on a notice posted with [`Notices::post_clickable`]: its tag, and the activation token the click carried, which
/// lets the window it shows come forward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Click {
    pub tag: u64,
    pub token: Option<String>,
}

/// What the posting future is asked.
enum Post {
    /// A notice, clickable when it has a tag.
    Notice(Notice, Option<u64>),
    /// Answer once every notice before this one was posted.
    Sent(async_channel::Sender<()>),
}

/// The notices of one program, posted in order by one future on its UI loop.
#[derive(Clone)]
pub struct Notices {
    sender: Option<async_channel::Sender<Post>>,
    clicks: Option<async_channel::Receiver<Click>>,
}

impl Notices {
    /// Notices that are logged, not posted: the session has no bus.
    pub fn logged() -> Notices {
        Notices {
            sender: None,
            clicks: None,
        }
    }

    /// Notices posted on `session` in order, and the future that posts them and hears their clicks, which the program
    /// runs on its UI loop. It touches no file and makes no blocking call; it ends once every [`Notices`] of it is
    /// dropped.
    pub fn on(session: Connection) -> (Notices, impl Future<Output = ()> + Send + 'static) {
        let (sender, posts) = async_channel::unbounded();
        let (clicked, clicks) = async_channel::unbounded();
        let notices = Notices {
            sender: Some(sender),
            clicks: Some(clicks),
        };
        (notices, post_all(session, posts, clicked))
    }

    pub fn post(&self, notice: Notice) {
        self.send(notice, None);
    }

    /// Post `notice` so that a click on it comes back from [`Notices::clicks`] as a [`Click`] with `tag`.
    pub fn post_clickable(&self, notice: Notice, tag: u64) {
        self.send(notice, Some(tag));
    }

    /// The clicks on the notices posted with a tag; none without a session bus.
    pub fn clicks(&self) -> Option<&async_channel::Receiver<Click>> {
        self.clicks.as_ref()
    }

    fn send(&self, notice: Notice, tag: Option<u64>) {
        match &self.sender {
            Some(sender) => {
                if let Err(e) = sender.try_send(Post::Notice(notice, tag)) {
                    if let Post::Notice(notice, _) = e.into_inner() {
                        log(&notice, &"the notices are no longer posted");
                    }
                }
            }
            None => log(&notice, &"no session bus"),
        }
    }

    /// Ends once every notice posted so far is sent: at once for notices that are logged.
    pub fn sent(&self) -> impl Future<Output = ()> + Send + 'static {
        let (done, wait) = async_channel::bounded(1);
        let asked = self
            .sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(Post::Sent(done)).is_ok());
        async move {
            if asked {
                let _ = wait.recv().await;
            }
        }
    }
}

/// What the posting future waits for next.
enum Next {
    Post(Option<Post>),
    Signal(Option<zbus::Result<Message>>),
}

/// Post every notice `posts` brings on `session`, in order, and hand each click on a clickable one to `clicked`: the
/// clicks are listened for from the first clickable notice on. Ends once `posts` has no sender left.
async fn post_all(
    session: Connection,
    posts: async_channel::Receiver<Post>,
    clicked: async_channel::Sender<Click>,
) {
    let mut signals: Option<MessageStream> = None;
    let mut unclickable = false;
    let mut tags = Tags::default();
    loop {
        let next = match signals.as_mut() {
            Some(stream) => {
                futures_lite::future::or(async { Next::Post(posts.recv().await.ok()) }, async {
                    Next::Signal(stream.next().await)
                })
                .await
            }
            None => Next::Post(posts.recv().await.ok()),
        };
        match next {
            Next::Post(None) => return,
            Next::Post(Some(Post::Sent(done))) => {
                let _ = done.try_send(());
            }
            Next::Post(Some(Post::Notice(notice, tag))) => {
                if tag.is_some() && signals.is_none() && !unclickable {
                    match listen(&session).await {
                        Ok(stream) => signals = Some(stream),
                        Err(e) => {
                            eprintln!("cupertino-files: notices can't be clicked: {e}");
                            unclickable = true;
                        }
                    }
                }
                let tag = tag.filter(|_| signals.is_some());
                match notify(&session, &notice, tag.is_some()).await {
                    Ok(id) => {
                        if let Some(tag) = tag {
                            tags.posted(id, tag);
                        }
                    }
                    Err(e) => log(&notice, &e),
                }
            }
            Next::Signal(Some(Ok(message))) => {
                if let Some(click) = tags.heard(&message) {
                    if clicked.try_send(click).is_err() {
                        signals = None;
                        unclickable = true;
                    }
                }
            }
            Next::Signal(_) => signals = None,
        }
    }
}

/// The clickable notices posted, by the notification service's id, and the tokens their clicks carried.
#[derive(Default)]
struct Tags {
    tags: BTreeMap<u32, u64>,
    tokens: HashMap<u32, String>,
}

impl Tags {
    /// The notice the service numbered `id` was posted with `tag`.
    fn posted(&mut self, id: u32, tag: u64) {
        self.tags.insert(id, tag);
        while self.tags.len() > MAX_CLICKABLE {
            if let Some((old, _)) = self.tags.pop_first() {
                self.tokens.remove(&old);
            }
        }
    }

    /// What the notification service's signal `message` says of the clickable notices: the click on one, if it is
    /// one.
    fn heard(&mut self, message: &Message) -> Option<Click> {
        let header = message.header();
        let member = header.member().map(|m| m.as_str()).unwrap_or_default();
        match member {
            "ActivationToken" => {
                if let Ok((id, token)) = message.body().deserialize::<(u32, String)>() {
                    if self.tags.contains_key(&id) {
                        self.tokens.insert(id, token);
                    }
                }
                None
            }
            "ActionInvoked" => {
                let (id, _key) = message.body().deserialize::<(u32, String)>().ok()?;
                let tag = *self.tags.get(&id)?;
                let token = self.tokens.remove(&id);
                Some(Click { tag, token })
            }
            "NotificationClosed" => {
                if let Ok((id, _reason)) = message.body().deserialize::<(u32, u32)>() {
                    self.tags.remove(&id);
                    self.tokens.remove(&id);
                }
                None
            }
            _ => None,
        }
    }
}

/// The notification service's signals on `session`.
async fn listen(session: &Connection) -> zbus::Result<MessageStream> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(NOTIFICATIONS)?
        .interface(NOTIFICATIONS)?
        .path(NOTIFICATIONS_PATH)?
        .build();
    MessageStream::for_match_rule(rule, session, None).await
}

/// Post `notice`, with a default action when it is `clickable`; the service's id of it.
async fn notify(session: &Connection, notice: &Notice, clickable: bool) -> zbus::Result<u32> {
    let actions: Vec<&str> = if clickable {
        vec![DEFAULT_ACTION, DEFAULT_LABEL]
    } else {
        Vec::new()
    };
    let hints = HashMap::from([
        ("transient", Value::Bool(true)),
        ("desktop-entry", Value::from(DESKTOP_ENTRY)),
    ]);
    // -1: the notification service decides how long the banner stays
    let reply = session
        .call_method(
            Some(NOTIFICATIONS),
            NOTIFICATIONS_PATH,
            Some(NOTIFICATIONS),
            "Notify",
            &(
                APP_NAME,
                0u32,
                notice.icon,
                notice.summary.as_str(),
                notice.body.as_str(),
                actions,
                hints,
                -1i32,
            ),
        )
        .await?;
    reply.body().deserialize::<u32>()
}

fn log(notice: &Notice, why: &dyn Display) {
    eprintln!(
        "cupertino-files: {} ({}; not posted: {why})",
        notice.summary, notice.body
    );
}
