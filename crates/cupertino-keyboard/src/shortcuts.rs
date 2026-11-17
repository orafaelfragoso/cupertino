//! Desktop-level shortcuts (screenshots, launcher, app switcher, input sources, overview, workspaces ...).
//!
//! [`ASSIGNMENTS`] is the built-in assignment table of the design: 80 rows keyed by the
//! numbering of the source table, expressed with logical modifiers ([`crate::modifiers::Mods::PRIMARY`] is the
//! shortcut modifier of the active [`crate::modifiers::Profile`]). Whether an assignment is *enabled*
//! out of the box is not stored in the table and could not be (unknown);
//! [`ShortcutSet::reference_defaults`] enables the commonly used subset (approximate).

use std::collections::HashMap;

use crate::keys::Chord;
pub use crate::shortcut_table::{Assignment, SystemAction, ASSIGNMENTS};

/// Actions enabled by [`ShortcutSet::reference_defaults`] (approximate).
pub const REFERENCE_ENABLED: &[SystemAction] = &[
    SystemAction::AppSwitcherNext,
    SystemAction::AppSwitcherPrevious,
    SystemAction::WindowCycleNext,
    SystemAction::WindowCyclePrevious,
    SystemAction::ForceQuitDialog,
    SystemAction::ScreenshotScreenToFile,
    SystemAction::ScreenshotScreenToClipboard,
    SystemAction::ScreenshotSelectionToFile,
    SystemAction::ScreenshotSelectionToClipboard,
    SystemAction::ScreenshotOptions,
    SystemAction::OverviewAllWindows,
    SystemAction::OverviewAppWindows,
    SystemAction::ShowDesktop,
    SystemAction::WorkspacePrevious,
    SystemAction::WorkspaceNext,
    SystemAction::LauncherToggle,
    SystemAction::LauncherFileSearch,
    SystemAction::InputSourcePrevious,
    SystemAction::InputSourceNext,
    SystemAction::SymbolPicker,
    SystemAction::HelpSearch,
    SystemAction::FullscreenToggle,
    SystemAction::WindowMinimize,
    SystemAction::WindowMinimizeAll,
    SystemAction::DockAutoHideToggle,
    SystemAction::FocusMenuBar,
    SystemAction::FocusDock,
    SystemAction::FocusWindow,
    SystemAction::FocusToolbar,
];

/// Chord → action map with enable / rebind support.
#[derive(Clone, Debug, Default)]
pub struct ShortcutSet {
    assigned: HashMap<SystemAction, Chord>,
    enabled: HashMap<SystemAction, bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutError {
    pub line: usize,
    pub message: String,
}

impl ShortcutSet {
    /// Every row of the built-in table, all enabled.
    pub fn all_assigned() -> ShortcutSet {
        let mut s = ShortcutSet::default();
        for a in ASSIGNMENTS {
            s.assigned.insert(a.action, Chord::new(a.mods, a.key));
            s.enabled.insert(a.action, true);
        }
        s
    }

    /// The built-in table with only [`REFERENCE_ENABLED`] switched on.
    pub fn reference_defaults() -> ShortcutSet {
        let mut s = Self::all_assigned();
        for v in s.enabled.values_mut() {
            *v = false;
        }
        for a in REFERENCE_ENABLED {
            s.enabled.insert(*a, true);
        }
        s
    }

    pub fn chord_for(&self, action: SystemAction) -> Option<Chord> {
        self.assigned.get(&action).copied()
    }

    pub fn is_enabled(&self, action: SystemAction) -> bool {
        self.enabled.get(&action).copied().unwrap_or(false)
    }

    pub fn set_enabled(&mut self, action: SystemAction, on: bool) {
        self.enabled.insert(action, on);
    }

    pub fn rebind(&mut self, action: SystemAction, chord: Chord) {
        self.assigned.insert(action, chord);
        self.enabled.entry(action).or_insert(true);
    }

    pub fn unbind(&mut self, action: SystemAction) {
        self.assigned.remove(&action);
    }

    /// Enabled action for a chord. When several actions share a chord (e.g. the slow variants) the one with the lowest
    /// source-table id wins.
    pub fn lookup(&self, chord: &Chord) -> Option<SystemAction> {
        let mut best: Option<(u16, SystemAction)> = None;
        for (a, c) in &self.assigned {
            if c == chord && self.is_enabled(*a) {
                let id = ASSIGNMENTS
                    .iter()
                    .find(|x| x.action == *a)
                    .map(|x| x.id)
                    .unwrap_or(u16::MAX);
                if best.is_none_or(|(b, _)| id < b) {
                    best = Some((id, *a));
                }
            }
        }
        best.map(|(_, a)| a)
    }

    /// `Action = Chord` or `Action = none` / `Action = off`; `#` comments.
    pub fn apply_overrides(&mut self, text: &str) -> Vec<ShortcutError> {
        let mut errs = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((l, r)) = line.split_once('=') else {
                errs.push(ShortcutError {
                    line: i + 1,
                    message: "expected 'Action = chord'".into(),
                });
                continue;
            };
            let Some(action) = SystemAction::ALL
                .iter()
                .copied()
                .find(|a| a.name().eq_ignore_ascii_case(l.trim()))
            else {
                errs.push(ShortcutError {
                    line: i + 1,
                    message: format!("unknown action '{}'", l.trim()),
                });
                continue;
            };
            let r = r.trim();
            if r.eq_ignore_ascii_case("none") || r.eq_ignore_ascii_case("off") {
                self.set_enabled(action, false);
                continue;
            }
            match Chord::parse(r) {
                Ok(c) => {
                    self.rebind(action, c);
                    self.set_enabled(action, true);
                }
                Err(e) => errs.push(ShortcutError {
                    line: i + 1,
                    message: e.to_string(),
                }),
            }
        }
        errs
    }
}
