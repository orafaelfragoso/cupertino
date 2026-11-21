//! The dock's work off the UI thread: what it reads at start on Kit's I/O pool ([`startup`]), the watches on
//! `dock.conf` and on the home trash, and the applications' badges heard on the session connection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cupertino_defaults::dock::DockSettings;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_kit::core::Command;
use cupertino_kit::task;
use futures_util::StreamExt;

use super::{Handed, Heard, Kept, PROCESS};
use crate::conf::{default_pins, load_pins, session_file_manager, TERMINALS};
use crate::desktop::DesktopIndex;
use crate::kit::Message;

/// How long reading what the dock needs at start should take: the desktop entries of every data directory, a few
/// settings files and a look along `PATH`.
const STARTUP_DEADLINE: Duration = Duration::from_secs(5);
/// How long reading a small file (`dock.conf`, the launcher's history) should take.
pub const CONF_DEADLINE: Duration = Duration::from_secs(2);
/// The default pins after the built-in file manager, when neither the stored pins nor `dock.conf` name any.
const DEFAULT_PINS: usize = 4;

/// What the dock reads at start, and the watches that follow the files after it.
pub struct Startup {
    pub(super) read: Kept,
    /// The terminal emulator that runs entries with `Terminal=true`: the first of [`TERMINALS`] installed.
    pub(super) terminal: Option<String>,
    pub(super) folders: Vec<(String, String)>,
    pub(super) home: Option<String>,
    pub(super) trash: Arc<cupertino_trash::Look>,
    pub(super) conf_watch: Option<ConfigWatcher>,
    pub(super) trash_watch: Option<notify::RecommendedWatcher>,
}

impl Kept {
    /// What the model starts with when nothing was read yet: no entries, no settings and no pins.
    pub(super) fn unread() -> Self {
        Self {
            desktop: DesktopIndex::default(),
            conf: None,
            pins: Vec::new(),
            file_manager: None,
            icons: HashMap::new(),
            badges: crate::badge::Badges::default(),
            trash_full: false,
        }
    }
}

/// Where the dock stores its pins.
pub fn pins_path() -> Option<PathBuf> {
    cupertino_defaults::config_file("CUPERTINO_DOCK_PINS", "dock-pins")
}

/// `dock.conf` at `path` (nothing set when it does not exist). Lines that are not understood are reported; with
/// `last_good` their keys keep its values. `None`, reported, when it cannot be read.
pub fn load_conf(path: &Path, last_good: Option<&DockSettings>) -> Option<DockSettings> {
    match DockSettings::load(path, last_good) {
        Ok((settings, problems)) => {
            for problem in problems {
                eprintln!(
                    "{PROCESS}: {} line {}: {}",
                    path.display(),
                    problem.line,
                    problem.message
                );
            }
            Some(settings)
        }
        Err(e) => {
            eprintln!("{PROCESS}: cannot read {}: {e}", path.display());
            None
        }
    }
}

/// Whether `program` is an executable file on `PATH` (or at the path it names).
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn installed(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The user's downloads folder (`XDG_DOWNLOAD_DIR` of `user-dirs.dirs`, else `Downloads` in `home`), when it is
/// there, with its icon's theme name.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn folders(home: &Path) -> Vec<(String, String)> {
    let assigned = cupertino_defaults::user_dirs::read(home)
        .unwrap_or_default()
        .into_iter()
        .find_map(|(name, path)| (name == "DOWNLOAD" && path != home).then_some(path));
    let downloads = assigned.unwrap_or_else(|| home.join("Downloads"));
    if !downloads.is_dir() {
        return Vec::new();
    }
    vec![(
        downloads.to_string_lossy().into_owned(),
        crate::stack::DOWNLOADS_ICON.to_owned(),
    )]
}

/// The pins: those the dock stored (a drag, Keep in Dock), which win over those of `dock.conf`, which the dock never
/// rewrites, else the defaults.
fn pins(desktop: &DesktopIndex, conf: Option<&DockSettings>) -> Vec<String> {
    let stored = pins_path().and_then(|path| match load_pins(&path) {
        Ok(pins) => pins,
        Err(e) => {
            eprintln!("{PROCESS}: cannot read {}: {e}", path.display());
            None
        }
    });
    stored
        .or_else(|| conf.map(|f| f.pinned.clone()).filter(|p| !p.is_empty()))
        .unwrap_or_else(|| default_pins(desktop, TERMINALS, DEFAULT_PINS))
}

/// Read what the dock needs at start; the watches tell their changes with `tell`.
fn read(tell: async_channel::Sender<Message>) -> Startup {
    let desktop = DesktopIndex::load_from(&cupertino_defaults::data_dirs());
    let conf_path = DockSettings::path();
    let conf = conf_path.as_deref().and_then(|path| load_conf(path, None));
    let conf_watch = conf_path.and_then(|path| match ConfigWatcher::new(vec![path.clone()]) {
        Ok(watch) => Some(watch),
        Err(e) => {
            eprintln!(
                "{PROCESS}: cannot watch {}: {e}; changes apply at the next start",
                path.display()
            );
            None
        }
    });
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    Startup {
        read: Kept {
            pins: pins(&desktop, conf.as_ref()),
            file_manager: session_file_manager(&desktop),
            desktop,
            conf,
            icons: HashMap::new(),
            badges: crate::badge::Badges::default(),
            trash_full: false,
        },
        terminal: TERMINALS
            .iter()
            .find(|t| installed(t))
            .map(|t| (*t).to_owned()),
        folders: home.as_deref().map(folders).unwrap_or_default(),
        home: home.map(|h| h.to_string_lossy().into_owned()),
        trash: cupertino_trash::Look::new(cupertino_trash::Trash::new()),
        conf_watch,
        trash_watch: watch_trash(tell),
    }
}

