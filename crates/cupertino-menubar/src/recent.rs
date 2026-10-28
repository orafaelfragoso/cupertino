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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct Fixture {
        dir: tempfile::TempDir,
        sources: RecentSources,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let dirs = XdgDirs {
                config_home: root.join("config"),
                config_dirs: vec![],
                data_home: root.join("data"),
                data_dirs: vec![root.join("usr")],
                current_desktops: vec![],
            };
            let visibility = Visibility {
                skip_try_exec: true,
                ..Visibility::default()
            };
            let sources = RecentSources::new(&dirs, Locale::default(), visibility);
            Fixture { dir, sources }
        }

        fn write(&self, path: &Path, text: &str) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn app(&self, id: &str, extra: &str) {
            self.write(
                &self.dir.path().join("usr/applications").join(id),
                &format!("[Desktop Entry]\nType=Application\nName=App {id}\nExec=app\n{extra}"),
            );
        }

        /// The launcher's store with `app:<id>` launched once at each time.
        fn launches(&self, launches: &[(&str, f64)]) {
            let items: Vec<String> = launches
                .iter()
                .map(|(id, t)| {
                    format!("\"app:{id}\": {{\"score\": 1.0, \"last_used\": {t}, \"launches\": 1}}")
                })
                .collect();
            self.write(
                &self.sources.launcher_store,
                &format!("{{\"version\": 1, \"items\": {{{}}}}}", items.join(",")),
            );
        }

        /// A document that exists.
        fn document(&self, name: &str) -> PathBuf {
            let path = self.dir.path().join("home").join(name);
            self.write(&path, "x");
            path
        }

        /// The recently-used list, each file added at 09:00 UTC on its day of October 2026.
        fn bookmarks(&self, files: &[(&Path, u32)]) {
            let rows: String = files
                .iter()
                .map(|(path, day)| {
                    format!(
                        "<bookmark href=\"file://{}\" added=\"2026-10-{day:02}T09:00:00Z\"/>\n",
                        path.display()
                    )
                })
                .collect();
            self.write(&self.sources.bookmarks, &format!("<xbel>\n{rows}</xbel>"));
        }
    }

    fn app_ids(items: &RecentItems) -> Vec<&str> {
        items.apps.iter().map(|a| a.entry.id.as_str()).collect()
    }

    fn document_labels(items: &RecentItems) -> Vec<String> {
        items.documents.iter().map(RecentDocument::label).collect()
    }

    #[test]
    fn nothing_recorded_lists_nothing() {
        assert_eq!(Fixture::new().sources.read(0.0), RecentItems::default());
    }

    #[test]
    fn apps_follow_the_launchers_order_and_skip_ids_without_a_visible_entry() {
        let f = Fixture::new();
        f.app("new.desktop", "");
        f.app("old.desktop", "");
        f.app("hidden.desktop", "NoDisplay=true\n");
        f.launches(&[
            ("old.desktop", 0.0),
            ("gone.desktop", 50.0 * 86400.0),
            ("hidden.desktop", 50.0 * 86400.0),
            ("new.desktop", 40.0 * 86400.0),
        ]);
        let items = f.sources.read(60.0 * 86400.0);
        assert_eq!(app_ids(&items), ["new.desktop", "old.desktop"]);
        assert_eq!(items.apps[0].entry.name, "App new.desktop");
    }

    #[test]
    fn documents_are_newest_first_and_missing_files_are_skipped() {
        let f = Fixture::new();
        let (a, b) = (f.document("a.txt"), f.document("b.txt"));
        let gone = f.dir.path().join("home/gone.txt");
        f.bookmarks(&[(&a, 1), (&gone, 9), (&b, 3)]);
        assert_eq!(document_labels(&f.sources.read(0.0)), ["b.txt", "a.txt"]);
    }

    #[test]
    fn each_section_lists_at_most_ten() {
        let f = Fixture::new();
        let mut launches = Vec::new();
        let ids: Vec<String> = (0..12).map(|i| format!("app{i:02}.desktop")).collect();
        for (i, id) in ids.iter().enumerate() {
            f.app(id, "");
            launches.push((id.as_str(), i as f64));
        }
        f.launches(&launches);
        let paths: Vec<PathBuf> = (0..12)
            .map(|i| f.document(&format!("{i:02}.txt")))
            .collect();
        let marks: Vec<(&Path, u32)> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (p.as_path(), i as u32 + 1))
            .collect();
        f.bookmarks(&marks);
        let items = f.sources.read(12.0);
        assert_eq!(items.apps.len(), RECENT_LIMIT);
        assert_eq!(app_ids(&items)[0], "app11.desktop", "most recent first");
        assert_eq!(items.documents.len(), RECENT_LIMIT);
        assert_eq!(document_labels(&items)[0], "11.txt");
        assert_eq!(document_labels(&items)[9], "02.txt");
    }

    #[test]
    fn clear_menu_hides_what_was_used_until_then() {
        // 2026-10-15T00:00:00Z
        const CLEARED: f64 = 1_792_022_400.0;
        let f = Fixture::new();
        f.app("before.desktop", "");
        f.app("at.desktop", "");
        f.app("after.desktop", "");
        f.launches(&[
            ("before.desktop", CLEARED - 86400.0),
            ("at.desktop", CLEARED),
            ("after.desktop", CLEARED + 1.0),
        ]);
        let (before, after) = (f.document("before.txt"), f.document("after.txt"));
        f.bookmarks(&[(&before, 14), (&after, 16)]);
        assert_eq!(f.sources.read(CLEARED + 2.0).apps.len(), 3);
        f.sources.clear(CLEARED).unwrap();
        assert_eq!(f.sources.cleared_at(), Some(CLEARED));
        let items = f.sources.read(CLEARED + 2.0);
        assert_eq!(
            app_ids(&items),
            ["after.desktop"],
            "at the cutoff is hidden"
        );
        assert_eq!(document_labels(&items), ["after.txt"]);
    }

    #[test]
    fn a_document_without_a_time_is_listed_until_the_menu_is_cleared() {
        let f = Fixture::new();
        let a = f.document("a.txt");
        f.write(
            &f.sources.bookmarks,
            &format!("<xbel><bookmark href=\"file://{}\"/></xbel>", a.display()),
        );
        assert_eq!(document_labels(&f.sources.read(0.0)), ["a.txt"]);
        f.sources.clear(0.0).unwrap();
        assert!(f.sources.read(0.0).documents.is_empty());
    }
}
