//! Chord, key and named-key parsing and display.

use cupertino_keyboard::modifiers::Mods;
use cupertino_keyboard::{Chord, ChordError, Key, NamedKey};

#[test]
fn chords_parse_to_modifiers_and_a_key() {
    let named = Key::Named;
    for (text, mods, key) in [
        (
            "Ctrl+Alt+Left",
            Mods::CTRL | Mods::ALT,
            named(NamedKey::Left),
        ),
        ("cTRL+shift+z", Mods::CTRL | Mods::SHIFT, Key::Char('z')),
        ("Control+a", Mods::CTRL, Key::Char('a')),
        ("Super+Q", Mods::PRIMARY, Key::Char('q')),
        ("Meta+Q", Mods::PRIMARY, Key::Char('q')),
        (
            "Primary+Shift+Z",
            Mods::PRIMARY | Mods::SHIFT,
            Key::Char('z'),
        ),
        ("F5", Mods::NONE, named(NamedKey::F(5))),
        ("Ctrl++", Mods::CTRL, Key::Char('+')),
        ("+", Mods::NONE, Key::Char('+')),
        ("  Ctrl+K ", Mods::CTRL, Key::Char('k')),
    ] {
        assert_eq!(
            Chord::parse(text),
            Ok(Chord::new(mods, key)),
            "chord {text:?}"
        );
    }
}

#[test]
fn malformed_chords_report_what_is_wrong() {
    for (text, err) in [
        ("", ChordError::Empty),
        ("   ", ChordError::Empty),
        ("Hyper+A", ChordError::UnknownModifier("hyper".into())),
        ("Ctrl+Foo", ChordError::UnknownKey("Foo".into())),
    ] {
        assert_eq!(Chord::parse(text), Err(err), "chord {text:?}");
    }
}

#[test]
fn chords_display_modifiers_then_key_and_parse_back() {
    for (chord, text) in [
        (
            Chord::new(Mods::PRIMARY | Mods::SHIFT, Key::Char('z')),
            "Primary+Shift+Z",
        ),
        (Chord::new(Mods::NONE, Key::Named(NamedKey::Tab)), "Tab"),
        (Chord::new(Mods::NONE, Key::Named(NamedKey::Home)), "Home"),
    ] {
        assert_eq!(chord.to_string(), text);
    }
    for text in [
        "Primary+Ctrl+Alt+Shift+Fn+Left",
        "Ctrl+Space",
        "Shift+F12",
        "Primary+,",
        "Return",
        "Escape",
    ] {
        let c = Chord::parse(text).unwrap();
        assert_eq!(c.to_string(), text, "display of {text:?}");
        assert_eq!(
            Chord::parse(&c.to_string()),
            Ok(c),
            "round trip of {text:?}"
        );
    }
}

#[test]
fn named_keys_parse_case_insensitively_with_aliases_and_f1_to_f35() {
    for (text, key) in [
        ("pageup", Some(NamedKey::PageUp)),
        ("Enter", Some(NamedKey::Return)),
        ("Esc", Some(NamedKey::Escape)),
        ("f1", Some(NamedKey::F(1))),
        ("F35", Some(NamedKey::F(35))),
        ("F0", None),
        ("F36", None),
        ("nothing", None),
    ] {
        assert_eq!(NamedKey::parse(text), key, "key {text:?}");
    }
}
