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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_keys_show_their_character() {
        for (name, shown) in [
            ("bracketleft", "["),
            ("bracketright", "]"),
            ("backslash", "\\"),
            ("slash", "/"),
            ("minus", "-"),
            ("equal", "="),
            ("q", "Q"),
            ("Up", "Up"),
        ] {
            assert_eq!(Key::new(name).unwrap().label(), shown, "{name}");
        }
    }
    use cupertino_modifiers::Profile;
    use Modifier::*;

    /// A profile in which no key carries Ctrl.
    struct NoCtrl;

    impl ModifierMap for NoCtrl {
        fn physical(&self, m: Modifier) -> Option<Physical> {
            Profile::classic_pc().physical(m).filter(|_| m != Ctrl)
        }

        fn logical(&self, p: Physical) -> Option<Modifier> {
            Profile::classic_pc().logical(p).filter(|m| *m != Ctrl)
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
            assert_eq!(c.display(&Profile::cupertino()).as_deref(), Some(cupertino));
            assert_eq!(c.display(&Profile::classic_pc()).as_deref(), Some(classic));
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
                .display(&Profile::cupertino())
                .as_deref(),
            Some("Shift+Super+Alt+Z")
        );
    }

    #[test]
    fn accelerators_name_the_physical_keys_of_the_profile() {
        let c = chord(&[Shift, Primary, Ctrl], "f");
        assert_eq!(
            c.to_accel(&Profile::cupertino()).as_deref(),
            Some("<Shift><Super><Control>f")
        );
        assert_eq!(
            c.to_accel(&Profile::classic_pc()).as_deref(),
            Some("<Shift><Control><Super>f")
        );
        for profile in [Profile::cupertino(), Profile::classic_pc()] {
            assert_eq!(
                Chord::from_accel(&c.to_accel(&profile).unwrap(), &profile),
                Some(c.clone())
            );
        }
    }

    #[test]
    fn a_gtk_primary_accelerator_is_the_control_key() {
        assert_eq!(
            Chord::from_accel("<Primary>q", &Profile::cupertino()),
            Some(chord(&[Ctrl], "q"))
        );
        assert_eq!(
            Chord::from_accel("<Primary>q", &Profile::classic_pc()),
            Some(chord(&[Primary], "q"))
        );
        assert_eq!(
            Chord::from_accel("<primary><SHIFT>z", &Profile::classic_pc()),
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
                Chord::from_accel(accel, &Profile::cupertino()),
                None,
                "{accel:?}"
            );
        }
        assert_eq!(Chord::new(&[Primary], "<Super>q"), None);
    }
}
