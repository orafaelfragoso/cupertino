//! Logical modifiers, keys and chords.
//!
//! A [`Chord`] is expressed with *logical* modifiers: [`cupertino_modifiers::Mods::PRIMARY`] is the modifier that carries
//! application shortcuts. Which physical key produces it is decided by a
//! [`cupertino_modifiers::Profile`].

use std::fmt;

use cupertino_modifiers::Mods;

/// Keys that have no character of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NamedKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Return,
    KpEnter,
    Tab,
    Escape,
    Space,
    Clear,
    /// Function key F1..F35.
    F(u8),
    Globe,
    Power,
    Help,
    LauncherKey,
    OverviewKey,
    DictationKey,
}

impl NamedKey {
    const NAMES: &'static [(&'static str, NamedKey)] = &[
        ("Up", NamedKey::Up),
        ("Down", NamedKey::Down),
        ("Left", NamedKey::Left),
        ("Right", NamedKey::Right),
        ("Home", NamedKey::Home),
        ("End", NamedKey::End),
        ("PageUp", NamedKey::PageUp),
        ("PageDown", NamedKey::PageDown),
        ("Backspace", NamedKey::Backspace),
        ("Delete", NamedKey::Delete),
        ("Return", NamedKey::Return),
        ("Enter", NamedKey::Return),
        ("KpEnter", NamedKey::KpEnter),
        ("Tab", NamedKey::Tab),
        ("Escape", NamedKey::Escape),
        ("Esc", NamedKey::Escape),
        ("Space", NamedKey::Space),
        ("Clear", NamedKey::Clear),
        ("Globe", NamedKey::Globe),
        ("Power", NamedKey::Power),
        ("Help", NamedKey::Help),
        ("LauncherKey", NamedKey::LauncherKey),
        ("OverviewKey", NamedKey::OverviewKey),
        ("DictationKey", NamedKey::DictationKey),
    ];

    pub fn name(self) -> String {
        match self {
            NamedKey::F(n) => format!("F{n}"),
            other => Self::NAMES
                .iter()
                .find(|(_, k)| *k == other)
                .map(|(n, _)| (*n).to_string())
                .unwrap_or_default(),
        }
    }

    pub fn parse(s: &str) -> Option<NamedKey> {
        if let Some(rest) = s.strip_prefix(['F', 'f']) {
            if let Ok(n) = rest.parse::<u8>() {
                if (1..=35).contains(&n) {
                    return Some(NamedKey::F(n));
                }
            }
        }
        Self::NAMES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(s))
            .map(|(_, k)| *k)
    }
}

/// The key of a chord: a (lower-case, unshifted) character or a named key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Char(char),
    Named(NamedKey),
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(c) => write!(f, "{}", c.to_ascii_uppercase()),
            Key::Named(n) => f.write_str(&n.name()),
        }
    }
}

/// Modifiers plus key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub mods: Mods,
    pub key: Key,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordError {
    Empty,
    UnknownModifier(String),
    UnknownKey(String),
}

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChordError::Empty => f.write_str("empty chord"),
            ChordError::UnknownModifier(m) => write!(f, "unknown modifier '{m}'"),
            ChordError::UnknownKey(k) => write!(f, "unknown key '{k}'"),
        }
    }
}

impl std::error::Error for ChordError {}

impl Chord {
    pub const fn new(mods: Mods, key: Key) -> Chord {
        Chord { mods, key }
    }

    /// Parses `Ctrl+Alt+Left`, `Primary+Shift+Z`, `F5`, `Ctrl++`. Modifier names are case-insensitive;
    /// `Super` and `Meta` are accepted as aliases of `Primary`.
    pub fn parse(s: &str) -> Result<Chord, ChordError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ChordError::Empty);
        }
        let mut parts: Vec<&str> = Vec::new();
        let mut rest = s;
        loop {
            if let Some(idx) = rest.find('+') {
                if idx == 0 {
                    parts.push("+");
                    break;
                }
                parts.push(&rest[..idx]);
                rest = &rest[idx + 1..];
                if rest.is_empty() {
                    parts.push("+");
                    break;
                }
            } else {
                parts.push(rest);
                break;
            }
        }
        let key_s = parts.pop().ok_or(ChordError::Empty)?;
        let mut mods = Mods::NONE;
        for p in parts {
            mods |= match p.trim().to_ascii_lowercase().as_str() {
                "shift" => Mods::SHIFT,
                "ctrl" | "control" => Mods::CTRL,
                "alt" => Mods::ALT,
                "primary" | "super" | "meta" => Mods::PRIMARY,
                "fn" => Mods::FN,
                other => return Err(ChordError::UnknownModifier(other.to_string())),
            };
        }
        let key = if let Some(n) = NamedKey::parse(key_s) {
            Key::Named(n)
        } else {
            let mut it = key_s.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Key::Char(c.to_lowercase().next().unwrap_or(c)),
                _ => return Err(ChordError::UnknownKey(key_s.to_string())),
            }
        };
        Ok(Chord { mods, key })
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.is_empty() {
            write!(f, "{}", self.key)
        } else {
            write!(f, "{}+{}", self.mods, self.key)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display_roundtrip() {
        let c = Chord::parse("ctrl+Alt+left").unwrap();
        assert_eq!(c.mods, Mods::CTRL | Mods::ALT);
        assert_eq!(c.key, Key::Named(NamedKey::Left));
        assert_eq!(c.to_string(), "Ctrl+Alt+Left");
        assert_eq!(Chord::parse(&c.to_string()).unwrap(), c);
    }

    #[test]
    fn plus_key_and_function_keys() {
        let c = Chord::parse("Ctrl++").unwrap();
        assert_eq!(c.key, Key::Char('+'));
        assert_eq!(
            Chord::parse("F12").unwrap().key,
            Key::Named(NamedKey::F(12))
        );
        assert!(Chord::parse("F99").is_err());
    }

    #[test]
    fn errors() {
        assert_eq!(Chord::parse(""), Err(ChordError::Empty));
        assert!(matches!(
            Chord::parse("Hyper+A"),
            Err(ChordError::UnknownModifier(_))
        ));
        assert!(matches!(
            Chord::parse("Ctrl+Nonsense"),
            Err(ChordError::UnknownKey(_))
        ));
    }

    #[test]
    fn mods_ops() {
        let m = Mods::SHIFT | Mods::PRIMARY;
        assert!(m.contains(Mods::SHIFT));
        assert!(!m.contains(Mods::ALT));
        assert_eq!(m.without(Mods::SHIFT), Mods::PRIMARY);
        assert_eq!(m.to_string(), "Primary+Shift");
    }
}
