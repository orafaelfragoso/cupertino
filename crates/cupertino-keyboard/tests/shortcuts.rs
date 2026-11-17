//! Desktop shortcut table, enabling, rebinding and override text.

use cupertino_keyboard::shortcuts::{ASSIGNMENTS, REFERENCE_ENABLED};
use cupertino_keyboard::{Chord, ShortcutSet, SystemAction};

fn chord(s: &str) -> Chord {
    Chord::parse(s).unwrap()
}

#[test]
fn every_action_has_one_assignment_with_a_unique_id_and_name() {
    let mut ids: Vec<u16> = ASSIGNMENTS.iter().map(|a| a.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), ASSIGNMENTS.len(), "ids are unique");
    let mut names: Vec<&str> = SystemAction::ALL.iter().map(|a| a.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), SystemAction::ALL.len(), "names are unique");
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
fn the_default_sets_keep_every_chord_and_enable_their_rows() {
    let all = ShortcutSet::all_assigned();
    let reference = ShortcutSet::reference_defaults();
    for a in ASSIGNMENTS {
        let name = a.action.name();
        let assigned = Some(Chord::new(a.mods, a.key));
        assert!(all.is_enabled(a.action), "{name}");
        assert_eq!(all.chord_for(a.action), assigned, "{name}");
        assert_eq!(reference.chord_for(a.action), assigned, "{name}");
        assert_eq!(
            reference.is_enabled(a.action),
            REFERENCE_ENABLED.contains(&a.action),
            "{name}"
        );
    }
    for (action, text) in [
        (SystemAction::AppSwitcherNext, "Primary+Tab"),
        (SystemAction::ScreenshotScreenToFile, "Primary+Shift+3"),
        (SystemAction::InputSourcePrevious, "Ctrl+Space"),
        (SystemAction::InputSourceNext, "Ctrl+Alt+Space"),
        (SystemAction::LauncherToggle, "Primary+Space"),
    ] {
        assert_eq!(
            reference.lookup(&chord(text)),
            Some(action),
            "{}",
            action.name()
        );
    }
}

#[test]
fn lookup_finds_only_enabled_bound_actions_lowest_id_first() {
    let mut s = ShortcutSet::reference_defaults();
    assert_eq!(s.lookup(&chord("Ctrl+Alt+Shift+Q")), None, "unbound chord");
    s.set_enabled(SystemAction::LauncherToggle, false);
    assert_eq!(s.lookup(&chord("Primary+Space")), None, "disabled action");
    s.set_enabled(SystemAction::LauncherToggle, true);
    assert_eq!(
        s.lookup(&chord("Primary+Space")),
        Some(SystemAction::LauncherToggle)
    );

    s.rebind(SystemAction::LauncherToggle, chord("Alt+Space"));
    assert_eq!(
        s.chord_for(SystemAction::LauncherToggle),
        Some(chord("Alt+Space"))
    );
    assert_eq!(s.lookup(&chord("Primary+Space")), None, "old chord is free");
    s.unbind(SystemAction::LauncherToggle);
    assert_eq!(s.chord_for(SystemAction::LauncherToggle), None);
    assert_eq!(s.lookup(&chord("Alt+Space")), None, "unbound action");

    let mut shared = ShortcutSet::all_assigned();
    shared.rebind(SystemAction::ZoomIn, chord("Primary+Space"));
    let first = ASSIGNMENTS
        .iter()
        .filter(|a| {
            matches!(
                a.action,
                SystemAction::ZoomIn | SystemAction::LauncherToggle
            )
        })
        .min_by_key(|a| a.id)
        .unwrap();
    assert_eq!(shared.lookup(&chord("Primary+Space")), Some(first.action));
}

#[test]
fn overrides_rebind_enable_and_disable_actions() {
    let mut s = ShortcutSet::reference_defaults();
    let text = "ZoomIn = Ctrl+Alt+Shift+Z\nlaunchertoggle = none\n# comment\nShowDesktop = OFF";
    assert_eq!(s.apply_overrides(text), Vec::new());
    assert_eq!(
        s.lookup(&chord("Ctrl+Alt+Shift+Z")),
        Some(SystemAction::ZoomIn)
    );
    assert!(!s.is_enabled(SystemAction::LauncherToggle));
    assert!(!s.is_enabled(SystemAction::ShowDesktop));
}

#[test]
fn override_problems_carry_line_and_message() {
    let mut s = ShortcutSet::reference_defaults();
    let errs = s.apply_overrides("nonsense\nNoSuchAction = Ctrl+A\nZoomIn = Hyper+A");
    let got: Vec<(usize, &str)> = errs.iter().map(|e| (e.line, e.message.as_str())).collect();
    assert_eq!(
        got,
        [
            (1, "expected 'Action = chord'"),
            (2, "unknown action 'NoSuchAction'"),
            (3, "unknown modifier 'hyper'"),
        ]
    );
}