/// Read what the dock needs at start on the I/O pool: it arrives as [`Heard::Loaded`].
pub fn startup(tell: async_channel::Sender<Message>) -> Command<Message> {
    task::io(
        STARTUP_DEADLINE,
        move || read(tell),
        |startup| Message::Shell(Heard::Loaded(Handed::new(startup))),
    )
}

/// Follow `dock.conf` as `watch` watches it: a change is told as [`Message::ConfChanged`]. A watch that fails is
/// reported: the file then applies at the next start.
pub fn follow_conf(watch: ConfigWatcher, tell: async_channel::Sender<Message>) -> Command<Message> {
    Command::Perform(Box::pin(task::follow_settings(watch, move |event| {
        match event {
            WatchEvent::Changed(_) => {
                // the receiver goes only with the component
                let _ = tell.try_send(Message::ConfChanged);
            }
            WatchEvent::Error(e) => eprintln!("{PROCESS}: watching dock.conf failed: {e}"),
        }
    })))
}

/// Watch the home trash's `files` folder (made when missing); an entry put there or taken away is told as
/// [`Message::TrashChanged`], so the trash's icon follows at once. A watch that cannot be set up leaves the
/// once-a-second look the model takes anyway.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn watch_trash(tell: async_channel::Sender<Message>) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let files = cupertino_trash::Trash::new().home_trash().join("files");
    if let Err(e) = std::fs::create_dir_all(&files) {
        eprintln!("{PROCESS}: cannot make {}: {e}", files.display());
        return None;
    }
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|e| !e.kind.is_access()) {
            let _ = tell.try_send(Message::TrashChanged);
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{PROCESS}: cannot watch the trash: {e}");
            return None;
        }
    };
    match watcher.watch(&files, notify::RecursiveMode::NonRecursive) {
        Ok(()) => Some(watcher),
        Err(e) => {
            eprintln!("{PROCESS}: cannot watch {}: {e}", files.display());
            None
        }
    }
}

/// The launcher's usage store, ranked now, the most used first.
pub fn recent_apps() -> Vec<String> {
    let Some(path) = cupertino_apps::frecency::default_path() else {
        return Vec::new();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    cupertino_apps::frecency::ranked_apps(&cupertino_apps::frecency::Store::read(&path), now)
        .into_iter()
        .map(|app| app.id)
        .collect()
}

/// A number of a badge's property.
fn number(v: &zbus::zvariant::Value<'_>) -> Option<i64> {
    use zbus::zvariant::Value;
    match *v {
        Value::I64(n) => Some(n),
        Value::I32(n) => Some(i64::from(n)),
        Value::U32(n) => Some(i64::from(n)),
        Value::U64(n) => i64::try_from(n).ok(),
        Value::I16(n) => Some(i64::from(n)),
        Value::U16(n) => Some(i64::from(n)),
        Value::U8(n) => Some(i64::from(n)),
        _ => None,
    }
}

/// Hear the launcher entry signals with which applications set the counts on their badges ([`crate::badge`]).
async fn hear_badges(
    connection: zbus::Connection,
    tell: async_channel::Sender<Message>,
) -> zbus::Result<()> {
    use crate::badge::{update, LAUNCHER_ENTRY_INTERFACE, LAUNCHER_ENTRY_UPDATE};
    use zbus::message::Type;
    use zbus::zvariant::{OwnedValue, Value};
    let rule = zbus::MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(LAUNCHER_ENTRY_INTERFACE)?
        .member(LAUNCHER_ENTRY_UPDATE)?
        .build();
    let mut signals = zbus::MessageStream::for_match_rule(rule, &connection, None).await?;
    while let Some(message) = signals.next().await {
        let Ok(message) = message else { break };
        let Ok((uri, properties)) = message
            .body()
            .deserialize::<(String, HashMap<String, OwnedValue>)>()
        else {
            continue;
        };
        let count = properties.get("count").and_then(|v| number(v));
        let visible = properties.get("count-visible").and_then(|v| match **v {
            Value::Bool(b) => Some(b),
            _ => None,
        });
        if let Some(update) = update(&uri, count, visible) {
            if tell.send(Message::Badge(update)).await.is_err() {
                break;
            }
        }
    }
    Ok(())
}

/// Listen on the component's session connection for the applications' badges, which are told as
/// [`Message::Badge`]. Without the signals the tiles show no badges.
pub fn listen_badges(
    connection: zbus::Connection,
    tell: async_channel::Sender<Message>,
) -> Command<Message> {
    Command::Perform(Box::pin(async move {
        if let Err(e) = hear_badges(connection, tell).await {
            eprintln!("{PROCESS}: cannot hear the applications' badges: {e}");
        }
        Message::Done
    }))
}
