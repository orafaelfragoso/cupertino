//! Failures a person caused, reported as notifications on the session bus (feature `app`). A [`Notice`] is worded by
//! the action that failed ([`failure`]) or by the removable media calls; [`Notices`] posts each from one thread, in
//! order, so the window never waits on the bus. A notice that can't be posted is logged instead, once. A notice posted
//! with a tag ([`Notices::post_clickable`]) can be clicked: the click comes back as a [`Click`] ([`Notices::clicks`]).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::zvariant::Value;
use zbus::MatchRule;

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

/// What the notices' thread is asked.
enum Post {
    /// A notice, clickable when it has a tag.
    Notice(Notice, Option<u64>),
    /// Answer once every notice before this one was posted.
    Flush(mpsc::SyncSender<()>),
}

/// The notices of one process, posted in order on a thread of their own.
#[derive(Clone)]
pub struct Notices {
    sender: Option<Sender<Post>>,
    clicks: Option<async_channel::Receiver<Click>>,
}

impl Notices {
    /// Post on `session`, or log every notice when there is no session bus.
    pub fn start(session: Option<Connection>) -> Notices {
        let Some(session) = session else {
            return Notices {
                sender: None,
                clicks: None,
            };
        };
        let (sender, posts) = mpsc::channel::<Post>();
        let (clicked, clicks) = async_channel::unbounded();
        let spawned = std::thread::Builder::new()
            .name("notices".into())
            .spawn(move || {
                // the clicks are listened for from the first clickable notice on
                let mut clickable: Option<Clickable> = None;
                for post in posts {
                    match post {
                        Post::Notice(notice, tag) => {
                            let listening = tag.and_then(|tag| {
                                if clickable.is_none() {
                                    clickable = Clickable::listen(&session, clicked.clone())
                                        .map_err(|e| {
                                            eprintln!(
                                                "cupertino-files: notices can't be clicked: {e}"
                                            )
                                        })
                                        .ok();
                                }
                                clickable.as_ref().map(|c| (c, tag))
                            });
                            match notify(&session, &notice, listening.is_some()) {
                                Ok(id) => {
                                    if let Some((clickable, tag)) = listening {
                                        clickable.posted(id, tag);
                                    }
                                }
                                Err(e) => log(&notice, &e),
                            }
                        }
                        Post::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            });
        match spawned {
            Ok(_) => Notices {
                sender: Some(sender),
                clicks: Some(clicks),
            },
            Err(e) => {
                eprintln!("cupertino-files: notices will be logged, not posted: {e}");
                Notices {
                    sender: None,
                    clicks: None,
                }
            }
        }
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
                if let Err(mpsc::SendError(Post::Notice(notice, _))) =
                    sender.send(Post::Notice(notice, tag))
                {
                    log(&notice, &"the notice thread ended");
                }
            }
            None => log(&notice, &"no session bus"),
        }
    }

    /// Wait up to `timeout` until every notice posted so far has been sent: the process is about to exit.
    pub fn flush(&self, timeout: std::time::Duration) {
        let Some(sender) = &self.sender else {
            return;
        };
        let (done, sent) = mpsc::sync_channel(1);
        if sender.send(Post::Flush(done)).is_ok() {
            let _ = sent.recv_timeout(timeout);
        }
    }
}

/// The clickable notices posted, by the notification service's id, and the tokens their clicks carried.
#[derive(Default)]
struct Tags {
    tags: BTreeMap<u32, u64>,
    tokens: HashMap<u32, String>,
}

/// The clicks of the clickable notices, heard on a thread of their own.
struct Clickable {
    tags: Arc<Mutex<Tags>>,
}

