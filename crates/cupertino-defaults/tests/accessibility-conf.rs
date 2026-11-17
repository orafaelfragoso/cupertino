//! `accessibility.conf`: the settings, a person's recorded keys over the machine's file.

use cupertino_defaults::accessibility::*;

fn apply(text: &str) -> (AccessibilitySettings, Vec<(usize, String)>) {
    let mut s = AccessibilitySettings::default();
    let problems = s.apply_text(text);
    (s, problems)
}

fn machine_settings(text: &str) -> AccessibilitySettings {
    SystemAccessibility::from_text(text).0.settings
}

#[test]
fn every_key_is_read_and_the_text_reads_back_unchanged() {
    let (s, problems) = apply(
        "reduce_motion = on\nreduce_transparency = yes\nincrease_contrast = 1\n\
         differentiate_without_color = true\ninvert_colors = on\ndisplay_contrast = 0.5\n\
         color_filter = tint 0.6\ncolor_filter_intensity = 0.9\nscreen_reader = on\nsteady_caret = on\n",
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        s,
        AccessibilitySettings {
            display: DisplayOptions {
                reduce_motion: true,
                reduce_transparency: true,
                increase_contrast: true,
                differentiate_without_color: true,
            },
            invert_colors: true,
            display_contrast: 0.5,
            color_filter: Some(ColorFilter::Tint { hue: 0.6 }),
            color_filter_intensity: Some(0.9),
            screen_reader: true,
            steady_caret: true,
        }
    );
    let (back, problems) = apply(&s.to_text());
    assert!(problems.is_empty());
    assert_eq!(back, s);
}

#[test]
fn rejected_lines_are_reported_by_number_and_change_nothing() {
    for (line, expected) in [
        ("reduce_motion = sometimes", "expected on or off"),
        ("bare", "key = value"),
        ("zoom = 3", "unknown setting"),
        ("display_contrast = 1.5", "from 0 to 1"),
        ("color_filter_intensity = 0.1", "from 0.25 to 1"),
        ("color_filter_intensity = 2", "from 0.25 to 1"),
        ("color_filter = sepia", "color_filter:"),
        ("color_filter = tint", "color_filter:"),
        ("color_filter = tint 2", "color_filter:"),
        ("color_filter = tint x", "color_filter:"),
        ("color_filter = grayscale extra", "color_filter:"),
        ("color_filter = tint 0.1 0.2", "color_filter:"),
    ] {
        let (s, problems) = apply(&format!("# c\n{line}\n"));
        assert_eq!(problems.len(), 1, "{line}");
        assert_eq!(problems[0].0, 2, "{line}");
        assert!(
            problems[0].1.contains(expected),
            "{line}: {}",
            problems[0].1
        );
        assert_eq!(s, AccessibilitySettings::default(), "{line}");
    }
}

#[test]
fn a_colour_filter_has_its_default_intensity_until_one_is_set() {
    for (text, filter, intensity) in [
        ("grayscale", ColorFilter::Grayscale, 1.0),
        ("protanopia", ColorFilter::Protanopia, 0.5),
        ("deuteranopia", ColorFilter::Deuteranopia, 0.5),
        ("tritanopia", ColorFilter::Tritanopia, 0.5),
        ("tint 0.25", ColorFilter::Tint { hue: 0.25 }, 0.4),
    ] {
        let (s, p) = apply(&format!("color_filter = {text}\n"));
        assert!(p.is_empty(), "{text}");
        assert_eq!(s.filter(), Some((filter, intensity)), "{text}");
        assert_eq!(filter.to_string(), text, "the file's word");
    }
    let (s, _) = apply("color_filter = tritanopia\ncolor_filter_intensity = 0.25\n");
    assert_eq!(
        s.filter(),
        Some((ColorFilter::Tritanopia, MIN_FILTER_INTENSITY))
    );
    let (s, _) = apply("color_filter = grayscale\ncolor_filter_intensity = 0.7\ncolor_filter_intensity = default\n");
    assert_eq!(s.filter(), Some((ColorFilter::Grayscale, 1.0)));
    assert_eq!(apply("color_filter = off\n").0.filter(), None);
}

