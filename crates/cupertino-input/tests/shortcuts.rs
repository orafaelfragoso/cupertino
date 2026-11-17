//! Shortcut maps bind key combinations to actions, are written in a small text format and ship with two default sets.

use cupertino_input::keysym::{self, ESCAPE, ISO_LEFT_TAB, TAB};
use cupertino_input::{
    names_super, Action, KeyCombo, Modifiers, ParseError, ScreenshotRequest, ShortcutMap,
};

fn mods(shift: bool, ctrl: bool, alt: bool, sup: bool) -> Modifiers {
    Modifiers::new(shift, ctrl, alt, sup)
}

fn combo(text: &str) -> KeyCombo {
    text.parse().unwrap_or_else(|e| panic!("{text}: {e}"))
}

#[test]
fn every_action_name_parses_back_to_the_action() {
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
}

#[test]
fn action_parsing_reports_what_was_wrong() {
    assert_eq!("".parse::<Action>(), Err(ParseError::Empty));
    assert_eq!("   ".parse::<Action>(), Err(ParseError::Empty));
    assert_eq!(
        "frobnicate".parse::<Action>(),
        Err(ParseError::UnknownAction("frobnicate".into()))
    );
    assert_eq!(
        "workspace:x".parse::<Action>(),
        Err(ParseError::BadNumber("x".into()))
    );
    assert_eq!(
        "workspace:-1".parse::<Action>(),
        Err(ParseError::BadNumber("-1".into()))
    );
    assert_eq!("workspace: 4".parse::<Action>(), Ok(Action::Workspace(4)));
    assert_eq!(
        "exec:  ls -l ".parse::<Action>(),
        Ok(Action::Exec("ls -l".into()))
    );
}

#[test]
fn combinations_parse_in_both_spellings() {
    let expected = KeyCombo::new(mods(false, true, true, false), u32::from(b'q'));
    assert_eq!(combo("Ctrl+Alt+q"), expected);
    assert_eq!(combo("<Ctrl><Alt>q"), expected);
    assert_eq!(
        combo("control+option+Q"),
        expected,
        "names and case are free"
    );
    assert_eq!(
        combo("Super+Tab").modifiers,
        mods(false, false, false, true)
    );
    for name in ["super", "logo", "win", "windows", "mod4"] {
        assert!(combo(&format!("{name}+a")).modifiers.super_key, "{name}");
    }
}

#[test]
fn a_literal_plus_key_is_written_with_a_trailing_plus() {
    let c = combo("Super++");
    assert_eq!(c.keysym, 0x2b);
    assert!(c.modifiers.super_key);
    assert_eq!(combo("+").keysym, 0x2b);
}

#[test]
fn combination_errors_name_the_offending_part() {
    assert_eq!("".parse::<KeyCombo>(), Err(ParseError::Empty));
    assert_eq!(
        "Hyper+a".parse::<KeyCombo>(),
        Err(ParseError::UnknownModifier("Hyper".into()))
    );
    assert_eq!(
        "<Hyper>a".parse::<KeyCombo>(),
        Err(ParseError::UnknownModifier("Hyper".into()))
    );
    assert_eq!(
        "<Ctrl".parse::<KeyCombo>(),
        Err(ParseError::UnknownModifier("<Ctrl".into()))
    );
    assert_eq!(
        "Ctrl+Nonsense".parse::<KeyCombo>(),
        Err(ParseError::UnknownKey("Nonsense".into()))
    );
    assert_eq!("<Ctrl>".parse::<KeyCombo>(), Err(ParseError::MissingKey));
}

#[test]
fn shift_tab_has_one_spelling() {
    let a = KeyCombo::new(Modifiers::NONE, ISO_LEFT_TAB);
    assert_eq!(a.keysym, TAB);
    assert!(a.modifiers.shift);
    assert_eq!(a, combo("Shift+Tab"));
    assert_eq!(combo("Super+Q"), combo("Super+q"), "letters are folded");
}

#[test]
fn combinations_print_in_a_fixed_order_and_parse_back() {
    for text in [
        "Ctrl+Alt+Shift+Super+Left",
        "Super+space",
        "F11",
        "Ctrl+XF86PowerOff",
        "Alt+1",
    ] {
        assert_eq!(combo(text).to_string(), text);
    }
    assert_eq!(combo("<Super><Ctrl>x").to_string(), "Ctrl+Super+x");
    let unnamed = KeyCombo::new(Modifiers::NONE, 0x1234_5678);
    assert_eq!(unnamed.to_string(), "0x12345678");
    assert_eq!(unnamed.to_string().parse::<KeyCombo>(), Ok(unnamed));
}

