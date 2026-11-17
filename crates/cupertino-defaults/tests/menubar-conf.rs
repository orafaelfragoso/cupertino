//! `menubar.conf`: clock options, shown items, background, recent counts and hidden status icons.

use cupertino_defaults::menubar::*;

#[test]
fn every_key_is_read() {
    let (s, problems) = MenuBarSettings::parse(
        &format!(
            "clock.show_seconds = on\nclock.show_date = always\nclock.hour_cycle = 24\n\
             clock.show_am_pm = off\nclock.show_day_of_week = yes\nclock.flash_separators = true\n\
             show.clock = off\nshow.wifi = off\nshow.battery = off\nshow.bluetooth = off\n\
             show.focus = off\nshow.background = on\ntone.per_item = off\n\
             recent.documents = 0\nrecent.apps = {RECENT_MAX}\ntray.hidden = nm-applet, , blueman\n"
        ),
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    let c = &s.clock;
    assert!(c.show_seconds && c.show_day_of_week && c.flash_separators && !c.show_am_pm);
    assert_eq!(c.show_date, ShowDate::Always);
    assert_eq!(c.hour_cycle, HourCycle::H24);
    let shown = &s.shown;
    assert!(!shown.clock && !shown.wifi && !shown.battery && !shown.bluetooth && !shown.focus);
    assert!(s.background && !s.item_tone);
    assert_eq!((s.recent_documents, s.recent_apps), (0, RECENT_MAX));
    assert_eq!(s.hidden_tray, ["nm-applet", "blueman"]);
}

#[test]
fn rejected_values_are_reported_with_the_expected_choices() {
    for (line, expected) in [
        (
            "clock.show_date = sometimes",
            "when-space-allows, always or never",
        ),
        ("clock.hour_cycle = 13", "12 or 24"),
        ("recent.documents = 51", "0 to 50"),
        ("recent.apps = -1", "0 to 50"),
    ] {
        let (s, problems) = MenuBarSettings::parse(&format!("{line}\n"), None);
        assert_eq!(problems.len(), 1, "{line}");
        assert!(
            problems[0].message.contains(expected),
            "{line}: {}",
            problems[0].message
        );
        assert_eq!(s, MenuBarSettings::UNSET, "{line}");
    }
}

#[test]
fn a_store_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("menubar.conf");
    let (mut s, _) = MenuBarSettings::load(&path, None).unwrap();
    s.clock.show_seconds = true;
    s.hidden_tray = vec!["a".into(), "b".into()];
    s.recent_apps = 3;
    assert!(s.store(&path).unwrap().is_empty());
    let text = std::fs::read_to_string(&path).unwrap();
    for line in [
        "clock.show_seconds = on\n",
        "tray.hidden = a,b\n",
        "recent.apps = 3\n",
    ] {
        assert!(text.contains(line), "{line:?} in {text}");
    }
    let (back, _) = MenuBarSettings::load(&path, None).unwrap();
    assert_eq!(back, s);
}