#[test]
fn one_key_is_set_as_the_file_writes_it_and_a_bad_value_changes_nothing() {
    let mut s = AccessibilitySettings::default();
    for option in DisplayOption::ALL {
        s.set(option.key(), "on").unwrap();
        assert!(s.display.get(option), "{}", option.key());
    }
    assert!(s.set("display_contrast", "9").is_err());
    assert_eq!(s.display_contrast, 0.0);
    assert!(s.set("nope", "on").unwrap_err().contains("unknown setting"));
}

#[test]
fn the_machines_file_holds_only_the_machine_keys_and_reads_back() {
    let (m, problems) = SystemAccessibility::from_text(
        "reduce_motion = on\nscreen_reader_at_login = on\ninvert_colors = on\nscreen_reader = on\nsteady_caret = on\n",
    );
    assert!(m.settings.display.reduce_motion && m.screen_reader_at_login);
    assert!(!m.settings.invert_colors);
    assert!(
        !m.settings.screen_reader,
        "the machine's file cannot run the session reader"
    );
    assert!(!m.settings.steady_caret);
    assert_eq!(problems.iter().map(|p| p.0).collect::<Vec<_>>(), [3, 4, 5]);
    assert!(problems[0]
        .1
        .contains("not a setting of the machine's file"));
    assert!(SystemAccessibility::default()
        .set("invert_colors", "on")
        .is_err());
    let (back, p) = SystemAccessibility::from_text(&m.to_text());
    assert!(p.is_empty());
    assert_eq!(back, m);
}

#[test]
fn a_persons_file_cannot_start_the_screen_reader_at_login() {
    let (p, problems) =
        PersonalAccessibility::from_text("screen_reader_at_login = on\nscreen_reader = on\n");
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].0, 1);
    assert!(problems[0].1.contains("unknown setting"));
    assert_eq!(p.to_text(), "screen_reader = on\n");
    assert!(
        p.over(&AccessibilitySettings::default()).screen_reader,
        "the session reader is the person's"
    );
}

#[test]
fn a_persons_recorded_keys_win_and_the_machine_gives_every_other() {
    let machine = machine_settings("reduce_motion = on\n");
    for (row, person, reduce_motion, increase_contrast) in [
        ("nothing recorded", "", true, false),
        (
            "another key recorded",
            "increase_contrast = on\n",
            true,
            true,
        ),
        (
            "the same key recorded off",
            "reduce_motion = off\n",
            false,
            false,
        ),
    ] {
        let (person, problems) = PersonalAccessibility::from_text(person);
        assert!(problems.is_empty(), "{row}");
        let s = person.over(&machine);
        assert_eq!(s.display.reduce_motion, reduce_motion, "{row}");
        assert_eq!(s.display.increase_contrast, increase_contrast, "{row}");
    }
    let every_key = AccessibilitySettings {
        invert_colors: true,
        ..AccessibilitySettings::default()
    }
    .to_text();
    let s = PersonalAccessibility::from_text(&every_key)
        .0
        .over(&machine);
    assert!(
        !s.display.reduce_motion,
        "a file with every key records each of them"
    );
    assert!(s.invert_colors);
}

#[test]
fn a_changed_key_is_recorded_and_stays_the_persons_when_set_back() {
    let machine = machine_settings("reduce_motion = on\n");
    let mut person = PersonalAccessibility::default();
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
    let machine_later = machine_settings("reduce_motion = off\nscreen_reader_at_login = on\n");
    assert!(
        person.over(&machine_later).display.reduce_motion,
        "the person's value follows them"
    );
}

#[test]
fn the_personal_file_is_stored_and_read_back_and_a_missing_one_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino/accessibility.conf");
    assert_eq!(
        PersonalAccessibility::load(&path).unwrap(),
        (PersonalAccessibility::default(), vec![])
    );
    let (person, _) =
        PersonalAccessibility::from_text("display_contrast = 0.25\nsteady_caret = off\n");
    person.store(&path).unwrap();
    assert_eq!(
        PersonalAccessibility::load(&path).unwrap(),
        (person, vec![])
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "display_contrast = 0.25\nsteady_caret = off\n"
    );
}
