//! Text-editing key bindings: standard table, override file and a resolver.
//!
//! The standard table ([`crate::binding_table::STANDARD`]) is generated from the verified binding table
//! Emacs-style control chords, alt+arrow word moves, primary+arrow line / document moves and the
//! shift variants that extend the selection. An optional user file extends or replaces rows.
//!
//! Override file (`cupertino-ui-text` reads `~/.config/cupertino/keybindings.conf`), one binding per line:
//!
//! ```text
//! # comment
//! Ctrl+K = DeleteToEndOfParagraph
//! Alt+Left = MoveWordLeft, MoveLeft        # several actions are tried in order by the editor
//! Ctrl+X Ctrl+C = Noop                     # chords separated by spaces form a sequence
//! Ctrl+Y = unbind                          # remove the standard binding
//! ```

use std::collections::HashMap;

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
