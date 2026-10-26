//! Text-editing key bindings: standard table, override file and a resolver.
//!
//! The standard table ([`crate::binding_table::STANDARD`]) is generated from the verified binding table
//! Emacs-style control chords, alt+arrow word moves, primary+arrow line / document moves and the
//! shift variants that extend the selection. An optional user file extends or replaces rows.
//!
//! Override file (`~/.config/cupertino/keybindings.conf`), one binding per line:
//!
//! ```text
//! # comment
//! Ctrl+K = DeleteToEndOfParagraph
//! Alt+Left = MoveWordLeft, MoveLeft        # several actions are tried in order by the editor
//! Ctrl+X Ctrl+C = Noop                     # chords separated by spaces form a sequence
//! Ctrl+Y = unbind                          # remove the standard binding
//! ```

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

pub use crate::binding_table::TextAction;
use crate::binding_table::STANDARD;
use crate::keys::{Chord, ChordError};

impl TextAction {
    /// Looks an action up by its (case-insensitive) name.
    pub fn from_name(name: &str) -> Option<TextAction> {
        TextAction::ALL
            .iter()
            .copied()
            .find(|a| a.name().eq_ignore_ascii_case(name))
    }
}

/// Problems found while parsing an override file; parsing continues after each one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideError {
    pub line: usize,
    pub message: String,
}

/// Chord sequence → actions.
#[derive(Clone, Debug, Default)]
pub struct BindingTable {
    map: HashMap<Vec<Chord>, Vec<TextAction>>,
}

impl BindingTable {
    pub fn empty() -> BindingTable {
        BindingTable::default()
    }

    /// The standard table.
    pub fn standard() -> BindingTable {
        let mut t = BindingTable::empty();
        for (mods, key, acts) in STANDARD {
            t.bind(vec![Chord::new(*mods, *key)], acts.to_vec());
        }
        t
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bind(&mut self, seq: Vec<Chord>, actions: Vec<TextAction>) {
        self.map.insert(seq, actions);
    }

    pub fn unbind(&mut self, seq: &[Chord]) -> bool {
        self.map.remove(seq).is_some()
    }

    pub fn get(&self, seq: &[Chord]) -> Option<&[TextAction]> {
        self.map.get(seq).map(|v| v.as_slice())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Vec<Chord>, &Vec<TextAction>)> {
        self.map.iter()
    }

    fn has_longer(&self, prefix: &[Chord]) -> bool {
        self.map
            .keys()
            .any(|k| k.len() > prefix.len() && k.starts_with(prefix))
    }

    /// Applies override text; returns the problems found.
    pub fn apply_overrides(&mut self, text: &str) -> Vec<OverrideError> {
        let mut errs = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((lhs, rhs)) = line.split_once('=') else {
                errs.push(OverrideError {
                    line: i + 1,
                    message: "expected 'chord = action'".into(),
                });
                continue;
            };
            let mut seq = Vec::new();
            let mut bad = false;
            for tok in lhs.split_whitespace() {
                match Chord::parse(tok) {
                    Ok(c) => seq.push(c),
                    Err(e) => {
                        errs.push(OverrideError {
                            line: i + 1,
                            message: describe(&e),
                        });
                        bad = true;
                        break;
                    }
                }
            }
            if bad {
                continue;
            }
            if seq.is_empty() {
                errs.push(OverrideError {
                    line: i + 1,
                    message: "empty chord".into(),
                });
                continue;
            }
            let rhs = rhs.trim();
            if rhs.eq_ignore_ascii_case("unbind") {
                self.unbind(&seq);
                continue;
            }
            let mut acts = Vec::new();
            for a in rhs.split(',') {
                match TextAction::from_name(a.trim()) {
                    Some(x) => acts.push(x),
                    None => {
                        errs.push(OverrideError {
                            line: i + 1,
                            message: format!("unknown action '{}'", a.trim()),
                        });
                        bad = true;
                        break;
                    }
                }
            }
            if !bad && !acts.is_empty() {
                self.bind(seq, acts);
            }
        }
        errs
    }

