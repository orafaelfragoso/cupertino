//! `displays.conf`: one section per display, every key required.

use cupertino_defaults::displays::*;

fn settings(w: u32, scale: f64) -> DisplaySettings {
    DisplaySettings {
        enabled: true,
        mode: ModeSetting {
            width: w,
            height: 1080,
            refresh_mhz: 59_940,
        },
        position: (-1920, 0),
        scale,
        transform: Transform::Rotate90,
    }
}

const SECTION: &str = "[connector DP-1]\nenabled = true\nmode = 3840x2160@60000\nposition = 0,0\nscale = 1.5\ntransform = normal\n";

#[test]
fn a_section_gives_the_settings_of_its_display() {
    let (c, problems) = DisplaysConfig::parse(SECTION);
    assert!(problems.is_empty(), "{problems:?}");
    let s = c.get("connector DP-1").unwrap();
    assert!(s.enabled);
    assert_eq!(
        s.mode,
        ModeSetting {
            width: 3840,
            height: 2160,
            refresh_mhz: 60_000
        }
    );
    assert_eq!(s.position, (0, 0));
    assert_eq!(s.scale, 1.5);
    assert_eq!(s.transform, Transform::Normal);
    assert!(c.get("connector DP-2").is_none());
}

#[test]
fn the_file_round_trips_through_its_text() {
    let mut c = DisplaysConfig::default();
    c.set("Dell P2723QE ABC123", settings(3840, 1.5));
    c.set("connector HDMI-A-1", settings(1920, 1.0));
    let (back, problems) = DisplaysConfig::parse(&c.to_text());
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(back, c);
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let text = format!(
        "# header\n\n{}  # trailing\n",
        SECTION.replace("scale = 1.5", "scale = 2 # hi")
    );
    let (c, problems) = DisplaysConfig::parse(&text);
    assert!(problems.is_empty());
    assert_eq!(c.get("connector DP-1").unwrap().scale, 2.0);
}

#[test]
fn setting_a_key_again_replaces_its_settings_in_place() {
    let mut c = DisplaysConfig::default();
    c.set("a", settings(1000, 1.0));
    c.set("b", settings(1100, 1.0));
    c.set("a", settings(1200, 2.0));
    assert_eq!(c.displays.len(), 2);
    assert_eq!(c.displays[0].0, "a");
    assert_eq!(c.get("a").unwrap().mode.width, 1200);
}

#[test]
fn a_section_with_a_problem_is_dropped_whole_and_the_others_stay() {
    let text = format!("{SECTION}\n[connector DP-2]\nenabled = true\nmode = 1920x1080@60000\nposition = 0,0\nscale = 0\ntransform = normal\n");
    let (c, problems) = DisplaysConfig::parse(&text);
    assert!(c.get("connector DP-1").is_some());
    assert!(c.get("connector DP-2").is_none());
    assert!(problems
        .iter()
        .any(|p| p.problem.contains("scale: must be above zero")));
    assert!(problems
        .iter()
        .any(|p| p.problem == "[connector DP-2] ignored"));
}

#[test]
fn a_section_lacking_a_key_is_reported_with_the_missing_names() {
    let (c, problems) = DisplaysConfig::parse("[x]\nenabled = true\nmode = 1x1@1\n");
    assert!(c.displays.is_empty());
    assert!(problems
        .iter()
        .any(|p| p.problem == "[x] lacks position, scale, transform"));
    assert_eq!(problems[0].line, 1);
}

#[test]
fn rejected_values_say_what_was_expected() {
    for (line, expect) in [
        ("enabled = yes", "expected true or false"),
        ("mode = 1920x1080", "WIDTHxHEIGHT@MILLIHERTZ"),
        ("mode = 0x1080@60000", "WIDTHxHEIGHT@MILLIHERTZ"),
        ("position = 1;2", "expected X,Y"),
        ("scale = fat", "expected a number"),
        ("scale = -1", "must be above zero"),
        ("transform = upside-down", "transform: unknown"),
        ("colour = red", "unknown setting"),
        ("bare", "key = value"),
    ] {
        let (_, problems) = DisplaysConfig::parse(&format!("[x]\n{line}\n"));
        assert!(
            problems
                .iter()
                .any(|p| p.line == 2 && p.problem.contains(expect)),
            "{line}: {problems:?}"
        );
    }
}

#[test]
fn a_setting_outside_a_section_and_a_bad_header_are_problems() {
    let (_, problems) = DisplaysConfig::parse("scale = 2\n[]\n[open\n");
    assert_eq!(problems.len(), 3);
    assert!(problems[0].problem.contains("outside a display section"));
    assert!(problems[1].problem.contains("[display key]"));
    assert_eq!(problems[2].line, 3);
}

#[test]
fn a_problem_prints_its_line_and_message() {
    let (_, problems) = DisplaysConfig::parse("scale = 2\n");
    let text = problems[0].to_string();
    assert!(
        text.contains('1') && text.contains("outside a display section"),
        "{text}"
    );
}

#[test]
fn transforms_have_names_that_read_back() {
    let names = [
        "normal",
        "90",
        "180",
        "270",
        "flipped",
        "flipped-90",
        "flipped-180",
        "flipped-270",
    ];
    for name in names {
        assert_eq!(Transform::from_name(name).unwrap().name(), name);
    }
    assert_eq!(Transform::from_name("45"), None);
}

#[test]
fn a_display_key_prefers_the_serial_and_falls_back_to_the_connector() {
    assert_eq!(
        display_key("DP-1", " Dell ", "P2723QE", Some(" 123 ")),
        "Dell P2723QE 123"
    );
    assert_eq!(
        display_key("DP-1", "Dell", "P2723QE", None),
        "connector DP-1"
    );
    assert_eq!(
        display_key("DP-1", "Dell", "P2723QE", Some("  ")),
        "connector DP-1"
    );
}

#[test]
fn characters_that_would_end_a_section_header_become_underscores() {
    assert_eq!(display_key("c", "A]B", "M\nN", Some("1")), "A_B M_N 1");
}

#[test]
fn a_missing_file_is_an_empty_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let (c, problems) = DisplaysConfig::load(&dir.path().join("displays.conf")).unwrap();
    assert!(c.displays.is_empty() && problems.is_empty());
}

#[test]
fn store_and_load_round_trip_and_create_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino/displays.conf");
    let mut c = DisplaysConfig::default();
    c.set("connector DP-1", settings(2560, 1.25));
    c.store(&path).unwrap();
    let (back, problems) = DisplaysConfig::load(&path).unwrap();
    assert!(problems.is_empty());
    assert_eq!(back, c);
}

#[test]
fn the_variable_and_file_name_are_the_documented_ones() {
    assert_eq!(DISPLAYS_ENV, "CUPERTINO_DISPLAYS_CONFIG");
    assert_eq!(DISPLAYS_FILE, "displays.conf");
}
