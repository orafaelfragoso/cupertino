//! `appearance.conf`: the automatic look and its schedule, accent and highlight, sidebar icon size, scroll bar click
//! and title bar double click.

use cupertino_defaults::appearance::*;

#[test]
fn every_key_is_read() {
    let (s, problems) = AppearanceSettings::parse(
        "auto = on\nauto_dark_from = 21:30\nauto_light_from = 06:05\naccent = green\n\
         highlight = pink\nsidebar_icons = large\nscroll_click = jump-to-spot\n\
         tint_windows = off\ntitlebar_double_click = minimize\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        s,
        AppearanceSettings {
            auto: true,
            auto_dark_from: 21 * 60 + 30,
            auto_light_from: 6 * 60 + 5,
            accent: Accent::Green,
            highlight: Highlight::Color(Accent::Pink),
            sidebar_icons: SidebarIcons::Large,
            scroll_click: ScrollClick::JumpToSpot,
            tint_windows: false,
            titlebar_double_click: TitlebarAction::Minimize,
        }
    );
    for accent in Accent::ALL {
        let (s, problems) =
            AppearanceSettings::parse(&format!("accent = {}\n", accent.name()), None);
        assert!(problems.is_empty(), "{accent:?}");
        assert_eq!(s.accent, accent);
    }
}

#[test]
fn rejected_values_name_the_expected_choices() {
    for (line, expected) in [
        ("accent = mauve", "accent colour name"),
        ("highlight = multicolor", "automatic or a colour name"),
        ("sidebar_icons = huge", "small, medium or large"),
        ("scroll_click = left", "next-page or jump-to-spot"),
        ("titlebar_double_click = shrink", "zoom, minimize or none"),
        ("auto_dark_from = 24:00", "HH:MM"),
        ("auto_light_from = 7", "HH:MM"),
        ("auto = maybe", "on or off"),
    ] {
        let (s, problems) = AppearanceSettings::parse(&format!("{line}\n"), None);
        assert_eq!(problems.len(), 1, "{line}");
        assert!(
            problems[0].message.contains(expected),
            "{line}: {}",
            problems[0].message
        );
        assert_eq!(s, AppearanceSettings::UNSET, "{line}");
    }
}

#[test]
fn times_read_as_minutes_and_write_as_hh_mm() {
    for (text, expected) in [
        ("00:00", Some(0)),
        ("23:59", Some(23 * 60 + 59)),
        (" 7 : 05 ", Some(7 * 60 + 5)),
        ("7", None),
        ("12:60", None),
        ("ab:cd", None),
    ] {
        assert_eq!(parse_time(text).ok(), expected, "{text:?}");
    }
    assert_eq!(time_text(7 * 60 + 5), "07:05");
}

#[test]
fn the_schedule_runs_across_midnight_and_equal_times_mean_always_light() {
    for (row, dark_from, light_from, at, dark) in [
        ("night, late", 19 * 60, 7 * 60, 23 * 60, true),
        ("night, midnight", 19 * 60, 7 * 60, 0, true),
        ("night, before light", 19 * 60, 7 * 60, 6 * 60 + 59, true),
        ("light from its time", 19 * 60, 7 * 60, 7 * 60, false),
        ("noon", 19 * 60, 7 * 60, 12 * 60, false),
        ("dark from its time", 19 * 60, 7 * 60, 19 * 60, true),
        ("dark by day", 9 * 60, 17 * 60, 12 * 60, true),
        ("light by night", 9 * 60, 17 * 60, 18 * 60, false),
        ("equal times, midnight", 9 * 60, 9 * 60, 0, false),
        ("equal times, at the time", 9 * 60, 9 * 60, 9 * 60, false),
        ("equal times, afternoon", 9 * 60, 9 * 60, 15 * 60, false),
    ] {
        let s = AppearanceSettings {
            auto_dark_from: dark_from,
            auto_light_from: light_from,
            ..AppearanceSettings::UNSET
        };
        assert_eq!(s.dark_at(at), dark, "{row}");
    }
}

#[test]
fn multicolor_is_blue_in_light_and_an_accent_differs_between_the_looks() {
    assert_eq!(Accent::Multicolor.rgb(false), Accent::Blue.rgb(true));
    assert_ne!(Accent::Red.rgb(false), Accent::Red.rgb(true));
}

#[test]
fn a_store_writes_every_changed_key_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appearance.conf");
    let s = AppearanceSettings {
        auto: true,
        accent: Accent::Purple,
        highlight: Highlight::Color(Accent::Orange),
        auto_dark_from: 20 * 60,
        sidebar_icons: SidebarIcons::Small,
        scroll_click: ScrollClick::JumpToSpot,
        tint_windows: false,
        titlebar_double_click: TitlebarAction::None,
        ..AppearanceSettings::UNSET
    };
    assert!(s.store(&path).unwrap().is_empty());
    assert_eq!(AppearanceSettings::load(&path, None).unwrap(), (s, vec![]));
}