    /// Loads a user file on top of the current table. A missing file is not an error.
    pub fn load_user_file(&mut self, path: &Path) -> io::Result<Vec<OverrideError>> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(self.apply_overrides(&s)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// `$XDG_CONFIG_HOME/cupertino/keybindings.conf`, falling back to `~/.config/cupertino/keybindings.conf`.
    pub fn default_user_path() -> Option<PathBuf> {
        cupertino_defaults::config_home().map(|c| c.join("cupertino").join("keybindings.conf"))
    }

    /// Standard table plus the user file when present.
    pub fn load_default() -> (BindingTable, Vec<OverrideError>) {
        let mut t = BindingTable::standard();
        let errs = Self::default_user_path()
            .and_then(|p| t.load_user_file(&p).ok())
            .unwrap_or_default();
        (t, errs)
    }
}

fn describe(e: &ChordError) -> String {
    e.to_string()
}

/// Result of feeding a chord to a [`Resolver`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// A binding matched; the editor tries the actions in order.
    Actions(Vec<TextAction>),
    /// The chord is the prefix of a longer sequence.
    Pending,
    /// No binding; the key is handled as text input or passed on.
    NoMatch,
}

/// Stateful resolver supporting chord sequences.
#[derive(Clone, Debug)]
pub struct Resolver {
    table: BindingTable,
    pending: Vec<Chord>,
}

impl Resolver {
    pub fn new(table: BindingTable) -> Resolver {
        Resolver {
            table,
            pending: Vec::new(),
        }
    }

    pub fn table(&self) -> &BindingTable {
        &self.table
    }

    pub fn table_mut(&mut self) -> &mut BindingTable {
        &mut self.table
    }

