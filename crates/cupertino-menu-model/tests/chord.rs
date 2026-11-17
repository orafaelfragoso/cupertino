//! Keys and chords: menu text and GTK accelerators through a modifier map.

use cupertino_keyboard::modifiers::{Modifier, ModifierMap, Mods, Physical, Profile};
use cupertino_menu_model::{Chord, Key};

fn chord(modifiers: &[Modifier], key: &str) -> Chord {
    Chord::new(modifiers, key).expect("a keysym name")
}

/// A map in which no key carries Alt.
struct NoAlt;

impl ModifierMap for NoAlt {
    fn physical(&self, m: Modifier) -> Option<Physical> {
        Profile::cupertino()
            .physical(m)
            .filter(|_| m != Modifier::Alt)
    }

    fn logical(&self, p: Physical) -> Option<Modifier> {
        Profile::cupertino()
            .logical(p)
            .filter(|m| *m != Modifier::Alt)
    }
}

#[test]
fn a_key_is_a_keysym_name() {
    for name in ["q", "comma", "Left", "F5", "KP_Enter", "7"] {
        assert_eq!(Key::new(name).expect(name).name(), name);
    }
}

#[test]
fn an_empty_or_malformed_key_name_is_rejected() {
    for name in ["", "a-b", "a b", "a+b", "<q>", "é", "a.b"] {
        assert!(Key::new(name).is_none(), "{name:?}");
    }
}

#[test]
fn letters_are_labelled_in_upper_case() {
    assert_eq!(Key::new("q").unwrap().label(), "Q");
    assert_eq!(Key::new("7").unwrap().label(), "7");
}

#[test]
fn punctuation_names_are_labelled_with_their_character() {
    let cases = [
        ("comma", ","),
        ("period", "."),
        ("slash", "/"),
        ("backslash", "\\"),
        ("semicolon", ";"),
        ("apostrophe", "'"),
        ("grave", "`"),
        ("minus", "-"),
        ("equal", "="),
        ("bracketleft", "["),
        ("bracketright", "]"),
    ];
    for (name, label) in cases {
        assert_eq!(Key::new(name).unwrap().label(), label, "{name}");
    }
}

#[test]
fn escape_is_labelled_esc_and_other_names_are_kept() {
    assert_eq!(Key::new("Escape").unwrap().label(), "Esc");
    assert_eq!(Key::new("Left").unwrap().label(), "Left");
    assert_eq!(Key::new("F5").unwrap().label(), "F5");
}

#[test]
fn a_chord_needs_a_valid_key() {
    assert!(Chord::new(&[Modifier::Primary], "").is_none());
    assert!(Chord::new(&[], "a-b").is_none());
}

#[test]
fn a_chord_collects_its_modifiers_into_a_set() {
    let c = chord(&[Modifier::Alt, Modifier::Primary, Modifier::Alt], "h");
    assert_eq!(c.modifiers, Mods::ALT | Mods::PRIMARY);
    assert_eq!(chord(&[], "h").modifiers, Mods::NONE);
}

#[test]
fn display_follows_the_active_profile() {
    let c = chord(&[Modifier::Shift, Modifier::Primary], "q");
    assert_eq!(c.display(&Profile::cupertino()).unwrap(), "Shift+Super+Q");
    assert_eq!(c.display(&Profile::classic_pc()).unwrap(), "Shift+Ctrl+Q");
}

#[test]
fn display_orders_modifiers_as_a_chord_shows_them() {
    let c = chord(
        &[
            Modifier::Fn,
            Modifier::Alt,
            Modifier::Ctrl,
            Modifier::Primary,
            Modifier::Shift,
        ],
        "comma",
    );
    assert_eq!(
        c.display(&Profile::cupertino()).unwrap(),
        "Shift+Super+Ctrl+Alt+Fn+,"
    );
}

#[test]
fn a_chord_without_modifiers_displays_as_its_key() {
    assert_eq!(
        chord(&[], "Escape").display(&Profile::cupertino()).unwrap(),
        "Esc"
    );
}

#[test]
fn display_is_none_when_no_key_carries_a_modifier() {
    let c = chord(&[Modifier::Alt], "h");
    assert_eq!(c.display(&NoAlt), None);
    assert_eq!(
        chord(&[Modifier::Primary], "h").display(&NoAlt).unwrap(),
        "Super+H"
    );
}

