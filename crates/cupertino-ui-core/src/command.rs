//! A window's keyboard shortcuts. A key reaches them after the focused node and its ancestors let it pass.

use crate::Shortcut;
use cupertino_ui_platform::{KeyEvent, Keysym};
use std::collections::{HashMap, HashSet};

/// Shortcuts and the message each one sends.
#[derive(Clone, Debug)]
pub struct CommandRegistry<M> {
    shortcuts: HashMap<Shortcut, M>,
    /// Shortcuts a held key's repeats do not send ([`CommandRegistry::with_fresh`]).
    fresh: HashSet<Shortcut>,
}

impl<M> Default for CommandRegistry<M> {
    fn default() -> Self {
        Self {
            shortcuts: HashMap::new(),
            fresh: HashSet::new(),
        }
    }
}

impl<M: Clone> CommandRegistry<M> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, shortcut: Shortcut, message: M) -> Self {
        self.insert(shortcut, message);
        self
    }

    pub fn insert(&mut self, shortcut: Shortcut, message: M) -> Option<M> {
        self.shortcuts.insert(normalize(shortcut), message)
    }

    /// [`CommandRegistry::with`] for a shortcut that only a fresh press sends, never the repeats of a held key: a key
    /// pressed before a consent window could act, and held, does not answer it.
    pub fn with_fresh(mut self, shortcut: Shortcut, message: M) -> Self {
        let shortcut = normalize(shortcut);
        self.fresh.insert(shortcut);
        self.shortcuts.insert(shortcut, message);
        self
    }

    /// The message of the shortcut `key` presses, if any.
    pub fn lookup(&self, key: &KeyEvent) -> Option<M> {
        if !key.pressed {
            return None;
        }
        let shortcut = normalize(Shortcut {
            mods: key.mods,
            keysym: key.keysym,
        });
        if key.repeat && self.fresh.contains(&shortcut) {
            return None;
        }
        self.shortcuts.get(&shortcut).cloned()
    }
}

/// Shift changes a letter's keysym to its capital; a shortcut names the letter either way.
fn normalize(shortcut: Shortcut) -> Shortcut {
    let Keysym(k) = shortcut.keysym;
    let keysym = if (0x41..=0x5a).contains(&k) {
        Keysym(k + 0x20)
    } else {
        shortcut.keysym
    };
    Shortcut { keysym, ..shortcut }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_ui_platform::Mods;

    fn key(keysym: u32, mods: Mods, pressed: bool) -> KeyEvent {
        KeyEvent {
            code: 0,
            keysym: Keysym(keysym),
            text: None,
            pressed,
            repeat: false,
            mods,
        }
    }

    #[test]
    fn a_shortcut_matches_its_modifiers_exactly_and_a_letter_in_either_case() {
        let shift_primary = Mods::PRIMARY | Mods::SHIFT;
        let registry = CommandRegistry::new()
            .with(
                Shortcut {
                    mods: Mods::PRIMARY,
                    keysym: Keysym(0x7a),
                },
                "undo",
            )
            .with(
                Shortcut {
                    mods: shift_primary,
                    keysym: Keysym(0x7a),
                },
                "redo",
            );
        assert_eq!(
            registry.lookup(&key(0x7a, Mods::PRIMARY, true)),
            Some("undo")
        );
        assert_eq!(
            registry.lookup(&key(0x5a, shift_primary, true)),
            Some("redo")
        );
        assert_eq!(registry.lookup(&key(0x7a, Mods::PRIMARY, false)), None);
        assert_eq!(registry.lookup(&key(0x7a, Mods::NONE, true)), None);
        assert_eq!(
            registry.lookup(&key(0x7a, Mods::PRIMARY | Mods::ALT, true)),
            None
        );
    }

    #[test]
    fn a_fresh_shortcut_ignores_the_repeats_of_a_held_key_and_others_take_them() {
        let enter = Shortcut {
            mods: Mods::NONE,
            keysym: Keysym(0xff0d),
        };
        let undo = Shortcut {
            mods: Mods::PRIMARY,
            keysym: Keysym(0x7a),
        };
        let registry = CommandRegistry::new()
            .with_fresh(enter, "share")
            .with(undo, "undo");
        let repeat = |mut k: KeyEvent| {
            k.repeat = true;
            k
        };
        assert_eq!(
            registry.lookup(&key(0xff0d, Mods::NONE, true)),
            Some("share")
        );
        assert_eq!(
            registry.lookup(&repeat(key(0xff0d, Mods::NONE, true))),
            None
        );
        assert_eq!(
            registry.lookup(&repeat(key(0x7a, Mods::PRIMARY, true))),
            Some("undo")
        );
    }
}
