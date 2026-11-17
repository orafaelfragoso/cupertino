//! Chords in logical modifiers, and their accelerator and display text through a modifier map.

use cupertino_modifiers::{Modifier, ModifierMap, Mods, Physical};

/// The modifier of an accelerator's angle brackets for `p`; `None` for a key GTK has no accelerator for.
fn accelerator(p: Physical) -> Option<&'static str> {
    match p {
        Physical::Shift => Some("<Shift>"),
        Physical::Ctrl => Some("<Control>"),
        Physical::Alt => Some("<Alt>"),
        Physical::Super => Some("<Super>"),
        Physical::Fn => None,
    }
}

/// A modifier name inside an accelerator's angle brackets; GTK's `Primary` is the Control key.
fn from_accel(name: &str) -> Option<Physical> {
    match name.to_ascii_lowercase().as_str() {
        "shift" => Some(Physical::Shift),
        "control" | "ctrl" | "primary" => Some(Physical::Ctrl),
        "alt" | "mod1" => Some(Physical::Alt),
        "super" | "meta" | "mod4" => Some(Physical::Super),
        _ => None,
    }
}

/// A key by its keysym name (`q`, `comma`, `Left`, `Escape`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key(String);

impl Key {
    /// `None` for an empty name or one with characters no keysym name has.
    pub fn new(name: &str) -> Option<Key> {
        (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .then(|| Key(name.to_string()))
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    /// The key as a menu shows it: a letter in upper case, punctuation as its character.
    pub fn label(&self) -> String {
        let symbol = match self.0.as_str() {
            "comma" => ",",
            "period" => ".",
            "slash" => "/",
            "backslash" => "\\",
            "semicolon" => ";",
            "apostrophe" => "'",
            "grave" => "`",
            "minus" => "-",
            "equal" => "=",
            "bracketleft" => "[",
            "bracketright" => "]",
            "Escape" => "Esc",
            name if name.chars().count() == 1 => return name.to_uppercase(),
            name => name,
        };
        symbol.to_string()
    }
}

/// A key with logical modifiers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub modifiers: Mods,
    pub key: Key,
}

impl Chord {
    /// `None` when `key` is not a keysym name.
    pub fn new(modifiers: &[Modifier], key: &str) -> Option<Chord> {
        Some(Chord {
            modifiers: modifiers.iter().copied().collect(),
            key: Key::new(key)?,
        })
    }

    /// Text such as `Shift+Super+Q`; `None` when `map` gives one of the modifiers no key.
    pub fn display(&self, map: &dyn ModifierMap) -> Option<String> {
        let mut text = String::new();
        for m in self.modifiers.iter() {
            text.push_str(map.physical(m)?.name());
            text.push('+');
        }
        text.push_str(&self.key.label());
        Some(text)
    }

    /// The accelerator of the keys that type the chord, such as `<Shift><Super>q`.
    pub fn to_accel(&self, map: &dyn ModifierMap) -> Option<String> {
        let mut accel = String::new();
        for m in self.modifiers.iter() {
            accel.push_str(accelerator(map.physical(m)?)?);
        }
        accel.push_str(self.key.name());
        Some(accel)
    }

    /// The chord an accelerator types; `None` when it is malformed or holds a key that carries no logical modifier.
    pub fn from_accel(accel: &str, map: &dyn ModifierMap) -> Option<Chord> {
        let mut rest = accel.trim();
        let mut modifiers = Mods::NONE;
        while let Some(r) = rest.strip_prefix('<') {
            let end = r.find('>')?;
            modifiers |= map.logical(from_accel(&r[..end])?)?.into();
            rest = &r[end + 1..];
        }
        Some(Chord {
            modifiers,
            key: Key::new(rest)?,
        })
    }
}
