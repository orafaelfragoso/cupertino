//! The file the notification list is kept in across a restart of the component (D21):
//! `$XDG_RUNTIME_DIR/cupertino/notifications-<display>.json`, mode 0600 in a directory of mode 0700, for the session
//! on the Wayland display `<display>`. The runtime directory is the person's own and is emptied when they log out, so
//! the list never outlives the login. The component writes the file whole as the list changes and reads it as it
//! starts; both run on the I/O pool, never on the UI thread.
//!
//! The file holds what the list shows and what the bus says of it: each notification but the transient ones, which
//! never enter the list, its read state and whether its close was announced, with its time on the wall clock; the id
//! the next notification takes; do not disturb. An image a notification sent as pixels (`image-data`) is not written:
//! after a restart its card shows the application's icon instead.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::center::{DndMode, Kept, NotificationCenter};
use crate::markup;
use crate::notification::{Action, Expire, Notification, Urgency};

/// The version of the file's layout; a file of another version is not read.
const VERSION: u32 = 1;
/// The name of the file without its display's part.
const STEM: &str = "notifications";
/// The access of the file: the person alone reads and writes it.
const FILE_MODE: u32 = 0o600;
/// The access of its directory.
const DIR_MODE: u32 = 0o700;

/// Where the list is kept: `None` without `XDG_RUNTIME_DIR`, and then it is not kept.
pub fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    // a display named by its path keeps its last part; any other character than these is left out of the name
    let display: String = Path::new(&display)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let name = if display.is_empty() {
        format!("{STEM}.json")
    } else {
        format!("{STEM}-{display}.json")
    };
    Some(PathBuf::from(dir).join("cupertino").join(name))
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    next_id: u32,
    dnd: Dnd,
    notifications: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum Dnd {
    Off,
    On,
    /// On until this time on the wall clock, in seconds since the Unix epoch.
    Until {
        unix: f64,
    },
}

#[derive(Serialize, Deserialize)]
struct Record {
    id: u32,
    app_name: String,
    icon: String,
    summary: String,
    body: String,
    actions: Vec<(String, String)>,
    /// `expire_timeout` as `Notify` gave it.
    expire: i32,
    urgency: u8,
    category: Option<String>,
    resident: bool,
    image_path: Option<String>,
    desktop_entry: Option<String>,
    /// When it arrived, in seconds since the Unix epoch.
    unix: f64,
    revision: u32,
    read: bool,
    closed_signalled: bool,
    lock_screen: bool,
}

impl Record {
    fn of(kept: &Kept, offset: f64) -> Record {
        let n = &kept.notification;
        Record {
            id: n.id,
            app_name: n.app_name.clone(),
            icon: n.icon.clone(),
            summary: n.summary.clone(),
            body: n.body.clone(),
            actions: n
                .actions
                .iter()
                .map(|a| (a.key.clone(), a.label.clone()))
                .collect(),
            expire: match n.expire {
                Expire::Default => -1,
                Expire::Never => 0,
                Expire::Millis(ms) => i32::try_from(ms).unwrap_or(i32::MAX),
            },
            urgency: n.urgency as u8,
            category: n.category.clone(),
            resident: n.resident,
            image_path: n.image_path.clone(),
            desktop_entry: n.desktop_entry.clone(),
            unix: n.timestamp + offset,
            revision: n.revision,
            read: kept.read,
            closed_signalled: kept.closed_signalled,
            lock_screen: kept.lock_screen,
        }
    }

    fn kept(self, offset: f64) -> Kept {
        Kept {
            notification: Notification {
                id: self.id,
                spans: markup::parse(&self.body),
                app_name: self.app_name,
                icon: self.icon,
                summary: self.summary,
                body: self.body,
                actions: self
                    .actions
                    .into_iter()
                    .map(|(key, label)| Action { key, label })
                    .collect(),
                expire: Expire::from_dbus(self.expire),
                urgency: Urgency::from_byte(self.urgency),
                category: self.category,
                transient: false,
                resident: self.resident,
                image: None,
                image_path: self.image_path,
                desktop_entry: self.desktop_entry,
                timestamp: self.unix - offset,
                revision: self.revision,
            },
            read: self.read,
            closed_signalled: self.closed_signalled,
            lock_screen: self.lock_screen,
        }
    }
}

/// What a file gives back.
#[derive(Clone, Debug, PartialEq)]
pub struct Restored {
    pub kept: Vec<Kept>,
    pub next_id: u32,
    pub dnd: DndMode,
}

/// The file's content for `center`. `offset` turns the model's clock into the wall clock: seconds since the Unix
/// epoch at the model's 0, taken once, so the same list always gives the same bytes.
pub fn encode(center: &NotificationCenter, offset: f64) -> Vec<u8> {
    let file = File {
        version: VERSION,
        next_id: center.next_id(),
        dnd: match center.dnd() {
            DndMode::Off => Dnd::Off,
            DndMode::On => Dnd::On,
            DndMode::Until(t) => Dnd::Until { unix: t + offset },
        },
        notifications: center
            .kept()
            .iter()
            .map(|kept| Record::of(kept, offset))
            .collect(),
    };
    serde_json::to_vec(&file).unwrap_or_default()
}

/// What `bytes` keep, on a model whose clock is at 0 `offset` seconds after the Unix epoch; an error for a file
/// that is not one this module wrote.
pub fn decode(bytes: &[u8], offset: f64) -> Result<Restored, String> {
    let file: File = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!("version {} is not {VERSION}", file.version));
    }
    Ok(Restored {
        kept: file
            .notifications
            .into_iter()
            .map(|record| record.kept(offset))
            .collect(),
        next_id: file.next_id,
        dnd: match file.dnd {
            Dnd::Off => DndMode::Off,
            Dnd::On => DndMode::On,
            Dnd::Until { unix } => DndMode::Until(unix - offset),
        },
    })
}

/// The file at `path`; `None` when there is none. For the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
pub fn read(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes `bytes` as the file at `path`, whole: to a file of this write's own beside it, then renamed over it, so a
/// reader finds the old file or the new one. For the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the file has no directory"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)?;
    // a write of an instance that failed may still run beside the next one's, in the same host process
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(
        ".{}-{}.new",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let new = dir.join(name);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(&new)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_data()
        })
        .and_then(|()| std::fs::rename(&new, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    written
}