    pub fn reset(&mut self) {
        self.pending.clear();
    }

    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn feed(&mut self, chord: Chord) -> Resolution {
        self.pending.push(chord);
        if let Some(a) = self.table.get(&self.pending) {
            let a = a.to_vec();
            self.pending.clear();
            return Resolution::Actions(a);
        }
        if self.table.has_longer(&self.pending) {
            return Resolution::Pending;
        }
        let was_sequence = self.pending.len() > 1;
        self.pending.clear();
        if was_sequence {
            return self.feed(chord);
        }
        Resolution::NoMatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    #[test]
    fn standard_table_rows() {
        let t = BindingTable::standard();
        assert!(t.len() >= 100);
        assert_eq!(
            t.get(&[c("Ctrl+A")]),
            Some(&[TextAction::MoveToBeginningOfParagraph][..])
        );
        assert_eq!(
            t.get(&[c("Ctrl+Shift+A")]),
            Some(&[TextAction::MoveToBeginningOfParagraphAndModifySelection][..])
        );
        assert_eq!(
            t.get(&[c("Alt+Left")]),
            Some(&[TextAction::MoveWordLeft][..])
        );
        assert_eq!(
            t.get(&[c("Primary+Left")]),
            Some(&[TextAction::MoveToLeftEndOfLine][..])
        );
        assert_eq!(
            t.get(&[c("Primary+Shift+Up")]),
            Some(&[TextAction::MoveToBeginningOfDocumentAndModifySelection][..])
        );
        assert_eq!(
            t.get(&[c("Alt+Backspace")]),
            Some(&[TextAction::DeleteWordBackward][..])
        );
        assert_eq!(
            t.get(&[c("Primary+Backspace")]),
            Some(&[TextAction::DeleteToBeginningOfLine][..])
        );
        assert_eq!(
            t.get(&[c("Ctrl+K")]),
            Some(&[TextAction::DeleteToEndOfParagraph][..])
        );
        assert_eq!(t.get(&[c("Ctrl+Y")]), Some(&[TextAction::Yank][..]));
        assert_eq!(t.get(&[c("Ctrl+T")]), Some(&[TextAction::Transpose][..]));
    }

    #[test]
    fn multi_action_rows() {
        let t = BindingTable::standard();
        assert_eq!(
            t.get(&[c("Alt+Up")]),
            Some(
                &[
                    TextAction::MoveBackward,
                    TextAction::MoveToBeginningOfParagraph
                ][..]
            )
        );
        assert_eq!(
            t.get(&[c("Ctrl+O")]),
            Some(
                &[
                    TextAction::InsertNewlineIgnoringFieldEditor,
                    TextAction::MoveBackward
                ][..]
            )
        );
    }

    #[test]
    fn plain_arrows_and_page_keys() {
        let t = BindingTable::standard();
        assert_eq!(t.get(&[c("Left")]), Some(&[TextAction::MoveLeft][..]));
        assert_eq!(
            t.get(&[c("Shift+Left")]),
            Some(&[TextAction::MoveLeftAndModifySelection][..])
        );
        assert_eq!(
            t.get(&[c("PageDown")]),
            Some(&[TextAction::ScrollPageDown][..])
        );
        assert_eq!(
            t.get(&[c("Delete")]),
            Some(&[TextAction::DeleteForward][..])
        );
    }

    #[test]
    fn overrides_bind_unbind_and_report_errors() {
        let mut t = BindingTable::standard();
        let errs = t.apply_overrides(
            "# test\nCtrl+K = DeleteToBeginningOfLine\nCtrl+Y = unbind\nAlt+Left = MoveWordLeft, MoveLeft\nbogus line\nCtrl+Q = NoSuchAction\nHyper+A = Noop\n",
        );
        assert_eq!(
            t.get(&[c("Ctrl+K")]),
            Some(&[TextAction::DeleteToBeginningOfLine][..])
        );
        assert_eq!(t.get(&[c("Ctrl+Y")]), None);
        assert_eq!(
            t.get(&[c("Alt+Left")]),
            Some(&[TextAction::MoveWordLeft, TextAction::MoveLeft][..])
        );
        assert_eq!(errs.len(), 3);
        assert_eq!(errs[0].line, 5);
    }

    #[test]
    fn sequences_resolve_with_prefix() {
        let mut t = BindingTable::empty();
        t.apply_overrides("Ctrl+X Ctrl+K = DeleteToBeginningOfLine\nCtrl+B = MoveBackward\n");
        let mut r = Resolver::new(t);
        assert_eq!(r.feed(c("Ctrl+X")), Resolution::Pending);
        assert!(r.is_pending());
        assert_eq!(
            r.feed(c("Ctrl+K")),
            Resolution::Actions(vec![TextAction::DeleteToBeginningOfLine])
        );
        assert_eq!(r.feed(c("Ctrl+X")), Resolution::Pending);
        assert_eq!(
            r.feed(c("Ctrl+B")),
            Resolution::Actions(vec![TextAction::MoveBackward])
        );
        assert_eq!(r.feed(c("A")), Resolution::NoMatch);
    }

    #[test]
    fn missing_user_file_is_fine() {
        let mut t = BindingTable::standard();
        let p = std::path::Path::new("/nonexistent/cupertino/keybindings.conf");
        assert_eq!(t.load_user_file(p).unwrap(), Vec::new());
    }

    #[test]
    fn user_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cupertino-kb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("keybindings.conf");
        std::fs::write(&p, "Ctrl+J = InsertNewline\n").unwrap();
        let mut t = BindingTable::standard();
        assert!(t.load_user_file(&p).unwrap().is_empty());
        assert_eq!(
            t.get(&[c("Ctrl+J")]),
            Some(&[TextAction::InsertNewline][..])
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_path_shape() {
        if let Some(p) = BindingTable::default_user_path() {
            assert!(p.ends_with("cupertino/keybindings.conf"));
        }
    }
}
