//! `menubar.conf`: clock options, shown items, background, recent counts and hidden status icons.

use cupertino_defaults::menubar::*;

#[test]
fn an_empty_file_reads_as_the_unset_settings() {
    let (s, problems) = MenuBarSettings::parse("", None);
    assert!(problems.is_empty());
    assert_eq!(s, MenuBarSettings::UNSET);
    assert_eq!(s.clock, ClockSettings::UNSET);
    assert!(!s.clock.show_seconds && !s.clock.show_day_of_week && s.clock.show_am_pm);
    assert_eq!(s.clock.show_date, ShowDate::Never);
    assert_eq!(s.clock.hour_cycle, HourCycle::H12);
    assert!(s.shown.clock && s.shown.wifi && s.shown.battery && s.shown.bluetooth && s.shown.focus);
    assert!(!s.background);
    assert!(s.item_tone);
    assert_eq!((s.recent_documents, s.recent_apps), (10, 10));
    assert!(s.hidden_tray.is_empty());
}

#[test]
fn clock_keys_are_read_and_bad_lines_are_reported_without_losing_the_rest() {
    let (s, problems) = MenuBarSettings::parse(
        "clock.show_seconds = on\nclock.show_date = always\nclock.hour_cycle = 24\n\
         clock.show_date = sometimes\nclock.hour_cycle = 13\nclock.show_am_pm = off\n\
         clock.show_day_of_week = yes\nclock.flash_separators = true\n",
        None,
    );
    assert_eq!(problems.iter().map(|p| p.line).collect::<Vec<_>>(), [4, 5]);
    assert!(problems[0]
        .message
        .contains("when-space-allows, always or never"));
    assert!(problems[1].message.contains("12 or 24"));
    assert!(s.clock.show_seconds && s.clock.show_day_of_week && s.clock.flash_separators);
    assert!(!s.clock.show_am_pm);
    assert_eq!(
        s.clock.show_date,
        ShowDate::Always,
        "the bad later line did not apply"
    );
    assert_eq!(s.clock.hour_cycle, HourCycle::H24);
}

#[test]
fn each_status_item_can_be_turned_off_and_the_bar_background_on() {
    let (s, problems) = MenuBarSettings::parse(
        "show.clock = off\nshow.wifi = off\nshow.battery = off\nshow.bluetooth = off\n\
         show.focus = off\nshow.background = on\ntone.per_item = off\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(!s.shown.clock && !s.shown.wifi && !s.shown.battery);
    assert!(!s.shown.bluetooth && !s.shown.focus);
    assert!(s.background);
    assert!(!s.item_tone);
}

#[test]
fn recent_counts_run_from_zero_to_the_maximum() {
    let (s, problems) = MenuBarSettings::parse(
        &format!("recent.documents = 0\nrecent.apps = {RECENT_MAX}\n"),
        None,
    );
    assert!(problems.is_empty());
    assert_eq!((s.recent_documents, s.recent_apps), (0, RECENT_MAX));
    let (s, problems) = MenuBarSettings::parse("recent.documents = 51\nrecent.apps = -1\n", None);
    assert_eq!(problems.len(), 2);
    assert!(problems[0].message.contains("0 to 50"));
    assert_eq!((s.recent_documents, s.recent_apps), (10, 10));
}

#[test]
fn hidden_tray_icons_are_a_comma_list() {
    let (s, _) = MenuBarSettings::parse("tray.hidden = nm-applet, , blueman\n", None);
    assert_eq!(s.hidden_tray, ["nm-applet", "blueman"]);
}

#[test]
fn a_wrong_value_keeps_the_last_good_one() {
    let (good, _) = MenuBarSettings::parse("recent.apps = 5\nclock.hour_cycle = 24\n", None);
    let (s, problems) =
        MenuBarSettings::parse("recent.apps = many\nclock.hour_cycle = 12\n", Some(&good));
    assert_eq!(problems.len(), 1);
    assert_eq!(s.recent_apps, 5);
    assert_eq!(s.clock.hour_cycle, HourCycle::H12);
}

#[test]
fn a_store_round_trips_and_keeps_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("menubar.conf");
    std::fs::write(&path, "# bar\nclock.show_seconds = off  # tick\n").unwrap();
    let (mut s, _) = MenuBarSettings::load(&path, None).unwrap();
    s.clock.show_seconds = true;
    s.hidden_tray = vec!["a".into(), "b".into()];
    s.recent_apps = 3;
    assert!(s.store(&path).unwrap().is_empty());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("# bar\nclock.show_seconds = on  # tick\n"),
        "{text}"
    );
    assert!(text.contains("tray.hidden = a,b\n") && text.contains("recent.apps = 3\n"));
    let (back, _) = MenuBarSettings::load(&path, None).unwrap();
    assert_eq!(back, s);
}

#[test]
fn the_environment_variable_redirects_the_file() {
    std::env::set_var(MENUBAR_CONF_ENV, "/x/menubar.conf");
    assert_eq!(MenuBarSettings::path(), Some("/x/menubar.conf".into()));
}
