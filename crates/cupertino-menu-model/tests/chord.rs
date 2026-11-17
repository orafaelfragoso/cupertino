//! Keys and chords: menu text and GTK accelerators through a modifier map.

use cupertino_keyboard::modifiers::{Modifier, ModifierMap, Mods, Physical, Profile};
use cupertino_menu_model::{Chord, Key};
use Modifier::{Alt, Ctrl, Fn as FnKey, Primary, Shift};

fn chord(modifiers: &[Modifier], key: &str) -> Chord {
    Chord::new(modifiers, key).expect("a keysym name")
}

/// A map in which no key carries Alt.
struct NoAlt;

impl ModifierMap for NoAlt {
    fn physical(&self, m: Modifier) -> Option<Physical> {
        Profile::cupertino().physical(m).filter(|_| m != Alt)
    }

    fn logical(&self, p: Physical) -> Option<Modifier> {
        Profile::cupertino().logical(p).filter(|m| *m != Alt)
    }
}

#[test]
fn a_key_is_a_keysym_name_labelled_as_menus_show_it() {
    let cases = [
        ("q", "Q"),
        ("7", "7"),
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
        ("Escape", "Esc"),
        ("Left", "Left"),
        ("F5", "F5"),
        ("KP_Enter", "KP_Enter"),
    ];
    for (name, label) in cases {
        let key = Key::new(name).expect(name);
        assert_eq!(key.name(), name);
        assert_eq!(key.label(), label, "{name}");
    }
    for name in ["", "a-b", "a b", "a+b", "<q>", "é", "a.b"] {
        assert!(Key::new(name).is_none(), "{name:?}");
        assert!(Chord::new(&[Primary], name).is_none(), "{name:?}");
    }
}

#[test]
fn a_chord_collects_its_modifiers_into_a_set() {
    assert_eq!(
        chord(&[Alt, Primary, Alt], "h").modifiers,
        Mods::ALT | Mods::PRIMARY
    );
    assert_eq!(chord(&[], "h").modifiers, Mods::NONE);
}

#[test]
fn a_chord_displays_through_the_map_in_menu_order() {
    let cup = Profile::cupertino();
    let classic = Profile::classic_pc();
    let cases: [(Chord, &dyn ModifierMap, Option<&str>); 6] = [
        (chord(&[Shift, Primary], "q"), &cup, Some("Shift+Super+Q")),
        (
            chord(&[Shift, Primary], "q"),
            &classic,
            Some("Shift+Ctrl+Q"),
        ),
        (
            chord(&[FnKey, Alt, Ctrl, Primary, Shift], "comma"),
            &cup,
            Some("Shift+Super+Ctrl+Alt+Fn+,"),
        ),
        (chord(&[], "Escape"), &cup, Some("Esc")),
        (chord(&[Primary], "h"), &NoAlt, Some("Super+H")),
        (chord(&[Alt], "h"), &NoAlt, None),
    ];
    for (c, map, expected) in cases {
        assert_eq!(c.display(map).as_deref(), expected, "{expected:?}");
    }
}

#[test]
fn a_chord_writes_the_accelerator_of_its_physical_keys() {
    let cup = Profile::cupertino();
    let classic = Profile::classic_pc();
    let cases: [(Chord, &dyn ModifierMap, Option<&str>); 6] = [
        (chord(&[Shift, Primary], "q"), &cup, Some("<Shift><Super>q")),
        (
            chord(&[Shift, Primary], "q"),
            &classic,
            Some("<Shift><Control>q"),
        ),
        (chord(&[Alt], "Left"), &cup, Some("<Alt>Left")),
        (chord(&[], "comma"), &cup, Some("comma")),
        (chord(&[FnKey], "F5"), &cup, None),
        (chord(&[Alt], "h"), &NoAlt, None),
    ];
    for (c, map, expected) in cases {
        assert_eq!(c.to_accel(map).as_deref(), expected, "{expected:?}");
    }
}

#[test]
fn an_accelerator_parses_to_the_logical_chord_through_the_map() {
    let cup = Profile::cupertino();
    let classic = Profile::classic_pc();
    let cases: [(&str, &dyn ModifierMap, Option<Chord>); 21] = [
        ("<Shift><Super>q", &cup, Some(chord(&[Shift, Primary], "q"))),
        ("<Control>q", &classic, Some(chord(&[Primary], "q"))),
        ("<Control>q", &cup, Some(chord(&[Ctrl], "q"))),
        ("<Primary>q", &cup, Some(chord(&[Ctrl], "q"))),
        ("<CONTROL>q", &cup, Some(chord(&[Ctrl], "q"))),
        ("<ctrl>q", &cup, Some(chord(&[Ctrl], "q"))),
        ("<Mod1>q", &cup, Some(chord(&[Alt], "q"))),
        ("<alt>q", &cup, Some(chord(&[Alt], "q"))),
        ("<Meta>q", &cup, Some(chord(&[Primary], "q"))),
        ("<Mod4>q", &cup, Some(chord(&[Primary], "q"))),
        ("  Escape ", &cup, Some(chord(&[], "Escape"))),
        ("<Shift>q", &NoAlt, Some(chord(&[Shift], "q"))),
        ("<Alt>q", &NoAlt, None),
        ("", &cup, None),
        ("<Shift>", &cup, None),
        ("<Shift q", &cup, None),
        ("<Bogus>q", &cup, None),
        ("<Shift><>q", &cup, None),
        ("q w", &cup, None),
        ("<Shift>q-", &cup, None),
        ("<Super>", &cup, None),
    ];
    for (accel, map, expected) in cases {
        assert_eq!(Chord::from_accel(accel, map), expected, "{accel:?}");
    }
}

#[test]
fn accelerators_round_trip_through_both_profiles() {
    for profile in [Profile::cupertino(), Profile::classic_pc()] {
        for modifiers in [
            vec![],
            vec![Primary],
            vec![Shift, Primary],
            vec![Primary, Ctrl, Alt],
        ] {
            let c = chord(&modifiers, "comma");
            let accel = c.to_accel(&profile).unwrap();
            assert_eq!(Chord::from_accel(&accel, &profile).unwrap(), c, "{accel}");
        }
    }
}
