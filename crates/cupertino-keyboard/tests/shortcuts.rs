//! Desktop shortcut table, enabling, rebinding and override text.

use cupertino_keyboard::modifiers::Mods;
use cupertino_keyboard::shortcuts::{ASSIGNMENTS, REFERENCE_ENABLED};
use cupertino_keyboard::{Chord, Key, NamedKey, ShortcutSet, SystemAction};

fn chord(s: &str) -> Chord {
    Chord::parse(s).unwrap()
}

#[test]
fn the_built_in_table_has_eighty_assignments_with_unique_ids() {
    assert_eq!(ASSIGNMENTS.len(), 80);
    let mut ids: Vec<u16> = ASSIGNMENTS.iter().map(|a| a.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 80);
}

#[test]
fn every_action_is_assigned_once() {
    assert_eq!(SystemAction::ALL.len(), ASSIGNMENTS.len());
    for action in SystemAction::ALL {
        assert_eq!(
            ASSIGNMENTS.iter().filter(|a| a.action == *action).count(),
            1,
            "{}",
            action.name()
        );
    }
}

#[test]
fn all_assigned_enables_every_row() {
    let s = ShortcutSet::all_assigned();
    for a in ASSIGNMENTS {
        assert!(s.is_enabled(a.action));
        assert_eq!(s.chord_for(a.action), Some(Chord::new(a.mods, a.key)));
    }
}

#[test]
fn reference_defaults_enable_only_the_listed_actions() {
    let s = ShortcutSet::reference_defaults();
    for a in ASSIGNMENTS {
        assert_eq!(
            s.is_enabled(a.action),
            REFERENCE_ENABLED.contains(&a.action),
            "{}",
            a.action.name()
        );
    }
}

#[test]
fn reference_defaults_keep_every_assignment_even_when_disabled() {
    let s = ShortcutSet::reference_defaults();
    assert!(s.chord_for(SystemAction::ZoomIn).is_some());
    assert!(!s.is_enabled(SystemAction::ZoomIn));
}

#[test]
fn well_known_defaults_are_assigned() {
    let s = ShortcutSet::reference_defaults();
    assert_eq!(
        s.chord_for(SystemAction::AppSwitcherNext),
        Some(Chord::new(Mods::PRIMARY, Key::Named(NamedKey::Tab)))
    );
    assert_eq!(
        s.chord_for(SystemAction::ScreenshotScreenToFile),
        Some(chord("Primary+Shift+3"))
    );
    assert_eq!(
        s.chord_for(SystemAction::InputSourcePrevious),
        Some(chord("Ctrl+Space"))
    );
    assert_eq!(
        s.chord_for(SystemAction::InputSourceNext),
        Some(chord("Ctrl+Alt+Space"))
    );
    assert_eq!(
        s.chord_for(SystemAction::LauncherToggle),
        Some(chord("Primary+Space"))
    );
}

#[test]
fn lookup_finds_an_enabled_action_by_chord() {
    let s = ShortcutSet::reference_defaults();
    assert_eq!(
        s.lookup(&chord("Primary+Shift+3")),
        Some(SystemAction::ScreenshotScreenToFile)
    );
    assert_eq!(s.lookup(&chord("Ctrl+Alt+Shift+Q")), None);
}

#[test]
fn lookup_skips_disabled_actions() {
    let mut s = ShortcutSet::reference_defaults();
    s.set_enabled(SystemAction::LauncherToggle, false);
    assert_eq!(s.lookup(&chord("Primary+Space")), None);
    s.set_enabled(SystemAction::LauncherToggle, true);
    assert_eq!(
        s.lookup(&chord("Primary+Space")),
        Some(SystemAction::LauncherToggle)
    );
}

#[test]
fn lookup_prefers_the_lowest_table_id_on_a_shared_chord() {
    let mut s = ShortcutSet::all_assigned();
    s.rebind(SystemAction::ZoomIn, chord("Primary+Space"));
    let first = ASSIGNMENTS
        .iter()
        .filter(|a| a.action == SystemAction::ZoomIn || a.action == SystemAction::LauncherToggle)
        .min_by_key(|a| a.id)
        .unwrap();
    assert_eq!(s.lookup(&chord("Primary+Space")), Some(first.action));
}

#[test]
fn rebind_replaces_the_chord_and_unbind_removes_it() {
    let mut s = ShortcutSet::reference_defaults();
    s.rebind(SystemAction::LauncherToggle, chord("Alt+Space"));
    assert_eq!(
        s.chord_for(SystemAction::LauncherToggle),
        Some(chord("Alt+Space"))
    );
    assert_eq!(s.lookup(&chord("Primary+Space")), None);
    s.unbind(SystemAction::LauncherToggle);
    assert_eq!(s.chord_for(SystemAction::LauncherToggle), None);
    assert_eq!(s.lookup(&chord("Alt+Space")), None);
}

#[test]
fn override_rebinds_and_enables_an_action() {
    let mut s = ShortcutSet::reference_defaults();
    let errs = s.apply_overrides("ZoomIn = Ctrl+Alt+Shift+Z");
    assert!(errs.is_empty());
    assert_eq!(
        s.lookup(&chord("Ctrl+Alt+Shift+Z")),
        Some(SystemAction::ZoomIn)
    );
}

#[test]
fn override_none_and_off_disable_an_action() {
    let mut s = ShortcutSet::reference_defaults();
    let errs = s.apply_overrides("LauncherToggle = none\n# comment\nShowDesktop = OFF");
    assert!(errs.is_empty());
    assert!(!s.is_enabled(SystemAction::LauncherToggle));
    assert!(!s.is_enabled(SystemAction::ShowDesktop));
}

#[test]
fn override_action_names_are_case_insensitive() {
    let mut s = ShortcutSet::reference_defaults();
    assert!(s.apply_overrides("launchertoggle = off").is_empty());
    assert!(!s.is_enabled(SystemAction::LauncherToggle));
}

#[test]
fn override_problems_carry_line_and_message() {
    let mut s = ShortcutSet::reference_defaults();
    let errs = s.apply_overrides("nonsense\nNoSuchAction = Ctrl+A\nZoomIn = Hyper+A");
    assert_eq!(errs.len(), 3);
    assert_eq!(
        (errs[0].line, errs[0].message.as_str()),
        (1, "expected 'Action = chord'")
    );
    assert_eq!(
        (errs[1].line, errs[1].message.as_str()),
        (2, "unknown action 'NoSuchAction'")
    );
    assert_eq!(
        (errs[2].line, errs[2].message.as_str()),
        (3, "unknown modifier 'hyper'")
    );
}

#[test]
fn system_action_names_are_unique() {
    let mut names: Vec<&str> = SystemAction::ALL.iter().map(|a| a.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), SystemAction::ALL.len());
}
