//! `dock.conf`: pins, options, the design's size validation and the corner and gesture actions.

use cupertino_defaults::dock::*;

fn temp_conf(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dock.conf");
    std::fs::write(&path, text).unwrap();
    (dir, path)
}

#[test]
fn a_missing_file_sets_nothing_and_binds_the_default_actions() {
    let dir = tempfile::tempdir().unwrap();
    let (s, problems) = DockSettings::load(&dir.path().join("dock.conf"), None).unwrap();
    assert!(problems.is_empty());
    assert_eq!(s, DockSettings::default());
    assert!(s.pinned.is_empty());
    assert_eq!(s.tile_size, None);
    for corner in &Trigger::ALL[..4] {
        assert_eq!(s.actions.get(*corner), &ActionTarget::None);
    }
    assert_eq!(
        s.actions.get(Trigger::SwipeUp),
        &ActionTarget::Server("overview".into())
    );
    assert_eq!(
        s.actions.get(Trigger::SwipeDown),
        &ActionTarget::Server("app-overview".into())
    );
    assert_eq!(s.actions.get(Trigger::Pinch), &ActionTarget::AppGrid);
    assert_eq!(
        s.actions.get(Trigger::Spread),
        &ActionTarget::Server("show-desktop".into())
    );
}

#[test]
fn options_and_pins_are_read() {
    let (s, problems) = DockSettings::parse(
        "pin = a.desktop\npinned = b.desktop\nc.desktop\ntile_size = 48\nlarge_size = 100\n\
         magnification = on\nautohide = no\nrecents = 1\nminimize_to_app = true\nbounce = off\n\
         indicators = yes\nstack = fan\nposition = left\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(s.pinned, ["a.desktop", "b.desktop", "c.desktop"]);
    assert_eq!(s.tile_size, Some(48.0));
    assert_eq!(s.large_size, Some(100.0));
    assert_eq!(s.magnification, Some(true));
    assert_eq!(s.autohide, Some(false));
    assert_eq!(s.recents, Some(true));
    assert_eq!(s.minimize_to_app, Some(true));
    assert_eq!(s.bounce, Some(false));
    assert_eq!(s.indicators, Some(true));
    assert_eq!(s.stack, Some(StackStyle::Fan));
    assert_eq!(s.position, Some(Position::Left));
}

#[test]
fn bad_values_are_problems_with_their_line_and_the_other_lines_apply() {
    let (s, problems) = DockSettings::parse(
        "position = top\ntile_size = big\nstack = pile\nmagnification = on\nswipe_up = \n",
        None,
    );
    let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
    assert_eq!(lines, [1, 2, 3, 5]);
    assert!(problems[0].message.contains("bottom, left or right"));
    assert!(problems[2].message.contains("auto, fan or grid"));
    assert_eq!(s.magnification, Some(true));
    assert_eq!(s.position, None);
}

#[test]
fn corner_and_gesture_keys_take_actions() {
    let (s, problems) = DockSettings::parse(
        "corner_top_left = launcher\ncorner_top_right = exec: foot -e top\n\
         corner_bottom_left = app-grid\ncorner_bottom_right = none\nswipe_up = none\nspread = lock\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        s.actions.get(Trigger::CornerTopLeft),
        &ActionTarget::Server("launcher".into())
    );
    assert_eq!(
        s.actions.get(Trigger::CornerTopRight),
        &ActionTarget::Exec("foot -e top".into())
    );
    assert_eq!(
        s.actions.get(Trigger::CornerBottomLeft),
        &ActionTarget::AppGrid
    );
    assert_eq!(s.actions.get(Trigger::SwipeUp), &ActionTarget::None);
    assert_eq!(
        s.actions.get(Trigger::Spread),
        &ActionTarget::Server("lock".into())
    );
}

#[test]
fn an_exec_action_without_a_program_is_a_problem() {
    let (_, problems) = DockSettings::parse("pinch = exec:\n", None);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].message.starts_with("pinch:"));
}

#[test]
fn actions_parse_to_their_targets_and_write_back() {
    assert_eq!(parse_action("none"), Some(ActionTarget::None));
    assert_eq!(parse_action("app-grid"), Some(ActionTarget::AppGrid));
    assert_eq!(
        parse_action(" exec:  ls -l "),
        Some(ActionTarget::Exec("ls -l".into()))
    );
    assert_eq!(
        parse_action("overview"),
        Some(ActionTarget::Server("overview".into()))
    );
    assert_eq!(parse_action(""), None);
    assert_eq!(parse_action("exec:   "), None);
    for a in [
        ActionTarget::None,
        ActionTarget::AppGrid,
        ActionTarget::Exec("ls -l".into()),
        ActionTarget::Server("lock".into()),
    ] {
        assert_eq!(parse_action(&action_text(&a)), Some(a));
    }
}

