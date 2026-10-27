//! Chords in logical modifiers, and their accelerator and display text through a modifier map.

/// A logical modifier. The order is the order a chord shows them in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Shift,
    /// The command modifier.
    Primary,
    Ctrl,
    Alt,
}

impl Modifier {
    pub const ALL: [Modifier; 4] = [
        Modifier::Shift,
        Modifier::Primary,
        Modifier::Ctrl,
        Modifier::Alt,
    ];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of logical modifiers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);

    pub fn contains(self, m: Modifier) -> bool {
        self.0 & m.bit() != 0
    }

    pub fn with(self, m: Modifier) -> Modifiers {
        Modifiers(self.0 | m.bit())
    }

    /// The modifiers in display order.
    pub fn iter(self) -> impl Iterator<Item = Modifier> {
        Modifier::ALL.into_iter().filter(move |m| self.contains(*m))
    }
}

impl FromIterator<Modifier> for Modifiers {
    fn from_iter<I: IntoIterator<Item = Modifier>>(iter: I) -> Modifiers {
        iter.into_iter().fold(Modifiers::NONE, Modifiers::with)
    }
}

/// A modifier key as the keyboard reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhysicalModifier {
    Shift,
    Control,
    Alt,
    Super,
}

impl PhysicalModifier {
    /// The key's name in a menu.
    pub fn name(self) -> &'static str {
        match self {
            PhysicalModifier::Shift => "Shift",
            PhysicalModifier::Control => "Ctrl",
            PhysicalModifier::Alt => "Alt",
            PhysicalModifier::Super => "Super",
        }
    }

    fn accel(self) -> &'static str {
        match self {
            PhysicalModifier::Shift => "<Shift>",
            PhysicalModifier::Control => "<Control>",
            PhysicalModifier::Alt => "<Alt>",
            PhysicalModifier::Super => "<Super>",
        }
    }

    /// A modifier name inside an accelerator's angle brackets; GTK's `Primary` is the Control key.
    fn from_accel(name: &str) -> Option<PhysicalModifier> {
        match name.to_ascii_lowercase().as_str() {
            "shift" => Some(PhysicalModifier::Shift),
            "control" | "ctrl" | "primary" => Some(PhysicalModifier::Control),
            "alt" | "mod1" => Some(PhysicalModifier::Alt),
            "super" | "meta" | "mod4" => Some(PhysicalModifier::Super),
            _ => None,
        }
    }
}

