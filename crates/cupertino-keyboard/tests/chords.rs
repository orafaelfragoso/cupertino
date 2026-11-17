//! Chord, key and named-key parsing and display.

use cupertino_keyboard::modifiers::Mods;
use cupertino_keyboard::{Chord, ChordError, Key, NamedKey};

#[test]
fn chord_parses_modifiers_and_a_named_key() {
    let c = Chord::parse("Ctrl+Alt+Left").unwrap();
    assert_eq!(c.mods, Mods::CTRL | Mods::ALT);
    assert_eq!(c.key, Key::Named(NamedKey::Left));
}

#[test]
fn chord_modifier_names_are_case_insensitive() {
    assert_eq!(
        Chord::parse("cTRL+shift+z").unwrap().mods,
        Mods::CTRL | Mods::SHIFT
    );
    assert_eq!(Chord::parse("Control+a").unwrap().mods, Mods::CTRL);
}

#[test]
fn super_and_meta_are_aliases_of_primary() {
    assert_eq!(Chord::parse("Super+Q").unwrap().mods, Mods::PRIMARY);
    assert_eq!(Chord::parse("Meta+Q").unwrap().mods, Mods::PRIMARY);
    assert_eq!(Chord::parse("Primary+Q").unwrap().mods, Mods::PRIMARY);
}

#[test]
fn chord_character_keys_are_lower_cased() {
    assert_eq!(Chord::parse("Primary+Shift+Z").unwrap().key, Key::Char('z'));
}

#[test]
fn chord_function_key_without_modifiers() {
    let c = Chord::parse("F5").unwrap();
    assert_eq!(c.mods, Mods::NONE);
    assert_eq!(c.key, Key::Named(NamedKey::F(5)));
}

#[test]
fn chord_plus_can_be_the_key() {
    let c = Chord::parse("Ctrl++").unwrap();
    assert_eq!(c.mods, Mods::CTRL);
    assert_eq!(c.key, Key::Char('+'));
    assert_eq!(Chord::parse("+").unwrap().key, Key::Char('+'));
}

#[test]
fn chord_parse_trims_surrounding_space() {
    assert_eq!(
        Chord::parse("  Ctrl+K ").unwrap(),
        Chord::parse("Ctrl+K").unwrap()
    );
}

#[test]
fn empty_chord_is_an_error() {
    assert_eq!(Chord::parse(""), Err(ChordError::Empty));
    assert_eq!(Chord::parse("   "), Err(ChordError::Empty));
}

#[test]
fn unknown_modifier_is_reported_lower_cased() {
    assert_eq!(
        Chord::parse("Hyper+A"),
        Err(ChordError::UnknownModifier("hyper".into()))
    );
}

#[test]
fn unknown_multi_character_key_is_reported() {
    assert_eq!(
        Chord::parse("Ctrl+Foo"),
        Err(ChordError::UnknownKey("Foo".into()))
    );
}

#[test]
fn chord_error_messages_name_the_problem() {
    assert_eq!(ChordError::Empty.to_string(), "empty chord");
    assert_eq!(
        ChordError::UnknownModifier("x".into()).to_string(),
        "unknown modifier 'x'"
    );
    assert_eq!(
        ChordError::UnknownKey("y".into()).to_string(),
        "unknown key 'y'"
    );
}

#[test]
fn chord_displays_modifiers_then_key() {
    let c = Chord::new(Mods::PRIMARY | Mods::SHIFT, Key::Char('z'));
    assert_eq!(c.to_string(), "Primary+Shift+Z");
    assert_eq!(
        Chord::new(Mods::NONE, Key::Named(NamedKey::Tab)).to_string(),
        "Tab"
    );
}

#[test]
fn displayed_chords_parse_back() {
    for text in [
        "Primary+Ctrl+Alt+Shift+Fn+Left",
        "Ctrl+Space",
        "Shift+F12",
        "Primary+,",
    ] {
        let c = Chord::parse(text).unwrap();
        assert_eq!(Chord::parse(&c.to_string()).unwrap(), c, "{text}");
    }
}

#[test]
fn named_keys_parse_case_insensitively_with_aliases() {
    assert_eq!(NamedKey::parse("pageup"), Some(NamedKey::PageUp));
    assert_eq!(NamedKey::parse("Enter"), Some(NamedKey::Return));
    assert_eq!(NamedKey::parse("Esc"), Some(NamedKey::Escape));
    assert_eq!(NamedKey::parse("nothing"), None);
}

#[test]
fn function_keys_run_from_one_to_thirty_five() {
    assert_eq!(NamedKey::parse("f1"), Some(NamedKey::F(1)));
    assert_eq!(NamedKey::parse("F35"), Some(NamedKey::F(35)));
    assert_eq!(NamedKey::parse("F0"), None);
    assert_eq!(NamedKey::parse("F36"), None);
}

#[test]
fn named_key_names_use_the_canonical_spelling() {
    assert_eq!(NamedKey::Return.name(), "Return");
    assert_eq!(NamedKey::Escape.name(), "Escape");
    assert_eq!(NamedKey::F(12).name(), "F12");
}

#[test]
fn character_keys_display_upper_case() {
    assert_eq!(Key::Char('q').to_string(), "Q");
    assert_eq!(Key::Named(NamedKey::Home).to_string(), "Home");
}
