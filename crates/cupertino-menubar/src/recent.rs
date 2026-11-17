//! What Recent Items of the system menu lists: the applications in the launcher's order (its usage store, read with
//! `cupertino_apps::frecency`) and the documents of the session's recently-used list (`cupertino_apps::recent`), at
//! most [`RECENT_LIMIT`] of each. The sources are read when the system menu opens. Clear Menu stores the time it was
//! chosen in a file of the bar's own; what was last used at or before that time is not listed. The launcher's and
//! the applications' files are only read.

use cupertino_apps::entry::index_dirs;
use cupertino_apps::frecency::{self, Store};
use cupertino_apps::recent::{read_recent_files, xbel_path};
use cupertino_apps::{DesktopEntry, Locale, Visibility, XdgDirs};
use std::io;
use std::path::PathBuf;

/// The most applications and the most documents listed. approximate.
pub const RECENT_LIMIT: usize = 10;

/// The bar's file in the data home that holds the Unix seconds of the last Clear Menu.
pub const CLEARED_NAME: &str = "cupertino/menubar-recent-items-cleared";

/// A listed application.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentApp {
    pub entry: DesktopEntry,
    /// Unix seconds of its last launch from the launcher.
    pub last_used: f64,
}

/// A listed document.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentDocument {
    pub path: PathBuf,
    /// Unix seconds of its last use; `None` when the list gives no time.
    pub used: Option<f64>,
}

impl RecentDocument {
    /// The file name, as the row shows it.
    pub fn label(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}

/// The rows of Recent Items, most recent first in each section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecentItems {
    pub apps: Vec<RecentApp>,
    pub documents: Vec<RecentDocument>,
}

impl RecentItems {
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty() && self.documents.is_empty()
    }
}

/// Where Recent Items is read from.
#[derive(Clone, Debug)]
pub struct RecentSources {
    /// The launcher's usage store.
    pub launcher_store: PathBuf,
    /// The recently-used list.
    pub bookmarks: PathBuf,
    /// Application directories, most important first: an application is listed while a visible entry is installed.
    pub app_dirs: Vec<PathBuf>,
    pub locale: Locale,
    pub visibility: Visibility,
    /// The bar's file with the time of the last Clear Menu.
    pub cleared: PathBuf,
}

impl RecentSources {
    /// The sources of the session's directories.
    pub fn new(dirs: &XdgDirs, locale: Locale, visibility: Visibility) -> Self {
        RecentSources {
            launcher_store: frecency::store_path(&dirs.data_home),
            bookmarks: xbel_path(&dirs.data_home),
            app_dirs: dirs
                .all_data_dirs()
                .iter()
                .map(|d| d.join("applications"))
                .collect(),
            locale,
            visibility,
            cleared: dirs.data_home.join(CLEARED_NAME),
        }
    }

    /// The time of the last Clear Menu; `None` when the menu was never cleared or the file does not read as one.
    pub fn cleared_at(&self) -> Option<f64> {
        std::fs::read_to_string(&self.cleared)
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|t| t.is_finite())
    }

    /// Clear Menu at `now` (Unix seconds): what was used until then is no longer listed.
    pub fn clear(&self, now: f64) -> io::Result<()> {
        cupertino_defaults::atomic::write_atomic(&self.cleared, format!("{now}\n").as_bytes())
    }

    /// The rows at `now` (Unix seconds). Applications without an installed visible entry and documents that no longer
    /// exist are left out; an unreadable store or list lists nothing of its section.
    pub fn read(&self, now: f64) -> RecentItems {
        let cleared = self.cleared_at();
        let after_clear = |used: Option<f64>| cleared.is_none_or(|c| used.is_some_and(|u| u > c));
        let ranked: Vec<_> = frecency::ranked_apps(&Store::read(&self.launcher_store), now)
            .into_iter()
            .filter(|app| after_clear(Some(app.last_used)))
            .collect();
        let entries = if ranked.is_empty() {
            Vec::new()
        } else {
            index_dirs(&self.app_dirs, &self.locale, &self.visibility)
        };
        let apps = ranked
            .into_iter()
            .filter_map(|app| {
                let entry = entries.iter().find(|e| e.id == app.id)?;
                Some(RecentApp {
                    entry: entry.clone(),
                    last_used: app.last_used,
                })
            })
            .take(RECENT_LIMIT)
            .collect();
        let mut files = read_recent_files(&self.bookmarks).unwrap_or_else(|e| {
            eprintln!("cupertino-menubar: cannot read the recent documents: {e}");
            Vec::new()
        });
        // newest first; a file without a time after those with one, each group in list order
        files.sort_by(|a, b| match (a.used, b.used) {
            (Some(x), Some(y)) => y.total_cmp(&x),
            (x, y) => y.is_some().cmp(&x.is_some()),
        });
        let documents = files
            .into_iter()
            .filter(|file| after_clear(file.used) && file.path.exists())
            .take(RECENT_LIMIT)
            .map(|file| RecentDocument {
                path: file.path,
                used: file.used,
            })
            .collect();
        RecentItems { apps, documents }
    }
}