#[test]
fn the_primary_modifier_follows_the_profile() {
    assert!(
        KeyCombo::parse_with("Primary+c", cupertino_keyboard::modifiers::Physical::Ctrl)
            .unwrap()
            .modifiers
            .ctrl
    );
    assert!(combo("Primary+c").modifiers.super_key);
}

#[test]
fn binding_replaces_and_returns_the_old_action() {
    let mut m = ShortcutMap::new();
    assert!(m.is_empty());
    assert_eq!(m.bind(combo("Super+q"), Action::QuitApp), None);
    assert_eq!(
        m.bind(combo("Super+Q"), Action::Lock),
        Some(Action::QuitApp)
    );
    assert_eq!(m.len(), 1);
    assert_eq!(
        m.lookup(mods(false, false, false, true), u32::from(b'q')),
        Some(&Action::Lock)
    );
    assert_eq!(
        m.lookup(mods(false, false, false, true), u32::from(b'Q')),
        Some(&Action::Lock)
    );
    assert_eq!(m.lookup(Modifiers::NONE, u32::from(b'q')), None);
}

#[test]
fn lookups_need_the_exact_modifiers() {
    let mut m = ShortcutMap::new();
    m.bind(combo("Ctrl+a"), Action::AppGrid);
    assert!(m
        .lookup(mods(false, true, false, false), u32::from(b'a'))
        .is_some());
    assert!(m
        .lookup(mods(true, true, false, false), u32::from(b'a'))
        .is_none());
    assert!(m
        .lookup(mods(false, true, false, true), u32::from(b'a'))
        .is_none());
}

#[test]
fn a_shift_tab_event_finds_the_shift_tab_binding() {
    let mut m = ShortcutMap::new();
    m.bind(combo("Shift+Tab"), Action::SwitchAppReverse);
    assert_eq!(
        m.lookup(Modifiers::NONE, ISO_LEFT_TAB),
        Some(&Action::SwitchAppReverse)
    );
    assert_eq!(
        m.lookup(mods(true, false, false, false), TAB),
        Some(&Action::SwitchAppReverse)
    );
}

#[test]
fn bindings_iterate_in_binding_order_and_unbinding_removes_them() {
    let mut m = ShortcutMap::new();
    m.bind(combo("F5"), Action::Overview);
    m.bind(combo("F1"), Action::Lock);
    m.bind(combo("F3"), Action::AppGrid);
    m.bind(combo("F1"), Action::Launcher);
    let order: Vec<String> = m.bindings().map(|(c, _)| c.to_string()).collect();
    assert_eq!(order, ["F5", "F1", "F3"]);
    assert_eq!(m.unbind(combo("F1")), Some(Action::Launcher));
    assert_eq!(m.unbind(combo("F1")), None);
    assert_eq!(m.len(), 2);
    assert_eq!(m.combo_for(&Action::AppGrid), Some(combo("F3")));
    assert_eq!(m.combo_for(&Action::Launcher), None);
}

#[test]
fn bind_str_reports_a_bad_combination() {
    let mut m = ShortcutMap::new();
    assert_eq!(m.bind_str("Ctrl+z", Action::HideApp), Ok(None));
    assert_eq!(
        m.bind_str("Ctrl+nokey", Action::HideApp),
        Err(ParseError::UnknownKey("nokey".into()))
    );
    assert_eq!(m.len(), 1);
}

