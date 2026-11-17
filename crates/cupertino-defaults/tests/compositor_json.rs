//! `compositor.json`: every key typed and checked on its own, unknown keys kept on a write.

use cupertino_defaults::compositor::*;

fn parse(text: &str) -> (CompositorSettings, Vec<JsonProblem>) {
    CompositorSettings::parse(text, None)
}

fn only_problem(problems: &[JsonProblem], key: &str) -> String {
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].key, key);
    problems[0].message.clone()
}

#[test]
fn an_empty_or_missing_file_sets_nothing() {
    assert_eq!(parse(""), (CompositorSettings::default(), vec![]));
    assert_eq!(parse("{}"), (CompositorSettings::default(), vec![]));
    let dir = tempfile::tempdir().unwrap();
    let (s, problems) = CompositorSettings::load(&dir.path().join("absent.json"), None).unwrap();
    assert_eq!(s, CompositorSettings::default());
    assert!(problems.is_empty());
}

#[test]
fn keys_are_read_with_their_types() {
    let (s, problems) = parse(
        r##"{
        "appearance": "dark-high-contrast", "terminals": ["foot", "$TERMINAL"],
        "keybindings": {"Super+T": "terminal"}, "corner_radius": 12.5, "menu_bar_height": 24,
        "show_menu_bar": false, "menu_bar_auto_hide": true, "edge_tiling": true, "tile_margins": false,
        "wallpaper_color": "#102030", "material_density": 0.25, "output_depth": 10,
        "prepare_toolkit_programs": true, "decorations": false, "minimize_effect": "none",
        "show_scroll_bars": "when-scrolling", "closing_animation": false, "hot_corner_size": 4,
        "hot_corner_dwell": 0.5, "mouse_speed": 0.3, "trackpad_speed": 0.7,
        "mouse_acceleration": false, "trackpad_acceleration": true, "natural_scrolling": true,
        "mouse_natural_scrolling": false, "cursor_size": 1.5, "cursor_fill": "#ffffff80",
        "cursor_outline": "#000000", "shake_to_locate": false, "wallpaper": "/w/a.png",
        "wallpaper_dark": "/w/b.png", "wallpaper_folders": ["/w", "/v"],
        "screenshot": ["grim", "-"]
        }"##,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(s.appearance, Some(AppearanceName::DarkHighContrast));
    assert_eq!(s.terminals.unwrap(), ["foot", "$TERMINAL"]);
    assert_eq!(s.keybindings.unwrap()["Super+T"], "terminal");
    assert_eq!(s.corner_radius, Some(12.5));
    assert_eq!(s.menu_bar_height, Some(24.0));
    assert_eq!(s.show_menu_bar, Some(false));
    assert_eq!(s.menu_bar_auto_hide, Some(true));
    assert_eq!(s.edge_tiling, Some(true));
    assert_eq!(s.tile_margins, Some(false));
    assert_eq!(s.wallpaper_color, Some([0x10, 0x20, 0x30, 255]));
    assert_eq!(s.material_density, Some(0.25));
    assert_eq!(s.output_depth, Some(10));
    assert_eq!(s.prepare_toolkit_programs, Some(true));
    assert_eq!(s.decorations, Some(false));
    assert_eq!(s.minimize_effect, Some(MinimizeEffectName::Plain));
    assert_eq!(s.show_scroll_bars, Some(ScrollBarsName::WhenScrolling));
    assert_eq!(s.closing_animation, Some(false));
    assert_eq!(s.hot_corner_size, Some(4.0));
    assert_eq!(s.hot_corner_dwell, Some(0.5));
    assert_eq!(s.mouse_speed, Some(0.3));
    assert_eq!(s.trackpad_speed, Some(0.7));
    assert_eq!(s.mouse_acceleration, Some(false));
    assert_eq!(s.trackpad_acceleration, Some(true));
    assert_eq!(s.natural_scrolling, Some(true));
    assert_eq!(s.mouse_natural_scrolling, Some(false));
    assert_eq!(s.cursor_size, Some(1.5));
    assert_eq!(s.cursor_fill, Some([255, 255, 255, 0x80]));
    assert_eq!(s.cursor_outline, Some([0, 0, 0, 255]));
    assert_eq!(s.shake_to_locate, Some(false));
    assert_eq!(s.wallpaper.as_deref(), Some("/w/a.png"));
    assert_eq!(s.wallpaper_dark.as_deref(), Some("/w/b.png"));
    assert_eq!(s.wallpaper_folders.unwrap(), ["/w", "/v"]);
    assert_eq!(s.screenshot.unwrap(), ["grim", "-"]);
}

