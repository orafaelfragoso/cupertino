//! `appearance.conf`: the automatic look and its schedule, accent and highlight, sidebar icon size, scroll bar click
//! and title bar double click.

use cupertino_defaults::appearance::*;

#[test]
fn an_empty_file_reads_as_the_unset_settings() {
    let (s, problems) = AppearanceSettings::parse("", None);
    assert!(problems.is_empty());
    assert_eq!(s, AppearanceSettings::UNSET);
    assert!(!s.auto);
    assert_eq!(s.auto_dark_from, 19 * 60);
    assert_eq!(s.auto_light_from, 7 * 60);
    assert_eq!(s.accent, Accent::Multicolor);
    assert_eq!(s.highlight, Highlight::Automatic);
    assert_eq!(s.sidebar_icons, SidebarIcons::Medium);
    assert_eq!(s.scroll_click, ScrollClick::NextPage);
    assert!(s.tint_windows);
    assert_eq!(s.titlebar_double_click, TitlebarAction::Zoom);
}

#[test]
fn every_key_is_read() {
    let (s, problems) = AppearanceSettings::parse(
        "auto = on\nauto_dark_from = 21:30\nauto_light_from = 06:05\naccent = green\n\
         highlight = pink\nsidebar_icons = large\nscroll_click = jump-to-spot\n\
         tint_windows = off\ntitlebar_double_click = minimize\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(s.auto);
    assert_eq!(s.auto_dark_from, 21 * 60 + 30);
    assert_eq!(s.auto_light_from, 6 * 60 + 5);
    assert_eq!(s.accent, Accent::Green);
    assert_eq!(s.highlight, Highlight::Color(Accent::Pink));
    assert_eq!(s.sidebar_icons, SidebarIcons::Large);
    assert_eq!(s.scroll_click, ScrollClick::JumpToSpot);
    assert!(!s.tint_windows);
    assert_eq!(s.titlebar_double_click, TitlebarAction::Minimize);
}

#[test]
fn rejected_values_are_reported_with_the_expected_choices() {
    let (_, problems) = AppearanceSettings::parse(
        "accent = mauve\nhighlight = multicolor\nsidebar_icons = huge\nscroll_click = left\n\
         titlebar_double_click = shrink\nauto_dark_from = 24:00\nauto_light_from = 7\nauto = maybe\n",
        None,
    );
    assert_eq!(problems.len(), 8);
    assert!(problems[0].message.contains("accent colour name"));
    assert!(problems[1].message.contains("automatic or a colour name"));
    assert!(problems[2].message.contains("small, medium or large"));
    assert!(problems[3].message.contains("next-page or jump-to-spot"));
    assert!(problems[4].message.contains("zoom, minimize or none"));
    assert!(problems[5].message.contains("HH:MM"));
    assert!(problems[6].message.contains("HH:MM"));
    assert!(problems[7].message.contains("on or off"));
}

#[test]
fn a_bad_value_keeps_the_last_good_one() {
    let (good, _) = AppearanceSettings::parse("accent = red\nauto = on\n", None);
    let (s, problems) = AppearanceSettings::parse("accent = mauve\nauto = off\n", Some(&good));
    assert_eq!(problems.len(), 1);
    assert_eq!(s.accent, Accent::Red);
    assert!(!s.auto);
}

#[test]
fn times_round_trip_between_text_and_minutes() {
    assert_eq!(parse_time("00:00"), Ok(0));
    assert_eq!(parse_time("23:59"), Ok(23 * 60 + 59));
    assert_eq!(parse_time(" 7 : 05 "), Ok(7 * 60 + 5));
    assert!(parse_time("7").is_err());
    assert!(parse_time("12:60").is_err());
    assert!(parse_time("ab:cd").is_err());
    assert_eq!(time_text(7 * 60 + 5), "07:05");
}

#[test]
fn the_schedule_runs_across_midnight_and_equal_times_mean_always_light() {
    let mut s = AppearanceSettings::UNSET;
    assert!(s.dark_at(23 * 60));
    assert!(s.dark_at(0));
    assert!(s.dark_at(6 * 60 + 59));
    assert!(!s.dark_at(7 * 60));
    assert!(!s.dark_at(12 * 60));
    assert!(s.dark_at(19 * 60));
    s.auto_dark_from = 9 * 60;
    s.auto_light_from = 17 * 60;
    assert!(s.dark_at(12 * 60));
    assert!(!s.dark_at(18 * 60));
    s.auto_light_from = 9 * 60;
    assert!(!s.dark_at(0) && !s.dark_at(9 * 60) && !s.dark_at(15 * 60));
}

#[test]
fn accents_have_names_that_read_back_and_a_colour_for_each_look() {
    for accent in Accent::ALL {
        assert_eq!(Accent::from_name(accent.name()), Some(accent));
    }
    assert_eq!(Accent::from_name("nope"), None);
    assert_eq!(Accent::Multicolor.rgb(false), Accent::Blue.rgb(true));
    assert_ne!(Accent::Red.rgb(false), Accent::Red.rgb(true));
}

#[test]
fn a_store_round_trips_and_keeps_comments_and_unknown_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appearance.conf");
    std::fs::write(&path, "# look\naccent = red  # mine\nfuture = 1\n").unwrap();
    let (mut s, problems) = AppearanceSettings::load(&path, None).unwrap();
    assert_eq!(problems.len(), 1);
    s.accent = Accent::Purple;
    s.auto = true;
    s.highlight = Highlight::Color(Accent::Orange);
    s.store(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("# look\naccent = purple  # mine\nfuture = 1\n"),
        "{text}"
    );
    let (back, _) = AppearanceSettings::load(&path, None).unwrap();
    assert_eq!(back, s);
}

#[test]
fn a_missing_file_loads_as_the_unset_settings() {
    let dir = tempfile::tempdir().unwrap();
    let (s, problems) = AppearanceSettings::load(&dir.path().join("none.conf"), None).unwrap();
    assert_eq!(s, AppearanceSettings::UNSET);
    assert!(problems.is_empty());
}

#[test]
fn the_environment_variable_redirects_the_file_that_current_reads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.conf");
    std::fs::write(&path, "accent = graphite\n").unwrap();
    std::env::set_var(APPEARANCE_CONF_ENV, &path);
    assert_eq!(AppearanceSettings::path(), Some(path));
    assert_eq!(AppearanceSettings::current().accent, Accent::Graphite);
}
