//! The notifications the lock screen shows: what the notification daemon lists for it, and where the cards sit.
//!
//! The daemon (`cupertino-notifications`) decides what may show: while the session is locked, its
//! `org.cupertino.NotificationCenter1.LockScreenNotifications` lists the notifications that arrived since the lock
//! from applications allowed on the lock screen, each as its application, a title and a body. The title is
//! "Notification" and the body empty unless the application's previews are always shown, so the rest of a
//! notification never reaches this process, its windows or the tree assistive technology reads. The daemon says when
//! the list changed with the signal `LockScreenChanged`, which carries nothing; [`watch`] then asks for the list
//! again, and again when the daemon's bus name changes owner. Each text is cut again here to its cap, whatever the
//! daemon sent.
//!
//! Any process of the user could own the daemon's bus name while the daemon is not running, so the name alone is not
//! trusted: [`fetch`] asks the bus for the name's owner, reads the owner's process from the credentials the bus gives
//! and asks that very connection only when the process is unsandboxed and runs one of the daemon's programs
//! ([`daemon_programs`]): `cupertino-notifications` from this program's own directory, or found on `PATH` the way the
//! server finds the shell processes it starts, when only root can change that file and the directories above it (so a
//! program the user put in a `PATH` directory of their own does not count). Any other owner gets no question and the list
//! is empty, so no other program can put words on the lock screen.

use cupertino_geometry::Rect;

/// One notification on the lock screen, as the daemon gave it.
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

/// The file name of the daemon's program.
pub const DAEMON: &str = "cupertino-notifications";

/// The program the daemon runs as `/proc` names it when it is installed beside this one: [`DAEMON`] in the directory of
/// `own`, this program's path. A program replaced on disk while it runs is named with " (deleted)" after its path; that
/// mark is not part of the directory or the name compared.
pub fn daemon_program(own: &std::path::Path) -> std::path::PathBuf {
    own.with_file_name(DAEMON)
}

/// Every program the daemon may run, as `/proc` names a program (links resolved): [`daemon_program`] of `own`, then
/// [`DAEMON`] in each directory of `path` (the `PATH` variable, searched as the server searches it for the shell
/// processes it starts) where the file exists and `trusted` accepts it. The lock screen trusts only a file that root
/// alone can change ([`cupertino_defaults::system::only_root_can_change`]).
pub fn daemon_programs(
    own: &std::path::Path,
    path: Option<&std::ffi::OsStr>,
    trusted: impl Fn(&std::path::Path) -> bool,
) -> Vec<std::path::PathBuf> {
    let mut programs = vec![daemon_program(own)];
    for dir in path.map(std::env::split_paths).into_iter().flatten() {
        // a relative entry names a directory of whoever starts the lock screen: not looked in
        if !dir.is_absolute() {
            continue;
        }
        let Ok(program) = std::fs::canonicalize(dir.join(DAEMON)) else {
            continue;
        };
        if program.is_file() && trusted(&program) && !programs.contains(&program) {
            programs.push(program);
        }
    }
    programs
}

/// `exe` as `/proc/<pid>/exe` names it, without the mark of a program replaced on disk since it started.
pub fn live_path(exe: &std::path::Path) -> std::path::PathBuf {
    let text = exe.as_os_str().to_string_lossy();
    match text.strip_suffix(" (deleted)") {
        Some(path) => std::path::PathBuf::from(path),
        None => exe.to_path_buf(),
    }
}

#[cfg(feature = "dbus")]
pub use bus::{fetch, fetch_served_by, watch};

#[cfg(feature = "dbus")]
mod bus {
    use super::{daemon_programs, from_rows, live_path, Notice};
    use cupertino_peer::signal::{from_owner, sent_by_bus};
    use cupertino_peer::Caller;
    use std::path::Path;
    use zbus::blocking::fdo::DBusProxy;
    use zbus::blocking::{Connection, MessageIterator};
    use zbus::message::Type;
    use zbus::names::BusName;
    use zbus::MatchRule;

    const BUS_NAME: &str = "org.cupertino.NotificationCenter1";
    const OBJECT_PATH: &str = "/org/cupertino/NotificationCenter1";
    const INTERFACE: &str = "org.cupertino.NotificationCenter1";

    /// The list the daemon gives the lock screen now, asked only of the real daemon ([`fetch_served_by`] with
    /// [`daemon_programs`] of this program and `PATH`). No daemon on the bus is an empty list.
    pub fn fetch(session: &Connection) -> zbus::Result<Vec<Notice>> {
        let own = std::env::current_exe()
            .map_err(|e| zbus::Error::Failure(format!("this program's path: {e}")))?;
        let programs = daemon_programs(
            &live_path(&own),
            std::env::var_os("PATH").as_deref(),
            cupertino_defaults::system::only_root_can_change,
        );
        fetch_served_by(session, &programs)
    }