#[test]
fn the_accelerator_names_the_physical_keys() {
    let c = chord(&[Modifier::Shift, Modifier::Primary], "q");
    assert_eq!(
        c.to_accel(&Profile::cupertino()).unwrap(),
        "<Shift><Super>q"
    );
    assert_eq!(
        c.to_accel(&Profile::classic_pc()).unwrap(),
        "<Shift><Control>q"
    );
}

#[test]
fn the_accelerator_keeps_the_keysym_name() {
    let c = chord(&[Modifier::Alt], "Left");
    assert_eq!(c.to_accel(&Profile::cupertino()).unwrap(), "<Alt>Left");
    assert_eq!(
        chord(&[], "comma").to_accel(&Profile::cupertino()).unwrap(),
        "comma"
    );
}

#[test]
fn a_modifier_gtk_has_no_accelerator_for_gives_none() {
    let c = chord(&[Modifier::Fn], "F5");
    assert_eq!(c.to_accel(&Profile::cupertino()), None);
    assert_eq!(chord(&[Modifier::Alt], "h").to_accel(&NoAlt), None);
}

#[test]
fn an_accelerator_parses_to_the_logical_chord_through_the_profile() {
    let cup = Profile::cupertino();
    let classic = Profile::classic_pc();
    assert_eq!(
        Chord::from_accel("<Shift><Super>q", &cup).unwrap(),
        chord(&[Modifier::Shift, Modifier::Primary], "q")
    );
    assert_eq!(
        Chord::from_accel("<Control>q", &classic).unwrap(),
        chord(&[Modifier::Primary], "q")
    );
    assert_eq!(
        Chord::from_accel("<Control>q", &cup).unwrap(),
        chord(&[Modifier::Ctrl], "q")
    );
}

#[test]
fn accelerators_round_trip_through_both_profiles() {
    for profile in [Profile::cupertino(), Profile::classic_pc()] {
        for modifiers in [
            vec![],
            vec![Modifier::Primary],
            vec![Modifier::Shift, Modifier::Primary],
            vec![Modifier::Primary, Modifier::Ctrl, Modifier::Alt],
        ] {
            let c = chord(&modifiers, "comma");
            let accel = c.to_accel(&profile).unwrap();
            assert_eq!(Chord::from_accel(&accel, &profile).unwrap(), c, "{accel}");
        }
    }
}

#[test]
fn accelerator_modifier_names_have_gtk_aliases() {
    let cup = Profile::cupertino();
    let primary = Chord::from_accel("<Primary>q", &cup).unwrap();
    assert_eq!(primary.modifiers, Mods::CTRL);
    for name in ["<CONTROL>q", "<ctrl>q"] {
        assert_eq!(Chord::from_accel(name, &cup).unwrap().modifiers, Mods::CTRL);
    }
    for name in ["<Mod1>q", "<alt>q"] {
        assert_eq!(Chord::from_accel(name, &cup).unwrap().modifiers, Mods::ALT);
    }
    for name in ["<Meta>q", "<Mod4>q", "<Super>q"] {
        assert_eq!(
            Chord::from_accel(name, &cup).unwrap().modifiers,
            Mods::PRIMARY
        );
    }
}

#[test]
fn a_bare_key_is_an_accelerator_without_modifiers() {
    let c = Chord::from_accel("  Escape ", &Profile::cupertino()).unwrap();
    assert_eq!(c, chord(&[], "Escape"));
}

#[test]
fn malformed_accelerators_are_rejected() {
    let cup = Profile::cupertino();
    for accel in [
        "",
        "<Shift>",
        "<Shift q",
        "<Bogus>q",
        "<Shift><>q",
        "q w",
        "<Shift>q-",
    ] {
        assert_eq!(Chord::from_accel(accel, &cup), None, "{accel:?}");
    }
}

#[test]
fn an_accelerator_with_a_key_that_carries_no_modifier_is_rejected() {
    assert_eq!(Chord::from_accel("<Alt>q", &NoAlt), None);
    assert!(Chord::from_accel("<Shift>q", &NoAlt).is_some());
}
