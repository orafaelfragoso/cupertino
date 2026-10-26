//! Rename logic: click-and-wait trigger, name validation, extension-change warning, and the text
//! range selected when editing starts.
//!
//! * The design waits for the system double-click interval (a gesture recogniser whose timeout
//!   is exactly that interval) before acting on a single click; starting a rename from a
//!   click on the name of an already selected item is the behaviour built on it (the
//!   link between the two is ).
//! * The extension-change warning has the preference default `true`.

use crate::entry::extension_of;
use crate::metrics::timing::DOUBLE_CLICK_INTERVAL;

/// State machine of "click on the selected item's name, wait, start editing".
#[derive(Clone, Debug)]
pub struct RenameTrigger {
    pub interval: f64,
    pending: Option<(usize, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameEvent {
    /// Start editing the item.
    Begin(usize),
}

impl Default for RenameTrigger {
    fn default() -> Self {
        RenameTrigger {
            interval: DOUBLE_CLICK_INTERVAL,
            pending: None,
        }
    }
}

impl RenameTrigger {
    /// A click. `on_name` is true when the click hit the name label, `was_selected` when the item
    /// was the only selected item before the click. A second click inside the interval is a
    /// double-click (open) and cancels the pending rename.
    pub fn click(&mut self, index: usize, on_name: bool, was_selected: bool, now: f64) {
        match self.pending {
            Some((i, t)) if i == index && now - t <= self.interval => {
                self.pending = None;
            }
            _ => {
                self.pending = if on_name && was_selected {
                    Some((index, now))
                } else {
                    None
                };
            }
        }
    }

    /// Anything that invalidates the click: drag, selection change, key press, scroll.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// Poll with the current time; emits `Begin` once the interval elapsed.
    pub fn poll(&mut self, now: f64) -> Option<RenameEvent> {
        match self.pending {
            Some((i, t)) if now - t > self.interval => {
                self.pending = None;
                Some(RenameEvent::Begin(i))
            }
            _ => None,
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Time at which `poll` will fire, if a rename is pending.
    pub fn deadline(&self) -> Option<f64> {
        self.pending.map(|(_, t)| t + self.interval)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    ContainsSlash,
    ContainsNul,
    TooLong,
    Dot,
}

/// Longest file name in bytes on common Linux file systems.
pub const MAX_NAME_BYTES: usize = 255;

/// Validate a new file name (POSIX rules: no `/`, no NUL, 1..=255 bytes, not `.` or `..`).
pub fn validate_name(new: &str) -> Result<(), NameError> {
    if new.is_empty() {
        return Err(NameError::Empty);
    }
    if new.contains('/') {
        return Err(NameError::ContainsSlash);
    }
    if new.contains('\0') {
        return Err(NameError::ContainsNul);
    }
    if new.len() > MAX_NAME_BYTES {
        return Err(NameError::TooLong);
    }
    if new == "." || new == ".." {
        return Err(NameError::Dot);
    }
    Ok(())
}

/// True when renaming `old` to `new` changes the extension, which the design confirms with a
/// dialog unless the user disabled it.
pub fn needs_extension_warning(old: &str, new: &str, is_dir: bool) -> bool {
    if is_dir {
        return false;
    }
    extension_of(old).map(str::to_lowercase) != extension_of(new).map(str::to_lowercase)
}

/// Character range (`start..end`, in chars) selected when editing begins: the name without its
/// extension (behaviour); the whole name for folders, dot-files and names without extension.
pub fn initial_selection(name: &str, is_dir: bool) -> std::ops::Range<usize> {
    let total = name.chars().count();
    if is_dir {
        return 0..total;
    }
    match name.rfind('.') {
        Some(idx) if idx > 0 && idx + 1 < name.len() => 0..name[..idx].chars().count(),
        _ => 0..total,
    }
}

/// A free name for `base` in `taken` using the "name copy", "name copy 2" ... scheme used for
/// duplicates and conflicts.
pub fn unique_name(base: &str, is_dir: bool, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    let (stem, ext) = match (is_dir, base.rfind('.')) {
        (false, Some(i)) if i > 0 => (&base[..i], &base[i..]),
        _ => (base, ""),
    };
    let mut n = 1;
    loop {
        let cand = if n == 1 {
            format!("{stem} copy{ext}")
        } else {
            format!("{stem} copy {n}{ext}")
        };
        if !taken(&cand) {
            return cand;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_starts_after_the_interval() {
        let mut r = RenameTrigger::default();
        r.click(3, true, true, 10.0);
        assert!(r.is_pending());
        assert_eq!(r.poll(10.4), None);
        assert_eq!(r.deadline(), Some(10.5));
        assert_eq!(r.poll(10.6), Some(RenameEvent::Begin(3)));
        assert_eq!(r.poll(11.0), None);
    }

    #[test]
    fn second_click_is_a_double_click_and_cancels() {
        let mut r = RenameTrigger::default();
        r.click(3, true, true, 0.0);
        r.click(3, true, true, 0.3);
        assert_eq!(r.poll(1.0), None);
    }

    #[test]
    fn only_selected_name_clicks_arm_the_trigger() {
        let mut r = RenameTrigger::default();
        r.click(1, false, true, 0.0);
        assert!(!r.is_pending());
        r.click(1, true, false, 0.0);
        assert!(!r.is_pending());
        r.click(1, true, true, 0.0);
        r.cancel();
        assert_eq!(r.poll(5.0), None);
    }

    #[test]
    fn name_validation() {
        assert_eq!(validate_name("ok.txt"), Ok(()));
        assert_eq!(validate_name(""), Err(NameError::Empty));
        assert_eq!(validate_name("a/b"), Err(NameError::ContainsSlash));
        assert_eq!(validate_name(".."), Err(NameError::Dot));
        assert_eq!(validate_name(&"x".repeat(256)), Err(NameError::TooLong));
    }

    #[test]
    fn extension_warning_and_selection_range() {
        assert!(needs_extension_warning("a.txt", "a.md", false));
        assert!(!needs_extension_warning("a.TXT", "b.txt", false));
        assert!(!needs_extension_warning("dir.d", "dir", true));
        assert_eq!(initial_selection("report.final.pdf", false), 0..12);
        assert_eq!(initial_selection(".hidden", false), 0..7);
        assert_eq!(initial_selection("folder.d", true), 0..8);
        assert_eq!(initial_selection("naïve.txt", false), 0..5);
    }

    #[test]
    fn unique_names() {
        let taken = |n: &str| matches!(n, "a.txt" | "a copy.txt" | "a copy 2.txt");
        assert_eq!(unique_name("a.txt", false, &taken), "a copy 3.txt");
        assert_eq!(unique_name("b.txt", false, &taken), "b.txt");
    }
}
