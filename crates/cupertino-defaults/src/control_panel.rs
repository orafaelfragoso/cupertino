//! `control-panel.conf` (`$CUPERTINO_CONTROL_PANEL_CONF` overrides the path): which controls the control panel shows
//! and in which order. The panel writes the file when its controls are edited and reads it as it opens and while it
//! is open.
//!
//! | Key | Values | Unset |
//! |---|---|---|
//! | `controls` | comma-separated names of the controls shown, in order (`wifi`, `now-playing`, `bluetooth`, `microphone`, `dock-hiding`, `lock-screen`, `mute`, `keyboard-brightness`, `focus`, `display`, `sound`); an empty value shows none | every control in the panel's own order |
//!
//! The names are the panel's: this file keeps them as written, and the panel skips one it does not know.

use std::io;
use std::path::{Path, PathBuf};

use crate::conf::{self, Key, Problem};

/// Environment variable that redirects the settings file.
pub const CONTROL_PANEL_CONF_ENV: &str = "CUPERTINO_CONTROL_PANEL_CONF";

/// Everything in `control-panel.conf`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlPanelSettings {
    /// The names of the controls shown, in order; `None` when the file does not say.
    pub controls: Option<Vec<String>>,
}

const KEYS: &[Key<ControlPanelSettings>] = &[Key {
    name: "controls",
    read: |s, v| {
        s.controls = Some(conf::parse_list(v));
        Ok(())
    },
    write: |s| s.controls.as_ref().map(|controls| controls.join(",")),
    keep: |s, g| s.controls = g.controls.clone(),
}];

impl ControlPanelSettings {
    /// What a missing or empty file reads as: the panel's own controls and order.
    pub const UNSET: Self = Self { controls: None };

    /// Where the file is: `$CUPERTINO_CONTROL_PANEL_CONF`, else `control-panel.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(CONTROL_PANEL_CONF_ENV, "control-panel.conf")
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
}
