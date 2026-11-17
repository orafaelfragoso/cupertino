//! `launcher.conf`, `control-panel.conf` and `power.conf`.

use std::path::Path;
use std::time::Duration;

use cupertino_defaults::control_panel::{ControlPanelSettings, CONTROL_PANEL_CONF_ENV};
use cupertino_defaults::launcher::{LauncherSettings, LAUNCHER_CONF_ENV};
use cupertino_defaults::power::{PowerSettings, Timer, POWER_CONF_ENV};

fn secs(n: u64) -> Timer {
    Timer::After(Duration::from_secs(n))
}

// launcher.conf

#[test]
fn the_launcher_lists_everything_when_the_file_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let (s, problems) = LauncherSettings::load(&dir.path().join("launcher.conf"), None).unwrap();
    assert!(problems.is_empty());
    assert_eq!(s, LauncherSettings::UNSET);
    assert_eq!(s, LauncherSettings::default());
    assert!(s.applications && s.folders && s.files && s.calculator && s.actions && s.top_hit);
    assert!(s.excluded.is_empty());
}

#[test]
fn the_launcher_kinds_of_result_and_the_top_hit_are_switches() {
    let (s, problems) = LauncherSettings::parse(
        "results.applications = off\nresults.folders = no\nresults.files = 0\n\
         results.calculator = false\nresults.actions = off\ntop_hit = off\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(!s.applications && !s.folders && !s.files && !s.calculator && !s.actions && !s.top_hit);
}

#[test]
fn a_bad_launcher_switch_is_a_problem_and_keeps_the_last_good_value() {
    let (good, _) = LauncherSettings::parse("results.files = off\n", None);
    let (s, problems) =
        LauncherSettings::parse("results.files = sometimes\ntop_hit = off\n", Some(&good));
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].line, 1);
    assert!(!s.files);
    assert!(!s.top_hit);
}

#[test]
fn excluded_folders_are_a_comma_list_turned_into_anchored_patterns() {
    let (s, _) = LauncherSettings::parse("files.exclude = /home/u/Private, node_modules ,\n", None);
    assert_eq!(s.excluded, ["/home/u/Private", "node_modules"]);
    assert_eq!(
        s.exclusion_patterns(Path::new("/home/u")),
        ["/Private/", "/node_modules/"]
    );
    assert_eq!(
        s.exclusion_patterns(Path::new("/srv")),
        ["/home/u/Private/", "/node_modules/"]
    );
}

#[test]
fn a_launcher_store_writes_the_changed_keys_and_removes_an_emptied_exclusion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("launcher.conf");
    std::fs::write(&path, "# l\nfiles.exclude = a,b\n").unwrap();
    let (mut s, _) = LauncherSettings::load(&path, None).unwrap();
    s.calculator = false;
    s.store(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# l\nfiles.exclude = a,b\nresults.calculator = off\n"
    );
    s.excluded.clear();
    s.store(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# l\nresults.calculator = off\n"
    );
}

#[test]
fn the_launcher_variable_redirects_the_file() {
    std::env::set_var(LAUNCHER_CONF_ENV, "/x/launcher.conf");
    assert_eq!(LauncherSettings::path(), Some("/x/launcher.conf".into()));
}

// control-panel.conf

#[test]
fn a_missing_or_empty_control_panel_file_does_not_say_which_controls_show() {
    let dir = tempfile::tempdir().unwrap();
    let (s, problems) = ControlPanelSettings::load(&dir.path().join("c.conf"), None).unwrap();
    assert!(problems.is_empty());
    assert_eq!(s, ControlPanelSettings::UNSET);
    assert_eq!(s.controls, None);
    assert_eq!(
        ControlPanelSettings::parse("# nothing\n", None).0.controls,
        None
    );
}

#[test]
fn the_controls_are_read_in_their_order_as_written_including_unknown_names() {
    let (s, problems) = ControlPanelSettings::parse("controls = wifi, nonsense,battery\n", None);
    assert!(problems.is_empty());
    assert_eq!(s.controls.unwrap(), ["wifi", "nonsense", "battery"]);
}

#[test]
fn an_empty_controls_value_shows_no_control() {
    let (s, _) = ControlPanelSettings::parse("controls =\n", None);
    assert_eq!(s.controls, Some(Vec::new()));
}

#[test]
fn an_unknown_control_panel_key_is_a_problem_and_the_controls_still_apply() {
    let (s, problems) = ControlPanelSettings::parse("layout = wide\ncontrols = a\n", None);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].line, 1);
    assert_eq!(s.controls.unwrap(), ["a"]);
}

