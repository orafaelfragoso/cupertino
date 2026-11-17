//! Shortcut maps bind key combinations to actions, are written in a small text format and ship with two default sets.

use cupertino_input::keysym::{ISO_LEFT_TAB, TAB};
use cupertino_input::{
    names_super, Action, KeyCombo, Modifiers, ParseError, ScreenshotRequest, ShortcutMap,
};
use cupertino_keyboard::modifiers::Physical;

fn mods(shift: bool, ctrl: bool, alt: bool, sup: bool) -> Modifiers {
    Modifiers::new(shift, ctrl, alt, sup)
}

const SUPER: Modifiers = Modifiers::new(false, false, false, true);
const CTRL: Modifiers = Modifiers::new(false, true, false, false);

fn combo(text: &str) -> KeyCombo {
    text.parse().unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn bound(m: &ShortcutMap) -> Vec<String> {
    m.bindings().map(|(c, _)| c.to_string()).collect()
}

#[test]
fn actions_are_written_by_name_and_parse_back_or_name_what_was_wrong() {
    let mut actions = vec![
        Action::CloseWindow,
        Action::MinimizeOrRestoreWindow,
        Action::SwitchAppReverse,
        Action::TileFill,
        Action::Overview,
        Action::WorkspaceNext,
        Action::Workspace(7),
        Action::MoveWindowToWorkspace(3),
        Action::Launcher,
        Action::ToggleStage,
        Action::PowerDialog,
        Action::InputSourceNext,
        Action::Exec("foot --title a b".into()),
    ];
    actions.extend(ScreenshotRequest::ALL.map(Action::Screenshot));
    for action in actions {
        let name = action.name();
        assert_eq!(name.parse::<Action>().as_ref(), Ok(&action), "{name}");
        assert_eq!(action.to_string(), name);
    }
    for (text, expected) in [
        ("", Err(ParseError::Empty)),
        ("   ", Err(ParseError::Empty)),
        (
            "frobnicate",
            Err(ParseError::UnknownAction("frobnicate".into())),
        ),
        ("workspace:x", Err(ParseError::BadNumber("x".into()))),
        ("workspace:-1", Err(ParseError::BadNumber("-1".into()))),
        ("workspace: 4", Ok(Action::Workspace(4))),
        ("exec:  ls -l ", Ok(Action::Exec("ls -l".into()))),
    ] {
        assert_eq!(text.parse::<Action>(), expected, "{text:?}");
    }
}

#[test]
fn combinations_parse_in_either_spelling_or_name_the_offending_part() {
    let ctrl_alt_q = KeyCombo::new(mods(false, true, true, false), u32::from(b'q'));
    let super_plus = KeyCombo::new(SUPER, 0x2b);
    for (text, expected) in [
        ("Ctrl+Alt+q", Ok(ctrl_alt_q)),
        ("<Ctrl><Alt>q", Ok(ctrl_alt_q)),
        ("control+option+Q", Ok(ctrl_alt_q)),
        ("super+a", Ok(KeyCombo::new(SUPER, u32::from(b'a')))),
        ("logo+a", Ok(KeyCombo::new(SUPER, u32::from(b'a')))),
        ("win+a", Ok(KeyCombo::new(SUPER, u32::from(b'a')))),
        ("windows+a", Ok(KeyCombo::new(SUPER, u32::from(b'a')))),
        ("mod4+a", Ok(KeyCombo::new(SUPER, u32::from(b'a')))),
        ("Super+Q", Ok(KeyCombo::new(SUPER, u32::from(b'q')))),
        ("Super++", Ok(super_plus)),
        ("+", Ok(KeyCombo::new(Modifiers::NONE, 0x2b))),
        (
            "Shift+Tab",
            Ok(KeyCombo::new(Modifiers::NONE, ISO_LEFT_TAB)),
        ),
        ("", Err(ParseError::Empty)),
        ("Hyper+a", Err(ParseError::UnknownModifier("Hyper".into()))),
        ("<Hyper>a", Err(ParseError::UnknownModifier("Hyper".into()))),
        ("<Ctrl", Err(ParseError::UnknownModifier("<Ctrl".into()))),
        (
            "Ctrl+Nonsense",
            Err(ParseError::UnknownKey("Nonsense".into())),
        ),
        ("<Ctrl>", Err(ParseError::MissingKey)),
    ] {
        assert_eq!(text.parse::<KeyCombo>(), expected, "{text:?}");
    }
    let shift_tab = KeyCombo::new(Modifiers::NONE, ISO_LEFT_TAB);
    assert_eq!(
        (shift_tab.keysym, shift_tab.modifiers.shift),
        (TAB, true),
        "Shift+Tab has one spelling"
    );
}

#[test]
fn combinations_print_in_a_fixed_order_and_parse_back() {
    for (text, printed) in [
        ("Ctrl+Alt+Shift+Super+Left", "Ctrl+Alt+Shift+Super+Left"),
        ("Super+space", "Super+space"),
        ("F11", "F11"),
        ("Ctrl+XF86PowerOff", "Ctrl+XF86PowerOff"),
        ("Alt+1", "Alt+1"),
        ("<Super><Ctrl>x", "Ctrl+Super+x"),
    ] {
        assert_eq!(combo(text).to_string(), printed, "{text}");
    }
    let unnamed = KeyCombo::new(Modifiers::NONE, 0x1234_5678);
    assert_eq!(unnamed.to_string(), "0x12345678");
    assert_eq!(unnamed.to_string().parse::<KeyCombo>(), Ok(unnamed));
}

#[test]
fn the_primary_modifier_is_the_profiles_command_key() {
    assert!(combo("Primary+c").modifiers.super_key);
    assert!(
        KeyCombo::parse_with("Primary+c", Physical::Ctrl)
            .unwrap()
            .modifiers
            .ctrl
    );
    for (profile, modifiers) in [("cupertino", SUPER), ("classic-pc", CTRL)] {
        let mut m = ShortcutMap::new();
        m.apply_config_for("Primary+c = lock\n", profile);
        assert_eq!(
            m.lookup(modifiers, u32::from(b'c')),
            Some(&Action::Lock),
            "{profile}"
        );
    }
}

#[test]
fn a_lookup_needs_the_exact_modifiers_and_folds_letters_and_shift_tab() {
    let mut m = ShortcutMap::new();
    m.bind(combo("Ctrl+a"), Action::AppGrid);
    m.bind(combo("Super+q"), Action::Lock);
    m.bind(combo("Shift+Tab"), Action::SwitchAppReverse);
    for (modifiers, keysym, expected) in [
        (CTRL, u32::from(b'a'), Some(Action::AppGrid)),
        (mods(true, true, false, false), u32::from(b'a'), None),
        (mods(false, true, false, true), u32::from(b'a'), None),
        (SUPER, u32::from(b'q'), Some(Action::Lock)),
        (SUPER, u32::from(b'Q'), Some(Action::Lock)),
        (Modifiers::NONE, u32::from(b'q'), None),
        (
            Modifiers::NONE,
            ISO_LEFT_TAB,
            Some(Action::SwitchAppReverse),
        ),
        (
            mods(true, false, false, false),
            TAB,
            Some(Action::SwitchAppReverse),
        ),
    ] {
        assert_eq!(
            m.lookup(modifiers, keysym),
            expected.as_ref(),
            "{modifiers:?} {keysym:#x}"
        );
    }
}

#[test]
fn binding_replaces_in_place_and_unbinding_removes() {
    let mut m = ShortcutMap::new();
    assert!(m.is_empty());
    assert_eq!(m.bind(combo("F5"), Action::Overview), None);
    m.bind(combo("F1"), Action::Lock);
    m.bind(combo("F3"), Action::AppGrid);
    assert_eq!(m.bind(combo("F1"), Action::Launcher), Some(Action::Lock));
    assert_eq!(bound(&m), ["F5", "F1", "F3"], "binding order is kept");
    assert_eq!(m.unbind(combo("F1")), Some(Action::Launcher));
    assert_eq!(m.unbind(combo("F1")), None);
    assert_eq!(m.len(), 2);
    assert_eq!(m.combo_for(&Action::AppGrid), Some(combo("F3")));
    assert_eq!(m.combo_for(&Action::Launcher), None);
    assert_eq!(m.bind_str("Ctrl+z", Action::HideApp), Ok(None));
    assert_eq!(
        m.bind_str("Ctrl+nokey", Action::HideApp),
        Err(ParseError::UnknownKey("nokey".into()))
    );
    assert_eq!(m.len(), 3);
}

#[test]
fn the_tap_binding_is_separate_from_the_combinations_and_only_super_can_be_tapped() {
    let mut m = ShortcutMap::new();
    assert_eq!(m.tap(), None);
    assert_eq!(m.bind_tap(Action::Launcher), None);
    assert_eq!(m.bind_tap(Action::Overview), Some(Action::Launcher));
    assert_eq!(m.tap(), Some(&Action::Overview));
    assert!(m.is_empty());
    assert_eq!(m.unbind_tap(), Some(Action::Overview));
    assert_eq!(m.tap(), None);

    let errors = m.apply_config("Super = launcher\nCtrl = overview\n");
    assert_eq!(errors, vec![(2, ParseError::TapNotSuper("Ctrl".into()))]);
    assert_eq!(m.tap(), Some(&Action::Launcher));
    assert!(m.apply_config("Super = none\n").is_empty());
    assert_eq!(m.tap(), None);
}

#[test]
fn a_config_binds_unbinds_and_skips_comments() {
    let mut m = ShortcutMap::defaults();
    let before = m.len();
    let errors = m.apply_config(
        "# my shortcuts\n\
         \n\
         Super+q = lock   # a trailing comment\n\
         Super+w = none\n\
         Ctrl+Shift+e = exec:foot -e htop\n",
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(m.lookup(SUPER, u32::from(b'q')), Some(&Action::Lock));
    assert_eq!(m.lookup(SUPER, u32::from(b'w')), None);
    assert_eq!(
        m.lookup(mods(true, true, false, false), u32::from(b'e')),
        Some(&Action::Exec("foot -e htop".into()))
    );
    assert_eq!(m.len(), before);
}

#[test]
fn config_errors_carry_their_line_numbers_and_do_not_stop_the_rest() {
    let mut m = ShortcutMap::new();
    let errors = m.apply_config(
        "Ctrl+a = quit-app\n\
         Ctrl+b quit-app\n\
         Ctrl+c = fly\n\
         Wizard+d = quit-app\n\
         Ctrl+e = workspace:two\n\
         Ctrl+f = lock\n",
    );
    assert_eq!(
        errors,
        vec![
            (2, ParseError::MissingSeparator("Ctrl+b quit-app".into())),
            (3, ParseError::UnknownAction("fly".into())),
            (4, ParseError::UnknownModifier("Wizard".into())),
            (5, ParseError::BadNumber("two".into())),
        ]
    );
    assert_eq!(m.len(), 2);
}

#[test]
fn sections_apply_to_their_profile_only() {
    let text = "Ctrl+a = lock\n\
                [cupertino]\n\
                Ctrl+b = lock\n\
                [classic-pc]\n\
                Ctrl+c = lock\n\
                [elsewhere]\n\
                Ctrl+d = lock\n";
    for (profile, expected) in [
        ("cupertino", ["Ctrl+a", "Ctrl+b"]),
        ("classic-pc", ["Ctrl+a", "Ctrl+c"]),
    ] {
        let mut m = ShortcutMap::new();
        let report = m.apply_config_for(text, profile);
        assert_eq!(
            report.errors,
            vec![(6, ParseError::UnknownSection("elsewhere".into()))],
            "{profile}"
        );
        assert_eq!(bound(&m), expected, "{profile}");
    }
}

#[test]
fn classic_pc_lines_naming_super_outside_a_section_are_reported_as_retargeted() {
    let mut m = ShortcutMap::new();
    let report = m.apply_config_for("Super+q = lock\nCtrl+q = lock\n", "classic-pc");
    assert_eq!(report.retargeted, vec![(1, "Super+q = lock".to_string())]);
    let mut other = ShortcutMap::new();
    assert!(other
        .apply_config_for("Super+q = lock\n", "cupertino")
        .retargeted
        .is_empty());
    for (text, names) in [
        ("Super+q", true),
        ("<Ctrl><Logo>x", true),
        ("alt+WIN+x", true),
        ("Ctrl+Alt+q", false),
        ("Superb", false),
    ] {
        assert_eq!(names_super(text), names, "{text}");
    }
}

#[test]
fn a_strong_release_chord_is_always_kept() {
    let chord = ShortcutMap::release_chord();
    assert_eq!(chord, combo("Super+Escape"));
    for (name, own, added, kept) in [
        (
            "weak ones only",
            &["F9", "Shift+F10"][..],
            Some(chord),
            chord,
        ),
        (
            "a strong one of the user's own",
            &["Ctrl+Alt+Escape"][..],
            None,
            combo("Ctrl+Alt+Escape"),
        ),
    ] {
        let mut m = ShortcutMap::new();
        for text in own {
            m.bind(combo(text), Action::ReleaseShortcuts);
        }
        assert_eq!(m.keep_release_chord(), added, "{name}");
        assert_eq!(m.combo_for(&Action::ReleaseShortcuts), Some(kept), "{name}");
        assert_eq!(m.len(), 1, "{name}: weak bindings are dropped");
        assert_eq!(m.keep_release_chord(), None, "{name}: already there");
    }
}

#[test]
fn both_default_sets_are_complete_without_duplicates() {
    for (profile, tap) in [("cupertino", None), ("classic-pc", Some(&Action::Launcher))] {
        let m = ShortcutMap::defaults_for(profile);
        assert_eq!(m.tap(), tap, "{profile}");
        assert!(m.len() > 40, "{profile}");
        let mut actions = vec![Action::ReleaseShortcuts, Action::Overview];
        actions.extend((1..=9).map(Action::Workspace));
        actions.extend(ScreenshotRequest::ALL.map(Action::Screenshot));
        for action in actions {
            assert!(m.combo_for(&action).is_some(), "{profile} {action:?}");
        }
        let mut combos = bound(&m);
        let count = combos.len();
        combos.sort();
        combos.dedup();
        assert_eq!(combos.len(), count, "{profile}: no two share a combination");
    }
    assert_eq!(
        ShortcutMap::defaults_for("anything else").len(),
        ShortcutMap::defaults().len()
    );
    assert_eq!(
        ShortcutMap::classic_pc_defaults().tap(),
        Some(&Action::Launcher)
    );
}
