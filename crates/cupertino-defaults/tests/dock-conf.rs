//! `dock.conf`: pins, options, the design's size validation and the corner and gesture actions.

use cupertino_defaults::dock::*;

#[test]
fn every_key_is_read_and_unset_gestures_keep_their_default_actions() {
    let (s, problems) = DockSettings::parse(
        "pin = a.desktop\npinned = b.desktop\nc.desktop\ntile_size = 48\nlarge_size = 100\n\
         magnification = on\nautohide = no\nrecents = 1\nminimize_to_app = true\nbounce = off\n\
         indicators = yes\nstack = fan\nposition = left\n\
         corner_top_left = launcher\ncorner_top_right = exec: foot -e top\n\
         corner_bottom_left = app-grid\nswipe_up = none\nspread = lock\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(s.pinned, ["a.desktop", "b.desktop", "c.desktop"]);
    assert_eq!((s.tile_size, s.large_size), (Some(48.0), Some(100.0)));
    assert_eq!(
        [
            s.magnification,
            s.autohide,
            s.recents,
            s.minimize_to_app,
            s.bounce,
            s.indicators
        ],
        [
            Some(true),
            Some(false),
            Some(true),
            Some(true),
            Some(false),
            Some(true)
        ]
    );
    assert_eq!(s.stack, Some(StackStyle::Fan));
    assert_eq!(s.position, Some(Position::Left));
    let server = |name: &str| ActionTarget::Server(name.into());
    for (trigger, expected) in [
        (Trigger::CornerTopLeft, server("launcher")),
        (
            Trigger::CornerTopRight,
            ActionTarget::Exec("foot -e top".into()),
        ),
        (Trigger::CornerBottomLeft, ActionTarget::AppGrid),
        (Trigger::CornerBottomRight, ActionTarget::None),
        (Trigger::SwipeUp, ActionTarget::None),
        (Trigger::SwipeDown, server("app-overview")),
        (Trigger::Pinch, ActionTarget::AppGrid),
        (Trigger::Spread, server("lock")),
    ] {
        assert_eq!(s.actions.get(trigger), &expected, "{trigger:?}");
    }
    assert_eq!(
        DockSettings::default().actions.get(Trigger::SwipeUp),
        &server("overview"),
        "swipe up opens the overview when unset"
    );
}

#[test]
fn rejected_values_are_reported_with_the_expected_choices() {
    for (line, expected) in [
        ("position = top", "bottom, left or right"),
        ("stack = pile", "auto, fan or grid"),
        ("tile_size = big", "tile_size:"),
        ("swipe_up = ", "swipe_up:"),
        ("pinch = exec:", "pinch:"),
    ] {
        let (s, problems) = DockSettings::parse(&format!("{line}\n"), None);
        assert_eq!(problems.len(), 1, "{line}");
        assert!(
            problems[0].message.contains(expected),
            "{line}: {}",
            problems[0].message
        );
        assert_eq!(s, DockSettings::default(), "{line}");
    }
}

#[test]
fn actions_parse_to_their_targets_and_write_back() {
    for (text, expected) in [
        ("none", Some(ActionTarget::None)),
        ("app-grid", Some(ActionTarget::AppGrid)),
        (" exec:  ls -l ", Some(ActionTarget::Exec("ls -l".into()))),
        ("overview", Some(ActionTarget::Server("overview".into()))),
        ("", None),
        ("exec:   ", None),
    ] {
        let parsed = parse_action(text);
        assert_eq!(parsed, expected, "{text:?}");
        if let Some(a) = parsed {
            assert_eq!(
                parse_action(&action_text(&a)),
                Some(a),
                "{text:?} writes back"
            );
        }
    }
}

#[test]
fn sizes_outside_the_design_range_fall_back_to_the_display_default() {
    for (value, height, expected) in [
        (Some(48.2), 1000.0, 49.0),
        (Some(256.0), 1000.0, 256.0),
        (Some(257.0), 1000.0, DEFAULT_TILE_SIZE),
        (Some(0.0), 1000.0, DEFAULT_TILE_SIZE),
        (Some(-4.0), 1000.0, DEFAULT_TILE_SIZE),
        (None, 1000.0, DEFAULT_TILE_SIZE),
        (None, SMALL_DISPLAY_HEIGHT, DEFAULT_TILE_SIZE_SMALL_DISPLAY),
        (None, SMALL_DISPLAY_HEIGHT + 1.0, DEFAULT_TILE_SIZE),
    ] {
        assert_eq!(
            sanitize_tile_size(value, height),
            expected,
            "tile {value:?} at {height}"
        );
    }
    for (value, expected) in [
        (Some(200.4), 201.0),
        (Some(512.0), 512.0),
        (Some(513.0), DEFAULT_LARGE_SIZE),
        (Some(0.0), DEFAULT_LARGE_SIZE),
        (None, DEFAULT_LARGE_SIZE),
    ] {
        assert_eq!(sanitize_large_size(value), expected, "large {value:?}");
    }
}

#[test]
fn a_store_keeps_pins_and_writes_changed_keys_and_removes_unset_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dock.conf");
    std::fs::write(
        &path,
        "pin = a.desktop\nposition = left\ncorner_top_left = lock\nrecents = off\n",
    )
    .unwrap();
    let (mut s, _) = DockSettings::load(&path, None).unwrap();
    s.position = Some(Position::Right);
    s.autohide = Some(true);
    s.recents = None;
    s.actions
        .set(Trigger::Pinch, ActionTarget::Exec("foot".into()));
    assert!(s.store(&path).unwrap().is_empty());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("pin = a.desktop\nposition = right\ncorner_top_left = lock\n"),
        "{text}"
    );
    assert!(
        text.contains("autohide = on\n") && text.contains("pinch = exec:foot\n"),
        "{text}"
    );
    assert!(!text.contains("recents"), "{text}");
    let (back, problems) = DockSettings::load(&path, None).unwrap();
    assert!(problems.is_empty());
    assert_eq!(back, s);
}