impl Clickable {
    /// Listen on `session` to the notification service's signals, handing each click on a clickable notice to `clicked`.
    fn listen(
        session: &Connection,
        clicked: async_channel::Sender<Click>,
    ) -> zbus::Result<Clickable> {
        let signals = MessageIterator::for_match_rule(
            MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(NOTIFICATIONS)?
                .interface(NOTIFICATIONS)?
                .path(NOTIFICATIONS_PATH)?
                .build(),
            session,
            None,
        )?;
        let tags = Arc::new(Mutex::new(Tags::default()));
        let heard = Arc::clone(&tags);
        std::thread::Builder::new()
            .name("notice-clicks".into())
            .spawn(move || {
                for message in signals {
                    let Ok(message) = message else { break };
                    let header = message.header();
                    let member = header.member().map(|m| m.as_str()).unwrap_or_default();
                    let mut tags = heard.lock().unwrap_or_else(PoisonError::into_inner);
                    match member {
                        "ActivationToken" => {
                            if let Ok((id, token)) = message.body().deserialize::<(u32, String)>() {
                                if tags.tags.contains_key(&id) {
                                    tags.tokens.insert(id, token);
                                }
                            }
                        }
                        "ActionInvoked" => {
                            let Ok((id, _key)) = message.body().deserialize::<(u32, String)>()
                            else {
                                continue;
                            };
                            if let Some(&tag) = tags.tags.get(&id) {
                                let token = tags.tokens.remove(&id);
                                if clicked.send_blocking(Click { tag, token }).is_err() {
                                    break;
                                }
                            }
                        }
                        "NotificationClosed" => {
                            if let Ok((id, _reason)) = message.body().deserialize::<(u32, u32)>() {
                                tags.tags.remove(&id);
                                tags.tokens.remove(&id);
                            }
                        }
                        _ => {}
                    }
                }
            })
            .map_err(|e| zbus::Error::Failure(e.to_string()))?;
        Ok(Clickable { tags })
    }

    /// The notice the service numbered `id` was posted with `tag`.
    fn posted(&self, id: u32, tag: u64) {
        let mut tags = self.tags.lock().unwrap_or_else(PoisonError::into_inner);
        tags.tags.insert(id, tag);
        while tags.tags.len() > MAX_CLICKABLE {
            if let Some((old, _)) = tags.tags.pop_first() {
                tags.tokens.remove(&old);
            }
        }
    }
}

/// Post `notice`, with a default action when it is `clickable`; the service's id of it.
fn notify(session: &Connection, notice: &Notice, clickable: bool) -> zbus::Result<u32> {
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
    let reply = session.call_method(
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
    )?;
    reply.body().deserialize::<u32>()
}

fn log(notice: &Notice, why: &dyn Display) {
    eprintln!(
        "cupertino-files: {} ({}; not posted: {why})",
        notice.summary, notice.body
    );
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn one_item_is_named_several_are_counted_and_the_error_is_a_sentence() {
        let denied = io::Error::from_raw_os_error(13);
        assert_eq!(
            failure(Action::Rename, &[Path::new("/home/ada/Notes.txt")], &denied),
            Notice {
                summary: "“Notes.txt” couldn't be renamed".into(),
                body: "Permission denied.".into(),
                icon: "",
            }
        );
        let two = [Path::new("/a/x"), Path::new("/a/y")];
        assert_eq!(
            failure(Action::Trash, &two, &denied).summary,
            "2 items couldn't be moved to the Trash"
        );
        assert_eq!(
            failure(Action::NewFolder, &[], &denied).summary,
            "A new folder couldn't be made"
        );
        assert_eq!(
            failure(Action::Open, &[Path::new("/")], &"no application opens it").summary,
            "“/” couldn't be opened"
        );
    }

    #[test]
    fn a_file_name_and_a_message_are_shown_as_text_never_as_markup() {
        let notice = failure(
            Action::Duplicate,
            &[Path::new("/tmp/\u{202e}txt.exe")],
            &"<a href=\"file:///etc\">here</a> & there",
        );
        assert_eq!(notice.summary, "“txt.exe” couldn't be duplicated");
        assert_eq!(
            notice.body,
            "&lt;a href=\"file:///etc\"&gt;here&lt;/a&gt; &amp; there."
        );
    }

    #[test]
    fn text_loses_what_would_disguise_it_and_is_cut_at_256_characters() {
        assert_eq!(shown("\u{202e}Spoo\u{061c}f\ned\u{2069}"), "Spoofed");
        let long = shown(&"x".repeat(300));
        assert_eq!(long.chars().count(), 256);
        assert!(long.ends_with("x…"));
        assert_eq!(shown(&"x".repeat(256)), "x".repeat(256));
    }
}
