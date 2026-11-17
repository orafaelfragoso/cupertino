//! `accessibility.conf`: the settings, a person's recorded keys and the machine's file.

use cupertino_defaults::accessibility::*;

fn apply(text: &str) -> (AccessibilitySettings, Vec<(usize, String)>) {
    let mut s = AccessibilitySettings::default();
    let problems = s.apply_text(text);
    (s, problems)
}

#[test]
fn a_missing_key_is_off_and_the_filter_defaults_are_the_design_ones() {
    let (s, problems) = apply("");
    assert!(problems.is_empty());
    assert_eq!(s, AccessibilitySettings::default());
    assert!(!s.display.reduce_motion && !s.display.reduce_transparency);
    assert!(!s.display.increase_contrast && !s.display.differentiate_without_color);
    assert!(!s.invert_colors && !s.screen_reader && !s.steady_caret);
    assert_eq!(s.display_contrast, 0.0);
    assert_eq!(s.filter(), None);
    assert_eq!(ColorFilter::Grayscale.default_intensity(), 1.0);
    assert_eq!(ColorFilter::Protanopia.default_intensity(), 0.5);
    assert_eq!(ColorFilter::Deuteranopia.default_intensity(), 0.5);
    assert_eq!(ColorFilter::Tritanopia.default_intensity(), 0.5);
    assert_eq!(ColorFilter::Tint { hue: 0.2 }.default_intensity(), 0.4);
}

#[test]
fn lines_set_the_options_and_bad_lines_are_reported_by_number() {
    let (s, problems) = apply(
        "reduce_motion = on\nreduce_transparency = yes\nincrease_contrast = 1\n\
         differentiate_without_color = true\ninvert_colors = on\ndisplay_contrast = 0.5\n\
         screen_reader = on\nsteady_caret = on\n# c\nreduce_motion = sometimes\nbare\nzoom = 3\n",
    );
    assert!(s.display.reduce_motion && s.display.reduce_transparency);
    assert!(s.display.increase_contrast && s.display.differentiate_without_color);
    assert!(s.invert_colors && s.screen_reader && s.steady_caret);
    assert_eq!(s.display_contrast, 0.5);
    assert_eq!(
        problems.iter().map(|p| p.0).collect::<Vec<_>>(),
        [10, 11, 12]
    );
    assert!(problems[0].1.contains("expected on or off"));
    assert!(problems[1].1.contains("key = value"));
    assert!(problems[2].1.contains("unknown setting"));
}

#[test]
fn the_contrast_and_the_filter_intensity_have_ranges() {
    let (_, p) =
        apply("display_contrast = 1.5\ncolor_filter_intensity = 0.1\ncolor_filter_intensity = 2\n");
    assert_eq!(p.len(), 3);
    assert!(p[0].1.contains("from 0 to 1"));
    assert!(p[1].1.contains("from 0.25 to 1"));
    let (s, p) = apply("color_filter = tritanopia\ncolor_filter_intensity = 0.25\n");
    assert!(p.is_empty());
    assert_eq!(
        s.filter(),
        Some((ColorFilter::Tritanopia, MIN_FILTER_INTENSITY))
    );
}

#[test]
fn colour_filters_parse_with_their_intensity() {
    for (text, filter) in [
        ("grayscale", ColorFilter::Grayscale),
        ("protanopia", ColorFilter::Protanopia),
        ("deuteranopia", ColorFilter::Deuteranopia),
        ("tritanopia", ColorFilter::Tritanopia),
        ("tint 0.25", ColorFilter::Tint { hue: 0.25 }),
    ] {
        let (s, p) = apply(&format!("color_filter = {text}\n"));
        assert!(p.is_empty(), "{text}");
        assert_eq!(s.filter(), Some((filter, filter.default_intensity())));
        assert_eq!(filter.to_string(), text);
    }
    let (s, _) = apply("color_filter = grayscale\ncolor_filter_intensity = 0.7\ncolor_filter_intensity = default\n");
    assert_eq!(s.color_filter_intensity, None);
    assert_eq!(apply("color_filter = off\n").0.filter(), None);
}

