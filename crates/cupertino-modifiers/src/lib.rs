//! Modifier keys: the logical roles a shortcut is written in, the physical keys that carry them, and the profile that
//! says which key carries which role.
//!
//! A shortcut is written once, in logical modifiers (`Primary+Q`). The keyboard reports physical keys (Super, Ctrl,
//! Alt, Shift, Fn); the profile translates them. Every profile is a swap: each physical key carries one role and each
//! role has one key, so every chord is reachable under every profile.

use std::fmt;

/// A logical modifier, in the order a chord shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Shift,
    /// The command modifier: the one that carries shortcuts.
    Primary,
    Ctrl,
    Alt,
    Fn,
}

impl Modifier {
    pub const ALL: [Modifier; 5] = [
        Modifier::Shift,
        Modifier::Primary,
        Modifier::Ctrl,
        Modifier::Alt,
        Modifier::Fn,
    ];

    const fn mods(self) -> Mods {
        match self {
            Modifier::Shift => Mods::SHIFT,
            Modifier::Primary => Mods::PRIMARY,
            Modifier::Ctrl => Mods::CTRL,
            Modifier::Alt => Mods::ALT,
            Modifier::Fn => Mods::FN,
        }
    }
}

/// A set of logical modifiers.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const SHIFT: Mods = Mods(1);
    pub const CTRL: Mods = Mods(2);
    pub const ALT: Mods = Mods(4);
    pub const PRIMARY: Mods = Mods(8);
    pub const FN: Mods = Mods(16);

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn union(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }

    pub const fn without(self, other: Mods) -> Mods {
        Mods(self.0 & !other.0)
    }

    /// The modifiers in the order a chord shows them.
    pub fn iter(self) -> impl Iterator<Item = Modifier> {
        Modifier::ALL
            .into_iter()
            .filter(move |m| self.contains(m.mods()))
    }
}

impl From<Modifier> for Mods {
    fn from(m: Modifier) -> Mods {
        m.mods()
    }
}

impl FromIterator<Modifier> for Mods {
    fn from_iter<I: IntoIterator<Item = Modifier>>(iter: I) -> Mods {
        iter.into_iter().fold(Mods::NONE, |all, m| all | m.mods())
    }
}

impl std::ops::BitOr for Mods {
    type Output = Mods;
    fn bitor(self, rhs: Mods) -> Mods {
        self.union(rhs)
    }
}

impl std::ops::BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Mods) {
        *self = self.union(rhs);
    }
}

impl fmt::Debug for Mods {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Mods({self})")
    }
}

impl fmt::Display for Mods {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (m, name) in [
            (Mods::PRIMARY, "Primary"),
            (Mods::CTRL, "Ctrl"),
            (Mods::ALT, "Alt"),
            (Mods::SHIFT, "Shift"),
            (Mods::FN, "Fn"),
        ] {
            if self.contains(m) {
                if !first {
                    f.write_str("+")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        if first {
            f.write_str("none")?;
        }
        Ok(())
    }
}

/// A modifier key as the keyboard reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Physical {
    Shift,
    Ctrl,
    Alt,
    Super,
    Fn,
}

impl Physical {
    pub const ALL: [Physical; 5] = [
        Physical::Shift,
        Physical::Ctrl,
        Physical::Alt,
        Physical::Super,
        Physical::Fn,
    ];

    /// The key's name in a menu.
    pub fn name(self) -> &'static str {
        match self {
            Physical::Shift => "Shift",
            Physical::Ctrl => "Ctrl",
            Physical::Alt => "Alt",
            Physical::Super => "Super",
            Physical::Fn => "Fn",
        }
    }
}

/// A set of physical modifier keys that are down.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RawMods(pub u8);

impl RawMods {
    pub const SHIFT: RawMods = RawMods(1);
    pub const CTRL: RawMods = RawMods(2);
    pub const ALT: RawMods = RawMods(4);
    pub const SUPER: RawMods = RawMods(8);
    pub const FN: RawMods = RawMods(16);

    pub fn contains(self, o: RawMods) -> bool {
        self.0 & o.0 == o.0
    }
}