#[test]
fn a_stored_control_order_comes_back_and_the_other_lines_stay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control-panel.conf");
    std::fs::write(&path, "# panel\n").unwrap();
    let s = ControlPanelSettings {
        controls: Some(vec!["b".into(), "a".into()]),
    };
    s.store(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# panel\ncontrols = b,a\n"
    );
    assert_eq!(ControlPanelSettings::load(&path, None).unwrap().0, s);
    ControlPanelSettings::UNSET.store(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# panel\n");
}

#[test]
fn the_control_panel_variable_redirects_the_file() {
    std::env::set_var(CONTROL_PANEL_CONF_ENV, "/x/cp.conf");
    assert_eq!(ControlPanelSettings::path(), Some("/x/cp.conf".into()));
}

// power.conf

#[test]
fn the_power_defaults_are_the_design_table() {
    let (s, problems) = PowerSettings::parse("", None);
    assert!(problems.is_empty());
    assert_eq!(s, PowerSettings::default());
    assert_eq!(s.ac.display_sleep, secs(600));
    assert_eq!(s.ac.system_sleep, Timer::Never);
    assert_eq!(s.battery.display_sleep, secs(120));
    assert_eq!(s.battery.system_sleep, secs(600));
    assert!(s.dim_before_display_sleep);
    assert_eq!(s.require_password_after, secs(300));
    assert_eq!(s.lock_screen_display_sleep, Duration::from_secs(30));
}

#[test]
fn power_lines_override_the_defaults() {
    let (s, problems) = PowerSettings::parse(
        "display_sleep_ac = 60\ndisplay_sleep_battery = never\nsystem_sleep_ac = 3600\n\
         system_sleep_battery = 900\ndim_before_display_sleep = off\nrequire_password_after = 0\n\
         lock_screen_display_sleep = 10\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(s.ac.display_sleep, secs(60));
    assert_eq!(s.battery.display_sleep, Timer::Never);
    assert_eq!(s.ac.system_sleep, secs(3600));
    assert_eq!(s.battery.system_sleep, secs(900));
    assert!(!s.dim_before_display_sleep);
    assert_eq!(
        s.require_password_after,
        secs(0),
        "0 locks as the display sleeps"
    );
    assert_eq!(s.lock_screen_display_sleep, Duration::from_secs(10));
}

#[test]
fn power_timers_reject_zero_and_text_with_a_message_and_a_line_number() {
    let (s, problems) = PowerSettings::parse(
        "display_sleep_ac = 0\nsystem_sleep_ac = soon\nlock_screen_display_sleep = 0\nrequire_password_after = x\n",
        None,
    );
    assert_eq!(
        problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert!(problems[0].message.contains("at least 1 second or never"));
    assert!(problems[1].message.contains("expected seconds"));
    assert_eq!(s, PowerSettings::default());
}

#[test]
fn a_bad_power_timer_keeps_the_last_good_value() {
    let (good, _) = PowerSettings::parse("display_sleep_battery = 45\n", None);
    let (s, problems) = PowerSettings::parse("display_sleep_battery = 0\n", Some(&good));
    assert_eq!(problems.len(), 1);
    assert_eq!(s.battery.display_sleep, secs(45));
}

#[test]
fn the_timers_of_a_source_are_those_of_the_mains_or_the_battery() {
    let s = PowerSettings::default();
    assert_eq!(s.timers(false), s.ac);
    assert_eq!(s.timers(true), s.battery);
}

#[test]
fn timers_have_a_duration_and_a_text() {
    assert_eq!(secs(90).duration(), Some(Duration::from_secs(90)));
    assert_eq!(Timer::Never.duration(), None);
    assert_eq!(secs(90).to_string(), "90 s");
    assert_eq!(Timer::Never.to_string(), "never");
}

#[test]
fn a_power_store_keeps_every_key_it_was_not_asked_to_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("power.conf");
    std::fs::write(&path, "# p\nsystem_sleep_battery = 30  # hm\n").unwrap();
    let (mut s, _) = PowerSettings::load(&path, None).unwrap();
    s.dim_before_display_sleep = false;
    s.ac.system_sleep = secs(7200);
    s.store(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("# p\nsystem_sleep_battery = 30  # hm\n"),
        "{text}"
    );
    assert!(text.contains("dim_before_display_sleep = off\n"));
    assert!(text.contains("system_sleep_ac = 7200\n"));
    assert!(
        !text.contains("display_sleep_ac"),
        "unchanged keys are not written"
    );
    assert_eq!(PowerSettings::load(&path, None).unwrap().0, s);
}

#[test]
fn the_power_variable_redirects_the_file() {
    std::env::set_var(POWER_CONF_ENV, "/x/power.conf");
    assert_eq!(PowerSettings::path(), Some("/x/power.conf".into()));
}