#[test]
fn the_tap_binding_is_separate_from_the_combinations() {
    let mut m = ShortcutMap::new();
    assert_eq!(m.tap(), None);
    assert_eq!(m.bind_tap(Action::Launcher), None);
    assert_eq!(m.bind_tap(Action::Overview), Some(Action::Launcher));
    assert_eq!(m.tap(), Some(&Action::Overview));
    assert!(m.is_empty());
    assert_eq!(m.unbind_tap(), Some(Action::Overview));
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
    assert_eq!(
        m.lookup(mods(false, false, false, true), u32::from(b'q')),
        Some(&Action::Lock)
    );
    assert_eq!(
        m.lookup(mods(false, false, false, true), u32::from(b'w')),
        None
    );
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
fn only_the_super_key_can_be_tapped() {
    let mut m = ShortcutMap::new();
    let errors = m.apply_config("Super = launcher\nCtrl = overview\n");
    assert_eq!(errors, vec![(2, ParseError::TapNotSuper("Ctrl".into()))]);
    assert_eq!(m.tap(), Some(&Action::Launcher));
    assert!(m.apply_config("Super = none\n").is_empty());
    assert_eq!(m.tap(), None);
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
    let mut cupertino = ShortcutMap::new();
    let report = cupertino.apply_config_for(text, "cupertino");
    assert_eq!(
        report.errors,
        vec![(6, ParseError::UnknownSection("elsewhere".into()))]
    );
    let bound: Vec<String> = cupertino.bindings().map(|(c, _)| c.to_string()).collect();
    assert_eq!(bound, ["Ctrl+a", "Ctrl+b"]);

    let mut classic = ShortcutMap::new();
    classic.apply_config_for(text, "classic-pc");
    let bound: Vec<String> = classic.bindings().map(|(c, _)| c.to_string()).collect();
    assert_eq!(bound, ["Ctrl+a", "Ctrl+c"]);
}

#[test]
fn the_primary_modifier_in_a_config_is_the_profiles_command_key() {
    let mut cupertino = ShortcutMap::new();
    cupertino.apply_config_for("Primary+c = lock\n", "cupertino");
    assert_eq!(
        cupertino.lookup(mods(false, false, false, true), u32::from(b'c')),
        Some(&Action::Lock)
    );
    let mut classic = ShortcutMap::new();
    classic.apply_config_for("Primary+c = lock\n", "classic-pc");
    assert_eq!(
        classic.lookup(mods(false, true, false, false), u32::from(b'c')),
        Some(&Action::Lock)
    );
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
}

#[test]
fn names_super_looks_at_every_part_of_the_left_hand_side() {
    assert!(names_super("Super+q"));
    assert!(names_super("<Ctrl><Logo>x"));
    assert!(names_super("alt+WIN+x"));
    assert!(!names_super("Ctrl+Alt+q"));
    assert!(!names_super("Superb"));
}

#[test]
fn the_release_chord_is_kept_strong_and_present() {
    let mut m = ShortcutMap::new();
    m.bind(combo("F9"), Action::ReleaseShortcuts);
    m.bind(combo("Shift+F10"), Action::ReleaseShortcuts);
    let added = m.keep_release_chord();
    assert_eq!(added, Some(ShortcutMap::release_chord()));
    assert_eq!(ShortcutMap::release_chord(), combo("Super+Escape"));
    assert_eq!(
        m.combo_for(&Action::ReleaseShortcuts),
        Some(ShortcutMap::release_chord())
    );
    assert_eq!(m.len(), 1, "the weak bindings were dropped");
    assert_eq!(m.keep_release_chord(), None, "already there");
}

#[test]
fn a_strong_release_chord_of_the_users_own_is_kept() {
    let mut m = ShortcutMap::new();
    m.bind(combo("Ctrl+Alt+Escape"), Action::ReleaseShortcuts);
    assert_eq!(m.keep_release_chord(), None);
    assert_eq!(
        m.combo_for(&Action::ReleaseShortcuts),
        Some(combo("Ctrl+Alt+Escape"))
    );
    assert_eq!(keysym::ESCAPE, ESCAPE);
}

#[test]
fn both_default_sets_parse_cleanly_and_can_always_release_shortcuts() {
    for profile in ["cupertino", "classic-pc"] {
        let m = ShortcutMap::defaults_for(profile);
        assert!(m.len() > 40, "{profile}");
        assert!(
            m.combo_for(&Action::ReleaseShortcuts).is_some(),
            "{profile}"
        );
        assert!(m.combo_for(&Action::Overview).is_some(), "{profile}");
        for n in 1..=9 {
            assert!(
                m.combo_for(&Action::Workspace(n)).is_some(),
                "{profile} {n}"
            );
        }
        for request in ScreenshotRequest::ALL {
            assert!(
                m.combo_for(&Action::Screenshot(request)).is_some(),
                "{profile} {request:?}"
            );
        }
    }
    assert_eq!(
        ShortcutMap::defaults_for("anything else").len(),
        ShortcutMap::defaults().len()
    );
}

#[test]
fn the_classic_defaults_tap_super_for_the_launcher_and_the_cupertino_ones_do_not() {
    assert_eq!(
        ShortcutMap::classic_pc_defaults().tap(),
        Some(&Action::Launcher)
    );
    assert_eq!(ShortcutMap::defaults().tap(), None);
}

#[test]
fn no_two_defaults_share_a_combination() {
    for profile in ["cupertino", "classic-pc"] {
        let m = ShortcutMap::defaults_for(profile);
        let all: Vec<KeyCombo> = m.bindings().map(|(c, _)| *c).collect();
        let mut sorted: Vec<String> = all.iter().map(ToString::to_string).collect();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "{profile}");
    }
}

#[test]
fn screenshot_requests_have_distinct_names() {
    let mut names: Vec<&str> = ScreenshotRequest::ALL.iter().map(|r| r.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), ScreenshotRequest::ALL.len());
}