#[test]
fn bad_colour_filters_are_reported() {
    for bad in [
        "sepia",
        "tint",
        "tint 2",
        "tint x",
        "grayscale extra",
        "tint 0.1 0.2",
    ] {
        let (s, p) = apply(&format!("color_filter = {bad}\n"));
        assert_eq!(p.len(), 1, "{bad}");
        assert!(p[0].1.starts_with("color_filter:"), "{bad}");
        assert_eq!(s.color_filter, None);
    }
}

#[test]
fn the_text_reads_back_unchanged() {
    let mut s = AccessibilitySettings::default();
    s.display.reduce_motion = true;
    s.display.differentiate_without_color = true;
    s.invert_colors = true;
    s.display_contrast = 0.75;
    s.color_filter = Some(ColorFilter::Tint { hue: 0.6 });
    s.color_filter_intensity = Some(0.9);
    s.screen_reader = true;
    s.steady_caret = true;
    let (back, p) = apply(&s.to_text());
    assert!(p.is_empty());
    assert_eq!(back, s);
    assert_eq!(back.to_text(), s.to_text());
}

#[test]
fn one_key_is_set_as_the_file_writes_it_and_a_bad_value_changes_nothing() {
    let mut s = AccessibilitySettings::default();
    s.set("reduce_motion", "on").unwrap();
    assert!(s.display.reduce_motion);
    assert!(s.set("display_contrast", "9").is_err());
    assert_eq!(s.display_contrast, 0.0);
    assert!(s.set("nope", "on").unwrap_err().contains("unknown setting"));
}

#[test]
fn the_display_options_are_addressed_by_their_keys() {
    let mut o = DisplayOptions::default();
    for option in DisplayOption::ALL {
        assert!(!o.get(option));
        o.set(option, true);
        assert!(o.get(option), "{}", option.key());
    }
    assert_eq!(
        DisplayOption::ALL.map(|o| o.key()),
        [
            "reduce_motion",
            "reduce_transparency",
            "increase_contrast",
            "differentiate_without_color"
        ]
    );
}

#[test]
fn a_steady_caret_is_off_until_a_person_turns_it_on() {
    assert_eq!(STEADY_CARET, "steady_caret");
    assert!(!AccessibilitySettings::default().steady_caret);
    let (p, errors) = PersonalAccessibility::from_text("steady_caret = on\n");
    assert!(errors.is_empty());
    assert!(p.over(&AccessibilitySettings::default()).steady_caret);
}

#[test]
fn the_machines_file_sets_only_its_keys_and_reports_every_other() {
    let (m, problems) = SystemAccessibility::from_text(
        "reduce_motion = on\nscreen_reader_at_login = on\ninvert_colors = on\nscreen_reader = on\nsteady_caret = on\n",
    );
    assert!(m.settings.display.reduce_motion);
    assert!(m.screen_reader_at_login);
    assert!(!m.settings.invert_colors && !m.settings.screen_reader && !m.settings.steady_caret);
    assert_eq!(problems.iter().map(|p| p.0).collect::<Vec<_>>(), [3, 4, 5]);
    assert!(problems[0]
        .1
        .contains("not a setting of the machine's file"));
    assert_eq!(SYSTEM_KEYS, ["reduce_motion", SCREEN_READER_AT_LOGIN]);
}

#[test]
fn the_machines_text_reads_back() {
    let mut m = SystemAccessibility::default();
    m.settings.display.reduce_motion = true;
    m.screen_reader_at_login = true;
    let (back, p) = SystemAccessibility::from_text(&m.to_text());
    assert!(p.is_empty());
    assert_eq!(back, m);
    assert!(m.to_text().contains("screen_reader_at_login = on"));
    assert!(SystemAccessibility::default()
        .set("invert_colors", "on")
        .is_err());
}

#[test]
fn a_persons_file_cannot_start_the_screen_reader_at_login() {
    let (p, problems) = PersonalAccessibility::from_text("screen_reader_at_login = on\n");
    assert_eq!(problems.len(), 1);
    assert!(problems[0].1.contains("unknown setting"));
    assert_eq!(p.to_text(), "");
    assert_eq!(SCREEN_READER_AT_LOGIN, "screen_reader_at_login");
}