#[test]
fn trigger_keys_name_their_triggers() {
    for (key, trigger) in TRIGGER_KEYS.iter().zip(Trigger::ALL) {
        assert_eq!(Trigger::from_key(key), Some(trigger));
    }
    assert_eq!(Trigger::from_key("swipe_left"), None);
}

#[test]
fn the_tile_size_is_valid_between_zero_and_257_rounded_up_else_the_display_default() {
    assert_eq!(sanitize_tile_size(Some(48.2), 1000.0), 49.0);
    assert_eq!(sanitize_tile_size(Some(256.0), 1000.0), 256.0);
    assert_eq!(sanitize_tile_size(Some(257.0), 1000.0), DEFAULT_TILE_SIZE);
    assert_eq!(sanitize_tile_size(Some(0.0), 1000.0), DEFAULT_TILE_SIZE);
    assert_eq!(sanitize_tile_size(Some(-4.0), 1000.0), DEFAULT_TILE_SIZE);
    assert_eq!(sanitize_tile_size(None, 1000.0), DEFAULT_TILE_SIZE);
}

#[test]
fn a_small_display_has_a_smaller_default_tile() {
    assert_eq!(
        default_tile_size(SMALL_DISPLAY_HEIGHT),
        DEFAULT_TILE_SIZE_SMALL_DISPLAY
    );
    assert_eq!(
        default_tile_size(SMALL_DISPLAY_HEIGHT + 1.0),
        DEFAULT_TILE_SIZE
    );
    assert_eq!(
        sanitize_tile_size(None, 600.0),
        DEFAULT_TILE_SIZE_SMALL_DISPLAY
    );
}

#[test]
fn the_large_size_is_valid_between_zero_and_513_rounded_up_else_128() {
    assert_eq!(sanitize_large_size(Some(200.4)), 201.0);
    assert_eq!(sanitize_large_size(Some(512.0)), 512.0);
    assert_eq!(sanitize_large_size(Some(513.0)), DEFAULT_LARGE_SIZE);
    assert_eq!(sanitize_large_size(Some(0.0)), DEFAULT_LARGE_SIZE);
    assert_eq!(sanitize_large_size(None), DEFAULT_LARGE_SIZE);
}

#[test]
fn a_store_keeps_pins_corner_lines_and_comments_and_writes_the_changed_keys() {
    let (_dir, path) =
        temp_conf("# my dock\npin = a.desktop\nposition = left  # side\ncorner_top_left = lock\n");
    let (mut s, _) = DockSettings::load(&path, None).unwrap();
    s.position = Some(Position::Right);
    s.autohide = Some(true);
    s.actions
        .set(Trigger::Pinch, ActionTarget::Exec("foot".into()));
    assert!(s.store(&path).unwrap().is_empty());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with(
            "# my dock\npin = a.desktop\nposition = right  # side\ncorner_top_left = lock\n"
        ),
        "{text}"
    );
    assert!(text.contains("autohide = on\n"));
    assert!(text.contains("pinch = exec:foot\n"));
    let (back, problems) = DockSettings::load(&path, None).unwrap();
    assert!(problems.is_empty());
    assert_eq!(back.pinned, ["a.desktop"]);
    assert_eq!(back, s);
}

#[test]
fn a_key_set_back_to_unset_is_removed_from_the_file() {
    let (_dir, path) = temp_conf("autohide = on\nrecents = off\n");
    let (mut s, _) = DockSettings::load(&path, None).unwrap();
    s.autohide = None;
    s.store(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "recents = off\n");
}

#[test]
fn a_bad_value_keeps_the_last_good_one_while_the_file_is_edited() {
    let (good, _) = DockSettings::parse("position = left\nautohide = on\n", None);
    let (s, problems) = DockSettings::parse("position = lefft\nautohide = off\n", Some(&good));
    assert_eq!(problems.len(), 1);
    assert_eq!(s.position, Some(Position::Left));
    assert_eq!(s.autohide, Some(false));
}

#[test]
fn the_environment_variable_redirects_the_file() {
    std::env::set_var(DOCK_CONF_ENV, "/somewhere/dock.conf");
    assert_eq!(DockSettings::path(), Some("/somewhere/dock.conf".into()));
}