    /// The list, asked of the owner of the daemon's name only when it is an unsandboxed process running one of
    /// `programs`; any other owner is an error and is not asked. No owner is an empty list.
    pub fn fetch_served_by(
        session: &Connection,
        programs: &[std::path::PathBuf],
    ) -> zbus::Result<Vec<Notice>> {
        let dbus = DBusProxy::new(session)?;
        let name = BusName::try_from(BUS_NAME)?;
        let owner = match dbus.get_name_owner(name) {
            Ok(owner) => owner,
            Err(zbus::fdo::Error::NameHasNoOwner(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let credentials =
            dbus.get_connection_credentials(BusName::from(owner.clone().into_inner()))?;
        let runs_program = match Caller::from_credentials(&credentials, Path::new("/proc")) {
            Caller::Process(process) => {
                process.sandbox.is_none()
                    && process
                        .exe
                        .as_deref()
                        .map(live_path)
                        .is_some_and(|exe| programs.contains(&exe))
            }
            Caller::Anonymous | Caller::Unclassified(_) => false,
        };
        if !runs_program {
            return Err(zbus::Error::Failure(format!(
                "{BUS_NAME} is owned by a process that runs none of {programs:?}"
            )));
        }
        // the owner's own connection, so a new owner of the name cannot answer in its place
        let reply = session.call_method(
            Some(owner.as_str()),
            OBJECT_PATH,
            Some(INTERFACE),
            "LockScreenNotifications",
            &(),
        );
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

    /// Pass the list to `on_list` now, at every `LockScreenChanged` from the owner of the daemon's name and when the name
    /// changes owner. A list that cannot be read is reported and passed on as empty, so nothing stale stays shown.
    ///
    /// The readers of the signals never wait on the bus: a signal only asks a thread of its own for a read, with at most
    /// one asked for while one runs. A stream that fills stops the connection, the replies to the read included, and the
    /// bus hands this process any signal another peer sends it; a forged `LockScreenChanged` is dropped as it arrives
    /// (`from_owner`), and a flood of owner changes costs one read at a time.
    pub fn watch(
        session: &Connection,
        on_list: impl Fn(Vec<Notice>) + Send + Sync + 'static,
    ) -> zbus::Result<()> {
        let changed = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(BUS_NAME)?
            .interface(INTERFACE)?
            .member("LockScreenChanged")?
            .path(OBJECT_PATH)?
            .build();
        let owner = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender("org.freedesktop.DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .add_arg(BUS_NAME)?
            .build();
        let changes = from_owner(session, BUS_NAME, changed, |_| {
            eprintln!("cupertino-login: a LockScreenChanged not sent by the notification daemon was ignored")
        })?;
        let owners = MessageIterator::for_match_rule(owner, session, None)?.filter(|message| {
            // zbus filters the rule's sender already; this does not rely on it
            !matches!(message, Ok(message) if !sent_by_bus(message))
        });
        let streams: [Box<dyn Iterator<Item = zbus::Result<zbus::Message>> + Send>; 2] =
            [Box::new(changes), Box::new(owners)];
        let report = std::sync::Arc::new(move |session: &Connection| {
            on_list(fetch(session).unwrap_or_else(|e| {
                eprintln!("cupertino-login: the lock screen's notifications: {e}");
                Vec::new()
            }))
        });
        let (again, asked) = std::sync::mpsc::sync_channel::<()>(1);
        let reader = session.clone();
        std::thread::Builder::new()
            .name("lock-notices-read".into())
            .spawn(move || asked.into_iter().for_each(|()| report(&reader)))
            .map_err(|e| zbus::Error::Failure(format!("starting the notifications reader: {e}")))?;
        for messages in streams {
            let again = again.clone();
            std::thread::Builder::new()
                .name("lock-notices".into())
                .spawn(move || {
                    for message in messages {
                        if message.is_err() {
                            break;
                        }
                        let _ = again.try_send(());
                    }
                    eprintln!(
                        "cupertino-login: the lock screen's notifications are no longer followed"
                    );
                })
                .map_err(|e| {
                    zbus::Error::Failure(format!("starting the notifications reader: {e}"))
                })?;
        }
        // the first read, on the same thread as the later ones so the lists arrive in order
        let _ = again.try_send(());
        Ok(())
    }
}