/// Which physical key carries each logical modifier: the active modifier profile.
pub trait ModifierMap {
    /// The key that carries `m`; `None` when no key does.
    fn physical(&self, m: Modifier) -> Option<PhysicalModifier>;
    /// The logical modifier `p` carries; `None` when it carries none.
    fn logical(&self, p: PhysicalModifier) -> Option<Modifier>;
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
            "semicolon" => ";",
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
    pub modifiers: Modifiers,
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
            accel.push_str(map.physical(m)?.accel());
        }
        accel.push_str(self.key.name());
        Some(accel)
    }

    /// The chord an accelerator types; `None` when it is malformed or holds a key that carries no logical modifier.
    pub fn from_accel(accel: &str, map: &dyn ModifierMap) -> Option<Chord> {
        let mut rest = accel.trim();
        let mut modifiers = Modifiers::NONE;
        while let Some(r) = rest.strip_prefix('<') {
            let end = r.find('>')?;
            modifiers = modifiers.with(map.logical(PhysicalModifier::from_accel(&r[..end])?)?);
            rest = &r[end + 1..];
        }
        Some(Chord {
            modifiers,
            key: Key::new(rest)?,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use Modifier::*;
    use PhysicalModifier::{Control, Super};

    /// The two profiles of the menu bar spec: `cupertino` (Super is the command key) and `classic-pc` (a full swap).
    pub(crate) enum Profile {
        Cupertino,
        ClassicPc,
    }

    impl ModifierMap for Profile {
        fn physical(&self, m: Modifier) -> Option<PhysicalModifier> {
            Some(match (self, m) {
                (_, Shift) => PhysicalModifier::Shift,
                (_, Alt) => PhysicalModifier::Alt,
                (Profile::Cupertino, Primary) | (Profile::ClassicPc, Ctrl) => Super,
                (Profile::Cupertino, Ctrl) | (Profile::ClassicPc, Primary) => Control,
            })
        }

        fn logical(&self, p: PhysicalModifier) -> Option<Modifier> {
            Modifier::ALL
                .into_iter()
                .find(|m| self.physical(*m) == Some(p))
        }
    }

    /// A profile in which no key carries Ctrl.
    struct NoCtrl;

    impl ModifierMap for NoCtrl {
        fn physical(&self, m: Modifier) -> Option<PhysicalModifier> {
            Profile::ClassicPc.physical(m).filter(|_| m != Ctrl)
        }

        fn logical(&self, p: PhysicalModifier) -> Option<Modifier> {
            Profile::ClassicPc.logical(p).filter(|m| *m != Ctrl)
        }
    }

    fn chord(modifiers: &[Modifier], key: &str) -> Chord {
        Chord::new(modifiers, key).unwrap()
    }

    #[test]
    fn chords_show_in_the_keys_of_each_profile() {
        let cases = [
            (chord(&[Primary], "q"), "Super+Q", "Ctrl+Q"),
            (
                chord(&[Primary, Alt], "Escape"),
                "Super+Alt+Esc",
                "Ctrl+Alt+Esc",
            ),
            (chord(&[Primary, Ctrl], "q"), "Super+Ctrl+Q", "Ctrl+Super+Q"),
            (
                chord(&[Shift, Primary], "q"),
                "Shift+Super+Q",
                "Shift+Ctrl+Q",
            ),
            (chord(&[Primary, Ctrl], "f"), "Super+Ctrl+F", "Ctrl+Super+F"),
            (
                chord(&[Primary, Ctrl], "Left"),
                "Super+Ctrl+Left",
                "Ctrl+Super+Left",
            ),
            (chord(&[Primary, Alt], "s"), "Super+Alt+S", "Ctrl+Alt+S"),
            (chord(&[Primary], "comma"), "Super+,", "Ctrl+,"),
        ];
        for (c, cupertino, classic) in cases {
            assert_eq!(c.display(&Profile::Cupertino).as_deref(), Some(cupertino));
            assert_eq!(c.display(&Profile::ClassicPc).as_deref(), Some(classic));
        }
    }

    #[test]
    fn the_modifier_order_does_not_depend_on_how_a_chord_was_written() {
        assert_eq!(
            chord(&[Alt, Primary, Shift], "z"),
            chord(&[Shift, Primary, Alt], "z")
        );
        assert_eq!(
            chord(&[Alt, Primary, Shift], "z")
                .display(&Profile::Cupertino)
                .as_deref(),
            Some("Shift+Super+Alt+Z")
        );
    }

    #[test]
    fn accelerators_name_the_physical_keys_of_the_profile() {
        let c = chord(&[Shift, Primary, Ctrl], "f");
        assert_eq!(
            c.to_accel(&Profile::Cupertino).as_deref(),
            Some("<Shift><Super><Control>f")
        );
        assert_eq!(
            c.to_accel(&Profile::ClassicPc).as_deref(),
            Some("<Shift><Control><Super>f")
        );
        for profile in [Profile::Cupertino, Profile::ClassicPc] {
            assert_eq!(
                Chord::from_accel(&c.to_accel(&profile).unwrap(), &profile),
                Some(c.clone())
            );
        }
    }

    #[test]
    fn a_gtk_primary_accelerator_is_the_control_key() {
        assert_eq!(
            Chord::from_accel("<Primary>q", &Profile::Cupertino),
            Some(chord(&[Ctrl], "q"))
        );
        assert_eq!(
            Chord::from_accel("<Primary>q", &Profile::ClassicPc),
            Some(chord(&[Primary], "q"))
        );
        assert_eq!(
            Chord::from_accel("<primary><SHIFT>z", &Profile::ClassicPc),
            Some(chord(&[Shift, Primary], "z"))
        );
    }

    #[test]
    fn a_modifier_without_a_key_has_no_text() {
        let c = chord(&[Primary, Ctrl], "f");
        assert_eq!(c.display(&NoCtrl), None);
        assert_eq!(c.to_accel(&NoCtrl), None);
        assert_eq!(Chord::from_accel("<Super>f", &NoCtrl), None);
    }

    #[test]
    fn malformed_accelerators_and_key_names_are_refused() {
        for accel in [
            "",
            "<Control>",
            "<Control q",
            "<Hyper>q",
            "<Control>q w",
            "<Control>+",
        ] {
            assert_eq!(
                Chord::from_accel(accel, &Profile::Cupertino),
                None,
                "{accel:?}"
            );
        }
        assert_eq!(Chord::new(&[Primary], "<Super>q"), None);
    }
}
