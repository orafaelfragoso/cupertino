//! `launcher.conf` (`$CUPERTINO_LAUNCHER_CONF` overrides the path): which kinds of result the launcher lists, whether
//! its best match leads the list, and which folders its file search leaves out. The launcher reads the file each time
//! its panel opens; the settings application writes it.
//!
//! | Key | Values | Unset |
//! |---|---|---|
//! | `results.applications`, `results.folders`, `results.files`, `results.calculator`, `results.actions` | `on`, `off`: whether that kind of result is listed (`actions` is an application's own shortcuts, such as a new window) | `on` |
//! | `top_hit` | `on`, `off`: the best match leads the list on its own | `on` |
//! | `files.exclude` | comma-separated folders (relative to the home folder, or absolute) the file search never enters | none |

use std::io;
use std::path::{Path, PathBuf};

use crate::conf::{self, parse_switch, switch_text, Key, Problem};

/// Environment variable that redirects the settings file.
pub const LAUNCHER_CONF_ENV: &str = "CUPERTINO_LAUNCHER_CONF";

/// Everything in `launcher.conf`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherSettings {
    pub applications: bool,
    pub folders: bool,
    pub files: bool,
    pub calculator: bool,
    pub actions: bool,
    pub top_hit: bool,
    /// Folders the file search leaves out, as written.
    pub excluded: Vec<String>,
}

macro_rules! switch_key {
    ($name:literal, $field:ident) => {
        Key {
            name: $name,
            read: |s, v| parse_switch(v).map(|on| s.$field = on),
            write: |s| Some(switch_text(s.$field)),
            keep: |s, g| s.$field = g.$field,
        }
    };
}

const KEYS: &[Key<LauncherSettings>] = &[
    switch_key!("results.applications", applications),
    switch_key!("results.folders", folders),
    switch_key!("results.files", files),
    switch_key!("results.calculator", calculator),
    switch_key!("results.actions", actions),
    switch_key!("top_hit", top_hit),
    Key {
        name: "files.exclude",
        read: |s, v| {
            s.excluded = conf::parse_list(v);
            Ok(())
        },
        write: |s| (!s.excluded.is_empty()).then(|| s.excluded.join(",")),
        keep: |s, g| s.excluded = g.excluded.clone(),
    },
];

impl LauncherSettings {
    /// What a missing or empty file reads as: every kind of result, the top hit, nothing left out.
    pub const UNSET: Self = Self {
        applications: true,
        folders: true,
        files: true,
        calculator: true,
        actions: true,
        top_hit: true,
        excluded: Vec::new(),
    };

    /// Where the file is: `$CUPERTINO_LAUNCHER_CONF`, else `launcher.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(LAUNCHER_CONF_ENV, "launcher.conf")
    }

    /// The settings of `text` and the problems of its lines; with `last_good`, a key whose line is wrong keeps its
    /// last good value.
    pub fn parse(text: &str, last_good: Option<&Self>) -> (Self, Vec<Problem>) {
        conf::read(
            &conf::ConfText::parse(text),
            KEYS,
            &Self::UNSET,
            last_good,
            None,
        )
    }

    /// [`Self::parse`] of the file at `path`; a missing file is every key unset.
    pub fn load(path: &Path, last_good: Option<&Self>) -> io::Result<(Self, Vec<Problem>)> {
        conf::load(path, KEYS, &Self::UNSET, last_good, None)
    }

    /// Write the keys whose value differs from the file's into `path` atomically; comments and other lines stay.
    pub fn store(&self, path: &Path) -> io::Result<Vec<Problem>> {
        conf::store(path, KEYS, &Self::UNSET, None, self)
    }

    /// The file search patterns the exclusions make: each folder anchored under the search root, gitignore style.
    pub fn exclusion_patterns(&self, root: &Path) -> Vec<String> {
        self.excluded
            .iter()
            .map(|folder| {
                let path = Path::new(folder);
                let relative = path.strip_prefix(root).unwrap_or(path);
                format!("/{}/", relative.to_string_lossy().trim_matches('/'))
            })
            .collect()
    }
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self::UNSET
    }
}