impl std::ops::BitOr for RawMods {
    type Output = RawMods;
    fn bitor(self, rhs: RawMods) -> RawMods {
        RawMods(self.0 | rhs.0)
    }
}

impl From<Physical> for RawMods {
    fn from(p: Physical) -> RawMods {
        match p {
            Physical::Shift => RawMods::SHIFT,
            Physical::Ctrl => RawMods::CTRL,
            Physical::Alt => RawMods::ALT,
            Physical::Super => RawMods::SUPER,
            Physical::Fn => RawMods::FN,
        }
    }
}

/// Which physical key carries each logical modifier: the active modifier profile, as callers that only format or parse
/// chords see it.
pub trait ModifierMap {
    /// The key that carries `m`; `None` when no key does.
    fn physical(&self, m: Modifier) -> Option<Physical>;
    /// The logical modifier `p` carries; `None` when it carries none.
    fn logical(&self, p: Physical) -> Option<Modifier>;
}

/// A mapping of the physical modifier keys to logical roles: a permutation, so each role has exactly one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: &'static str,
    shift: Modifier,
    ctrl: Modifier,
    alt: Modifier,
    sup: Modifier,
    fnk: Modifier,
    /// Whether the alt keys also select the third symbol level of the keymap (character composition).
    pub alt_selects_symbols: bool,
}

impl Profile {
    /// Super carries shortcuts (the Primary role), Alt is the alternate / symbol modifier, Ctrl keeps its own role for
    /// the text-editing commands.
    pub fn cupertino() -> Profile {
        Profile {
            name: "cupertino",
            shift: Modifier::Shift,
            ctrl: Modifier::Ctrl,
            alt: Modifier::Alt,
            sup: Modifier::Primary,
            fnk: Modifier::Fn,
            alt_selects_symbols: true,
        }
    }

    /// The conventional PC layout: Ctrl carries shortcuts, and Super takes the role Ctrl has under `cupertino`, so a
    /// chord that needs Primary and Ctrl together is still typed with two keys.
    pub fn classic_pc() -> Profile {
        Profile {
            name: "classic-pc",
            shift: Modifier::Shift,
            ctrl: Modifier::Primary,
            alt: Modifier::Alt,
            sup: Modifier::Ctrl,
            fnk: Modifier::Fn,
            alt_selects_symbols: false,
        }
    }

    pub fn by_name(name: &str) -> Option<Profile> {
        match name {
            "cupertino" => Some(Self::cupertino()),
            "classic-pc" => Some(Self::classic_pc()),
            _ => None,
        }
    }

    /// The logical role of one physical key.
    pub fn role(&self, p: Physical) -> Modifier {
        match p {
            Physical::Shift => self.shift,
            Physical::Ctrl => self.ctrl,
            Physical::Alt => self.alt,
            Physical::Super => self.sup,
            Physical::Fn => self.fnk,
        }
    }

    /// Logical modifiers for a physical state.
    pub fn translate(&self, raw: RawMods) -> Mods {
        Physical::ALL
            .into_iter()
            .filter(|p| raw.contains((*p).into()))
            .map(|p| self.role(p))
            .collect()
    }

    /// The key that carries the command role: Super under `cupertino`, Ctrl under `classic-pc`.
    pub fn primary(&self) -> Physical {
        self.physical_for(Modifier::Primary)
    }

    /// The key that carries `m`.
    pub fn physical_for(&self, m: Modifier) -> Physical {
        Physical::ALL
            .into_iter()
            .find(|p| self.role(*p) == m)
            .expect("a profile gives every role a key")
    }

    /// The name of the key that carries `m`, as a menu or the settings show it (`Super` for Primary under `cupertino`,
    /// `Ctrl` under `classic-pc`).
    pub fn display(&self, m: Modifier) -> &'static str {
        self.physical_for(m).name()
    }
}

impl ModifierMap for Profile {
    fn physical(&self, m: Modifier) -> Option<Physical> {
        Some(self.physical_for(m))
    }

    fn logical(&self, p: Physical) -> Option<Modifier> {
        Some(self.role(p))
    }
}