#[test]
fn a_persons_file_runs_the_session_reader_and_the_machines_file_cannot() {
    let (p, problems) = PersonalAccessibility::from_text("screen_reader = on\n");
    assert!(problems.is_empty());
    assert!(p.over(&AccessibilitySettings::default()).screen_reader);
    let (m, problems) = SystemAccessibility::from_text("screen_reader = on\n");
    assert_eq!(problems.len(), 1);
    assert!(!m.settings.screen_reader);
    assert_eq!(SCREEN_READER, "screen_reader");
}

#[test]
fn a_person_who_changed_one_key_has_the_machines_value_for_every_other() {
    let (machine, _) = SystemAccessibility::from_text("reduce_motion = on\n");
    let (person, _) = PersonalAccessibility::from_text("increase_contrast = on\n");
    let s = person.over(&machine.settings);
    assert!(s.display.reduce_motion, "from the machine");
    assert!(s.display.increase_contrast, "from the person");
    let (person, _) = PersonalAccessibility::from_text("reduce_motion = off\n");
    assert!(
        !person.over(&machine.settings).display.reduce_motion,
        "the person's own value wins"
    );
}

#[test]
fn a_file_with_every_key_sets_them_all() {
    let s = AccessibilitySettings {
        invert_colors: true,
        ..AccessibilitySettings::default()
    };
    let (p, problems) = PersonalAccessibility::from_text(&s.to_text());
    assert!(problems.is_empty());
    let machine = SystemAccessibility::from_text("reduce_motion = on\n")
        .0
        .settings;
    let over = p.over(&machine);
    assert!(!over.display.reduce_motion, "recorded as off");
    assert!(over.invert_colors);
}

#[test]
fn a_changed_key_is_recorded_and_stays_the_persons_when_set_back() {
    let machine = SystemAccessibility::from_text("reduce_motion = on\n")
        .0
        .settings;
    let mut person = PersonalAccessibility::default();
    assert_eq!(person.over(&machine), machine);
    let mut after = machine;
    after.display.reduce_motion = false;
    person.record(&machine, &after);
    assert_eq!(person.to_text(), "reduce_motion = off\n");
    person.record(&after, &machine);
    assert_eq!(
        person.to_text(),
        "reduce_motion = on\n",
        "still recorded, now at the machine's value"
    );
}

#[test]
fn a_recorded_key_follows_the_person_when_the_machine_changes() {
    let mut person = PersonalAccessibility::default();
    let before = AccessibilitySettings::default();
    let mut after = before;
    after.invert_colors = true;
    person.record(&before, &after);
    let machine_now = SystemAccessibility::from_text("reduce_motion = on\n")
        .0
        .settings;
    let s = person.over(&machine_now);
    assert!(s.invert_colors && s.display.reduce_motion);
}

#[test]
fn the_personal_file_is_stored_and_read_back_and_a_missing_one_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino/accessibility.conf");
    let (none, p) = PersonalAccessibility::load(&path).unwrap();
    assert!(p.is_empty());
    assert_eq!(none, PersonalAccessibility::default());
    let (mut person, _) =
        PersonalAccessibility::from_text("display_contrast = 0.25\nsteady_caret = off\n");
    person.store(&path).unwrap();
    let (back, p) = PersonalAccessibility::load(&path).unwrap();
    assert!(p.is_empty());
    assert_eq!(back, person);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "display_contrast = 0.25\nsteady_caret = off\n"
    );
    person = PersonalAccessibility::default();
    assert_eq!(person.to_text(), "");
}

#[test]
fn the_documented_names_and_locations() {
    assert_eq!(ACCESSIBILITY_ENV, "CUPERTINO_ACCESSIBILITY_CONF");
    assert_eq!(ACCESSIBILITY_FILE, "accessibility.conf");
    assert_eq!(SYSTEM_ACCESSIBILITY, "/etc/cupertino/accessibility.conf");
    assert_eq!(SCREEN_READERS, ["/usr/bin/orca"]);
}

#[test]
fn an_installed_screen_reader_is_one_of_the_listed_programs_only_root_can_change() {
    match installed_screen_reader() {
        None => {}
        Some(path) => assert!(path.is_absolute()),
    }
}
