//! `launcher.conf`, `control-panel.conf` and `power.conf`.

use std::path::Path;
use std::time::Duration;

use cupertino_defaults::control_panel::ControlPanelSettings;
use cupertino_defaults::launcher::LauncherSettings;
use cupertino_defaults::power::{PowerSettings, Timer};

fn secs(n: u64) -> Timer {
    Timer::After(Duration::from_secs(n))
}

#[test]
fn the_launcher_kinds_of_result_and_the_top_hit_are_switches_on_by_default() {
    assert_eq!(LauncherSettings::parse("", None).0, LauncherSettings::UNSET);
    let s = LauncherSettings::UNSET;
    assert!(s.applications && s.folders && s.files && s.calculator && s.actions && s.top_hit);
    let (s, problems) = LauncherSettings::parse(
        "results.applications = off\nresults.folders = no\nresults.files = 0\n\
         results.calculator = false\nresults.actions = off\ntop_hit = off\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(!s.applications && !s.folders && !s.files && !s.calculator && !s.actions && !s.top_hit);
}

#[test]
fn excluded_folders_are_a_comma_list_turned_into_anchored_patterns() {
    let (s, _) = LauncherSettings::parse("files.exclude = /home/u/Private, node_modules ,\n", None);
    assert_eq!(s.excluded, ["/home/u/Private", "node_modules"]);
    for (home, expected) in [
        ("/home/u", ["/Private/", "/node_modules/"]),
        ("/srv", ["/home/u/Private/", "/node_modules/"]),
    ] {
        assert_eq!(
            s.exclusion_patterns(Path::new(home)),
            expected,
            "home {home}"
        );
    }
}

#[test]
fn a_launcher_store_writes_the_changed_keys_and_removes_an_emptied_exclusion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("launcher.conf");
    std::fs::write(&path, "files.exclude = a,b\n").unwrap();
    let (mut s, _) = LauncherSettings::load(&path, None).unwrap();
    s.calculator = false;
    s.store(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "files.exclude = a,b\nresults.calculator = off\n"
    );
    s.excluded.clear();
    s.store(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "results.calculator = off\n"
    );
    assert_eq!(LauncherSettings::load(&path, None).unwrap().0, s);
}

#[test]
fn the_controls_are_unsaid_absent_and_otherwise_in_their_written_order() {
    for (text, expected) in [
        ("", None),
        ("# nothing\n", None),
        ("controls =\n", Some(vec![])),
        (
            "controls = wifi, nonsense,battery\n",
            Some(vec!["wifi", "nonsense", "battery"]),
        ),
    ] {
        let (s, problems) = ControlPanelSettings::parse(text, None);
        assert!(problems.is_empty(), "{text:?}");
        assert_eq!(
            s.controls,
            expected.map(|v| v.into_iter().map(String::from).collect()),
            "{text:?}"
        );
    }
}

#[test]
fn a_stored_control_order_comes_back_and_unset_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control-panel.conf");
    let s = ControlPanelSettings {
        controls: Some(vec!["b".into(), "a".into()]),
    };
    s.store(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "controls = b,a\n");
    assert_eq!(ControlPanelSettings::load(&path, None).unwrap().0, s);
    ControlPanelSettings::UNSET.store(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}

#[test]
fn every_power_key_is_read_and_the_timers_follow_the_power_source() {
    let (s, problems) = PowerSettings::parse(
        "display_sleep_ac = 60\ndisplay_sleep_battery = never\nsystem_sleep_ac = 3600\n\
         system_sleep_battery = 900\ndim_before_display_sleep = off\nrequire_password_after = 0\n\
         lock_screen_display_sleep = 10\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        (s.ac.display_sleep, s.ac.system_sleep),
        (secs(60), secs(3600))
    );
    assert_eq!(
        (s.battery.display_sleep, s.battery.system_sleep),
        (Timer::Never, secs(900))
    );
    assert!(!s.dim_before_display_sleep);
    assert_eq!(
        s.require_password_after,
        secs(0),
        "0 locks as the display sleeps"
    );
    assert_eq!(s.lock_screen_display_sleep, Duration::from_secs(10));
    assert_eq!(s.timers(false), s.ac, "on mains");
    assert_eq!(s.timers(true), s.battery, "on battery");
    assert_eq!(secs(90).duration(), Some(Duration::from_secs(90)));
    assert_eq!(Timer::Never.duration(), None);
}

#[test]
fn power_timers_reject_zero_and_text() {
    for (line, expected) in [
        ("display_sleep_ac = 0", "at least 1 second or never"),
        ("system_sleep_ac = soon", "expected seconds"),
        (
            "lock_screen_display_sleep = 0",
            "lock_screen_display_sleep:",
        ),
        ("require_password_after = x", "require_password_after:"),
    ] {
        let (s, problems) = PowerSettings::parse(&format!("{line}\n"), None);
        assert_eq!(problems.len(), 1, "{line}");
        assert!(
            problems[0].message.contains(expected),
            "{line}: {}",
            problems[0].message
        );
        assert_eq!(s, PowerSettings::default(), "{line}");
    }
}

#[test]
fn a_power_store_writes_only_the_changed_keys_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("power.conf");
    let (mut s, _) = PowerSettings::parse("", None);
    s.dim_before_display_sleep = false;
    s.ac.system_sleep = secs(7200);
    s.store(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("dim_before_display_sleep = off\n"), "{text}");
    assert!(text.contains("system_sleep_ac = 7200\n"), "{text}");
    assert!(!text.contains("display_sleep_ac"), "{text}");
    assert_eq!(PowerSettings::load(&path, None).unwrap().0, s);
}
