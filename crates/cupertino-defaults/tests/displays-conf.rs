//! `displays.conf`: one section per display, every key required.

use cupertino_defaults::displays::*;

fn settings(w: u32, scale: f64, transform: Transform) -> DisplaySettings {
    DisplaySettings {
        enabled: true,
        mode: ModeSetting {
            width: w,
            height: 1080,
            refresh_mhz: 59_940,
        },
        position: (-1920, 0),
        scale,
        transform,
    }
}

const SECTION: &str = "[connector DP-1]\nenabled = true\nmode = 3840x2160@60000\nposition = 0,0\nscale = 1.5\ntransform = normal\n";

#[test]
fn a_section_gives_the_settings_of_its_display_and_comments_are_ignored() {
    let text = format!(
        "# header\n\n{}  # trailing\n",
        SECTION.replace("scale = 1.5", "scale = 2 # hi")
    );
    let (c, problems) = DisplaysConfig::parse(&text);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        c.get("connector DP-1"),
        Some(&DisplaySettings {
            enabled: true,
            mode: ModeSetting {
                width: 3840,
                height: 2160,
                refresh_mhz: 60_000
            },
            position: (0, 0),
            scale: 2.0,
            transform: Transform::Normal,
        })
    );
    assert!(c.get("connector DP-2").is_none());
}

#[test]
fn the_file_round_trips_through_its_text_and_through_a_store() {
    let transforms = [
        Transform::Normal,
        Transform::Rotate90,
        Transform::Rotate180,
        Transform::Rotate270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];
    let mut c = DisplaysConfig::default();
    for (i, transform) in transforms.into_iter().enumerate() {
        c.set(
            &format!("Dell P2723QE {i}"),
            settings(1920 + i as u32, 1.0 + i as f64 / 4.0, transform),
        );
    }
    let (back, problems) = DisplaysConfig::parse(&c.to_text());
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(back, c);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino/displays.conf");
    assert_eq!(
        DisplaysConfig::load(&path).unwrap(),
        (DisplaysConfig::default(), vec![]),
        "a missing file"
    );
    c.store(&path).unwrap();
    assert_eq!(
        DisplaysConfig::load(&path).unwrap(),
        (c, vec![]),
        "the directory is created"
    );
}

#[test]
fn setting_a_key_again_replaces_its_settings_in_place() {
    let mut c = DisplaysConfig::default();
    c.set("a", settings(1000, 1.0, Transform::Normal));
    c.set("b", settings(1100, 1.0, Transform::Normal));
    c.set("a", settings(1200, 2.0, Transform::Normal));
    assert_eq!(c.displays.len(), 2);
    assert_eq!(c.displays[0].0, "a");
    assert_eq!(c.get("a").unwrap().mode.width, 1200);
}

#[test]
fn a_section_with_a_problem_is_dropped_whole_and_the_others_stay() {
    for (row, section, reason) in [
        (
            "a bad value",
            "[connector DP-2]\nenabled = true\nmode = 1920x1080@60000\nposition = 0,0\nscale = 0\ntransform = normal\n",
            "[connector DP-2] ignored",
        ),
        ("a missing key", "[connector DP-2]\nenabled = true\nmode = 1x1@1\n", "[connector DP-2] lacks position, scale, transform"),
    ] {
        let (c, problems) = DisplaysConfig::parse(&format!("{SECTION}\n{section}"));
        assert!(c.get("connector DP-1").is_some(), "{row}");
        assert!(c.get("connector DP-2").is_none(), "{row}");
        assert!(problems.iter().any(|p| p.line == 8 && p.problem == reason), "{row}: {problems:?}");
    }
}

#[test]
fn rejected_lines_say_what_was_expected() {
    for (text, line, expected) in [
        ("[x]\nenabled = yes", 2, "expected true or false"),
        ("[x]\nmode = 1920x1080", 2, "WIDTHxHEIGHT@MILLIHERTZ"),
        ("[x]\nmode = 0x1080@60000", 2, "WIDTHxHEIGHT@MILLIHERTZ"),
        ("[x]\nposition = 1;2", 2, "expected X,Y"),
        ("[x]\nscale = fat", 2, "expected a number"),
        ("[x]\nscale = -1", 2, "must be above zero"),
        ("[x]\ntransform = upside-down", 2, "transform: unknown"),
        ("[x]\ntransform = 45", 2, "transform: unknown"),
        ("[x]\ncolour = red", 2, "unknown setting"),
        ("[x]\nbare", 2, "key = value"),
        ("scale = 2", 1, "outside a display section"),
        ("[]", 1, "[display key]"),
        ("\n\n[open", 3, "[display key]"),
    ] {
        let (_, problems) = DisplaysConfig::parse(&format!("{text}\n"));
        assert!(
            problems
                .iter()
                .any(|p| p.line == line && p.problem.contains(expected)),
            "{text:?}: {problems:?}"
        );
    }
}

#[test]
fn a_display_key_prefers_the_serial_and_cannot_end_a_section_header() {
    for (make, model, serial, expected) in [
        (" Dell ", "P2723QE", Some(" 123 "), "Dell P2723QE 123"),
        ("Dell", "P2723QE", None, "connector DP-1"),
        ("Dell", "P2723QE", Some("  "), "connector DP-1"),
        ("A]B", "M\nN", Some("1"), "A_B M_N 1"),
    ] {
        assert_eq!(
            display_key("DP-1", make, model, serial),
            expected,
            "{make:?} {model:?} {serial:?}"
        );
    }
}