#[test]
fn each_bad_key_is_a_problem_and_the_others_apply() {
    let (s, problems) = parse(
        r#"{"corner_radius": "round", "show_menu_bar": 1, "edge_tiling": true,
            "appearance": "sepia", "minimize_effect": "poof", "terminals": "foot"}"#,
    );
    let mut keys: Vec<_> = problems.iter().map(|p| p.key.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "appearance",
            "corner_radius",
            "minimize_effect",
            "show_menu_bar",
            "terminals"
        ]
    );
    assert_eq!(s.edge_tiling, Some(true));
    assert_eq!(s.corner_radius, None);
}

#[test]
fn show_scroll_bars_takes_its_three_names_and_refuses_others() {
    for name in ScrollBarsName::ALL {
        let (s, p) = parse(&format!(r#"{{"show_scroll_bars": "{}"}}"#, name.name()));
        assert!(p.is_empty());
        assert_eq!(s.show_scroll_bars, Some(name));
    }
    assert_eq!(
        ScrollBarsName::ALL.map(|n| n.name()),
        ["automatic", "when-scrolling", "always"]
    );
    let (_, p) = parse(r#"{"show_scroll_bars": "never"}"#);
    assert!(only_problem(&p, "show_scroll_bars").contains("never"));
}

#[test]
fn the_output_depth_is_8_or_10_bits() {
    assert_eq!(parse(r#"{"output_depth": 8}"#).0.output_depth, Some(8));
    for bad in ["12", "\"10\"", "10.5"] {
        let (s, p) = parse(&format!(r#"{{"output_depth": {bad}}}"#));
        assert!(only_problem(&p, "output_depth").contains("8 or 10 bits"));
        assert_eq!(s.output_depth, None);
    }
}

#[test]
fn the_material_density_is_between_zero_and_one() {
    assert_eq!(
        parse(r#"{"material_density": 1}"#).0.material_density,
        Some(1.0)
    );
    let (_, p) = parse(r#"{"material_density": 1.5}"#);
    assert!(only_problem(&p, "material_density").contains("between 0 and 1"));
}

#[test]
fn colours_are_hex_with_an_optional_alpha() {
    assert_eq!(parse_rgba("#0a0b0c"), Ok([10, 11, 12, 255]));
    assert_eq!(parse_rgba("#0a0b0c0d"), Ok([10, 11, 12, 13]));
    for bad in ["0a0b0c", "#0a0b", "#gg0000", "#0a0b0c0", "#éé00"] {
        assert!(parse_rgba(bad).unwrap_err().contains("#rrggbb"), "{bad}");
    }
    assert_eq!(rgba_text([1, 2, 3, 255]), "#010203");
    assert_eq!(rgba_text([1, 2, 3, 4]), "#01020304");
}

#[test]
fn the_wallpapers_and_their_folders_are_absolute_paths() {
    let (_, p) = parse(r#"{"wallpaper": "rel.png"}"#);
    assert!(only_problem(&p, "wallpaper").contains("not an absolute path"));
    let (_, p) = parse(r#"{"wallpaper_folders": ["/ok", "rel"]}"#);
    assert!(only_problem(&p, "wallpaper_folders").contains("\"rel\""));
}

#[test]
fn a_shell_process_has_the_roles_the_file_lists_and_the_session_role_by_default() {
    let (s, p) = parse(
        r#"{"shell": [{"name": "bar", "command": ["cupertino-menubar"], "roles": ["menu_bar", "session"]},
                      {"name": "obs", "command": ["x", "y"]}]}"#,
    );
    assert!(p.is_empty(), "{p:?}");
    let shell = s.shell.unwrap();
    assert_eq!(shell[0].name, "bar");
    assert_eq!(
        shell[0].roles,
        [ShellRoleName::MenuBar, ShellRoleName::Session]
    );
    assert_eq!(shell[1].roles, [ShellRoleName::Session]);
    assert_eq!(shell[1].command, ["x", "y"]);
}

#[test]
fn an_unknown_shell_role_an_empty_command_or_a_missing_name_is_a_problem() {
    for (text, expect) in [
        (
            r#"{"shell": [{"name": "a", "command": ["x"], "roles": ["boss"]}]}"#,
            "unknown role",
        ),
        (
            r#"{"shell": [{"name": "a", "command": []}]}"#,
            "empty command",
        ),
        (r#"{"shell": [{"command": ["x"]}]}"#, "needs a name"),
        (r#"{"shell": [{"name": "a"}]}"#, "needs a command"),
    ] {
        let (s, p) = parse(text);
        assert!(only_problem(&p, "shell").contains(expect), "{text}");
        assert_eq!(s.shell, None);
    }
}

#[test]
fn only_the_shells_own_chrome_may_start_from_the_development_folder() {
    let allowed: Vec<_> = ShellRoleName::ALL
        .iter()
        .filter(|r| r.allows_dev_folder())
        .collect();
    assert_eq!(allowed.len(), 4);
    assert!(!ShellRoleName::SessionLock.allows_dev_folder());
    assert!(!ShellRoleName::Clipboard.allows_dev_folder());
    let entry = |roles: Vec<ShellRoleName>| ShellEntry {
        name: "x".into(),
        command: vec!["x".into()],
        roles,
    };
    assert!(entry(vec![ShellRoleName::Dock, ShellRoleName::Session]).may_start_from_dev_folder());
    assert!(
        !entry(vec![ShellRoleName::Dock, ShellRoleName::SessionLock]).may_start_from_dev_folder()
    );
    for role in ShellRoleName::ALL {
        assert_eq!(ShellRoleName::from_name(role.name()), Some(role));
    }
}

#[test]
fn the_development_folder_is_an_absolute_path_without_dots_and_distinct_names() {
    let (s, p) = parse(r#"{"dev_folder": {"path": "/dev/f", "components": ["dock", "bar"]}}"#);
    assert!(p.is_empty());
    let f = s.dev_folder.unwrap();
    assert_eq!((f.path.as_str(), f.components.len()), ("/dev/f", 2));
    for (text, expect) in [
        (
            r#"{"path": "rel", "components": []}"#,
            "not an absolute path",
        ),
        (r#"{"path": "/a/../b", "components": []}"#, "`..`"),
        (
            r#"{"path": "/a", "components": ["x", "x"]}"#,
            "listed twice",
        ),
        (r#"{"path": "/a", "components": [""]}"#, "empty"),
        (r#"{"path": "/a"}"#, "list of components"),
    ] {
        let (_, p) = parse(&format!(r#"{{"dev_folder": {text}}}"#));
        assert!(only_problem(&p, "dev_folder").contains(expect), "{text}");
    }
}

#[test]
fn the_screenshot_command_is_not_empty() {
    let (_, p) = parse(r#"{"screenshot": []}"#);
    assert!(only_problem(&p, "screenshot").contains("empty"));
}

#[test]
fn the_resolution_of_each_mouse_is_stored_by_its_key() {
    let key = mouse_dpi_key(0x046d, 0xc077, "USB Mouse");
    assert_eq!(key, "046d:c077:USB Mouse");
    let (s, p) = parse(&format!(r#"{{"mouse_dpi": {{"{key}": 1600}}}}"#));
    assert!(p.is_empty());
    assert_eq!(s.mouse_dpi.unwrap()[&key], 1600);
    for bad in ["49", "32001", "1.5", "\"fast\"", "-3"] {
        let (_, p) = parse(&format!(r#"{{"mouse_dpi": {{"m": {bad}}}}}"#));
        assert!(
            only_problem(&p, "mouse_dpi").contains("50 to 32000"),
            "{bad}"
        );
    }
}

#[test]
fn one_mouse_is_set_or_forgotten_and_a_resolution_outside_the_range_is_refused() {
    let mut s = CompositorSettings::default();
    s.set_mouse_dpi("a", Some(800)).unwrap();
    s.set_mouse_dpi("b", Some(1600)).unwrap();
    assert_eq!(s.mouse_dpi.as_ref().unwrap().len(), 2);
    assert!(s
        .set_mouse_dpi("a", Some(49))
        .unwrap_err()
        .contains("not from 50 to 32000"));
    assert_eq!(
        s.mouse_dpi.as_ref().unwrap()["a"],
        800,
        "unchanged by a refusal"
    );
    s.set_mouse_dpi("a", None).unwrap();
    assert_eq!(s.mouse_dpi.as_ref().unwrap().len(), 1);
    s.set_mouse_dpi("b", None).unwrap();
    assert_eq!(s.mouse_dpi, None, "with no mouse left the key is unset");
}

#[test]
fn the_fractional_scale_mode_is_one_for_every_display_or_one_a_display() {
    let (s, p) = parse(r#"{"fractional_scale_mode": "direct"}"#);
    assert!(p.is_empty());
    let modes = s.fractional_scale_mode.unwrap();
    assert_eq!(modes.of(["DP-1"]), FractionalScaleMode::Direct);

    let (s, p) = parse(r#"{"fractional_scale_mode": {"DP-1": "direct", "*": "supersample"}}"#);
    assert!(p.is_empty());
    let modes = s.fractional_scale_mode.unwrap();
    assert_eq!(
        modes.of(["HDMI-1", "other"]),
        FractionalScaleMode::Supersample
    );
    assert_eq!(modes.of(["HDMI-1", "DP-1"]), FractionalScaleMode::Direct);
    assert_eq!(
        FractionalScaleModes::default().of(["x"]),
        FractionalScaleMode::Supersample
    );

    let (_, p) = parse(r#"{"fractional_scale_mode": "blurry"}"#);
    assert!(only_problem(&p, "fractional_scale_mode").contains("supersample or direct"));
}

#[test]
fn the_fractional_scale_mode_is_written_as_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compositor.json");
    for text in [
        r#"{"fractional_scale_mode": "direct"}"#,
        r#"{"fractional_scale_mode": {"*": "direct", "DP-1": "supersample"}}"#,
    ] {
        std::fs::write(&path, "{}").unwrap();
        let (s, _) = parse(text);
        s.store(&path).unwrap();
        let (back, p) = CompositorSettings::load(&path, None).unwrap();
        assert!(p.is_empty());
        assert_eq!(back, s);
    }
    std::fs::write(&path, "{}").unwrap();
    parse(r#"{"fractional_scale_mode": "direct"}"#)
        .0
        .store(&path)
        .unwrap();
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("\"fractional_scale_mode\": \"direct\""));
}

#[test]
fn a_broken_file_keeps_the_last_good_settings() {
    let (good, _) = parse(r#"{"corner_radius": 9, "edge_tiling": true}"#);
    let (s, p) = CompositorSettings::parse("{ not json", Some(&good));
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].key, "");
    assert_eq!(s, good);
    let (s, p) = CompositorSettings::parse("[1]", Some(&good));
    assert!(p[0].message.contains("JSON object"));
    assert_eq!(s, good);
    let (s, p) = CompositorSettings::parse("{ not json", None);
    assert_eq!((s, p.len()), (CompositorSettings::default(), 1));
}

#[test]
fn a_wrong_key_keeps_its_last_good_value_only() {
    let (good, _) = parse(r#"{"corner_radius": 9, "edge_tiling": true}"#);
    let (s, p) = CompositorSettings::parse(
        r#"{"corner_radius": "x", "edge_tiling": false}"#,
        Some(&good),
    );
    assert_eq!(p.len(), 1);
    assert_eq!(s.corner_radius, Some(9.0));
    assert_eq!(s.edge_tiling, Some(false));
}

#[test]
fn a_store_keeps_unknown_keys_and_writes_only_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compositor.json");
    std::fs::write(
        &path,
        r#"{"zzz_future": {"a": [1, 2]}, "corner_radius": 4, "bogus_value": 1}"#,
    )
    .unwrap();
    let (mut s, p) = CompositorSettings::load(&path, None).unwrap();
    assert!(p.is_empty(), "unknown keys are not problems");
    s.edge_tiling = Some(true);
    s.corner_radius = None;
    assert!(s.store(&path).unwrap().is_empty());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["zzz_future"]["a"][1], 2);
    assert_eq!(v["bogus_value"], 1);
    assert_eq!(v["edge_tiling"], true);
    assert!(v.get("corner_radius").is_none());
}

#[test]
fn a_store_creates_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new/compositor.json");
    let s = CompositorSettings {
        show_menu_bar: Some(false),
        ..Default::default()
    };
    s.store(&path).unwrap();
    assert_eq!(CompositorSettings::load(&path, None).unwrap().0, s);
}

#[test]
fn a_store_refuses_to_overwrite_a_file_that_is_not_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compositor.json");
    std::fs::write(&path, "hand written, not json").unwrap();
    let err = CompositorSettings::default().store(&path).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "hand written, not json"
    );
}

#[test]
fn the_environment_variable_redirects_the_file() {
    std::env::set_var(COMPOSITOR_CONF_ENV, "/x/compositor.json");
    assert_eq!(
        CompositorSettings::path(),
        Some("/x/compositor.json".into())
    );
}

#[test]
fn the_login_screens_own_file_is_under_etc() {
    assert_eq!(GREETER_CONF_PATH, "/etc/cupertino/greeter.json");
}
