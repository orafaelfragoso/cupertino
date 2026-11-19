//! `compositor.json`: every key typed and checked on its own, unknown keys kept on a write.

use cupertino_defaults::compositor::*;

fn parse(text: &str) -> (CompositorSettings, Vec<JsonProblem>) {
    CompositorSettings::parse(text, None)
}

#[test]
fn keys_are_read_with_their_types_and_an_empty_file_sets_nothing() {
    for empty in ["", "{}"] {
        assert_eq!(
            parse(empty),
            (CompositorSettings::default(), vec![]),
            "{empty:?}"
        );
    }
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
fn a_rejected_value_is_a_problem_of_its_key_alone_and_leaves_it_unset() {
    for (json, key, expected) in [
        (r#"{"corner_radius": "round"}"#, "corner_radius", ""),
        (r#"{"show_menu_bar": 1}"#, "show_menu_bar", ""),
        (r#"{"appearance": "sepia"}"#, "appearance", ""),
        (r#"{"minimize_effect": "poof"}"#, "minimize_effect", ""),
        (r#"{"terminals": "foot"}"#, "terminals", ""),
        (
            r#"{"show_scroll_bars": "never"}"#,
            "show_scroll_bars",
            "never",
        ),
        (r#"{"output_depth": 12}"#, "output_depth", "8 or 10 bits"),
        (r#"{"output_depth": "10"}"#, "output_depth", "8 or 10 bits"),
        (r#"{"output_depth": 10.5}"#, "output_depth", "8 or 10 bits"),
        (
            r#"{"material_density": 1.5}"#,
            "material_density",
            "between 0 and 1",
        ),
        (
            r#"{"wallpaper": "rel.png"}"#,
            "wallpaper",
            "not an absolute path",
        ),
        (
            r#"{"wallpaper_folders": ["/ok", "rel"]}"#,
            "wallpaper_folders",
            "\"rel\"",
        ),
        (r#"{"screenshot": []}"#, "screenshot", "empty"),
        (
            r#"{"shell": [{"name": "a", "command": ["x"], "roles": ["boss"]}]}"#,
            "shell",
            "unknown role",
        ),
        (
            r#"{"shell": [{"name": "a", "command": []}]}"#,
            "shell",
            "empty command",
        ),
        (
            r#"{"shell": [{"command": ["x"]}]}"#,
            "shell",
            "needs a name",
        ),
        (r#"{"shell": [{"name": "a"}]}"#, "shell", "needs a command"),
        (
            r#"{"dev_folder": {"path": "rel", "components": []}}"#,
            "dev_folder",
            "not an absolute path",
        ),
        (
            r#"{"dev_folder": {"path": "/a/../b", "components": []}}"#,
            "dev_folder",
            "`..`",
        ),
        (
            r#"{"dev_folder": {"path": "/a", "components": ["x", "x"]}}"#,
            "dev_folder",
            "listed twice",
        ),
        (
            r#"{"dev_folder": {"path": "/a", "components": [""]}}"#,
            "dev_folder",
            "empty",
        ),
        (
            r#"{"dev_folder": {"path": "/a"}}"#,
            "dev_folder",
            "list of components",
        ),
        (r#"{"mouse_dpi": {"m": 49}}"#, "mouse_dpi", "50 to 32000"),
        (r#"{"mouse_dpi": {"m": 32001}}"#, "mouse_dpi", "50 to 32000"),
        (r#"{"mouse_dpi": {"m": 1.5}}"#, "mouse_dpi", "50 to 32000"),
        (
            r#"{"mouse_dpi": {"m": "fast"}}"#,
            "mouse_dpi",
            "50 to 32000",
        ),
        (r#"{"mouse_dpi": {"m": -3}}"#, "mouse_dpi", "50 to 32000"),
        (
            r#"{"fractional_scale_mode": "blurry"}"#,
            "fractional_scale_mode",
            "supersample or direct",
        ),
    ] {
        let with_good_key = format!("{{\"edge_tiling\": true, {}", &json[1..]);
        let (s, problems) = parse(&with_good_key);
        assert_eq!(problems.len(), 1, "{json}: {problems:?}");
        assert_eq!(problems[0].key, key, "{json}");
        assert!(
            problems[0].message.contains(expected),
            "{json}: {}",
            problems[0].message
        );
        assert_eq!(
            s,
            CompositorSettings {
                edge_tiling: Some(true),
                ..Default::default()
            },
            "{json}: only the good key applies"
        );
    }
}

#[test]
fn every_name_of_a_named_value_is_accepted() {
    for name in ScrollBarsName::ALL {
        let (s, p) = parse(&format!(r#"{{"show_scroll_bars": "{}"}}"#, name.name()));
        assert!(p.is_empty(), "{name:?}");
        assert_eq!(s.show_scroll_bars, Some(name));
    }
    assert_eq!(parse(r#"{"output_depth": 8}"#).0.output_depth, Some(8));
    assert_eq!(
        parse(r#"{"material_density": 1}"#).0.material_density,
        Some(1.0)
    );
}

#[test]
fn colours_are_hex_with_an_optional_alpha() {
    for (text, expected) in [
        ("#0a0b0c", Some([10, 11, 12, 255])),
        ("#0a0b0c0d", Some([10, 11, 12, 13])),
        ("0a0b0c", None),
        ("#0a0b", None),
        ("#gg0000", None),
        ("#0a0b0c0", None),
        ("#éé00", None),
    ] {
        match expected {
            Some(rgba) => {
                assert_eq!(parse_rgba(text), Ok(rgba), "{text}");
                assert_eq!(rgba_text(rgba), text, "{text}");
            }
            None => assert!(parse_rgba(text).unwrap_err().contains("#rrggbb"), "{text}"),
        }
    }
}

#[test]
fn a_shell_process_has_the_roles_the_file_lists_and_the_session_role_by_default() {
    let (s, p) = parse(
        r#"{"shell": [{"name": "bar", "command": ["cupertino-menubar"], "roles": ["menu_bar", "session"]},
                      {"name": "obs", "command": ["x", "y"]}]}"#,
    );
    assert!(p.is_empty(), "{p:?}");
    let shell = s.shell.unwrap();
    let procs = shell.processes.unwrap();
    assert_eq!(procs[0].name, "bar");
    assert_eq!(
        procs[0].roles,
        [ShellRoleName::MenuBar, ShellRoleName::Session]
    );
    assert_eq!(procs[1].roles, [ShellRoleName::Session]);
    assert_eq!(procs[1].command, ["x", "y"]);

    // Object form (`shell-host.md`): components, off, alone, processes.
    let (obj, p2) = parse(
        r#"{"shell": {"components": ["menubar", "dock"], "off": ["dock"], "alone": ["menubar"]}}"#,
    );
    assert!(p2.is_empty(), "{p2:?}");
    let sec = obj.shell.unwrap();
    assert_eq!(
        sec.components.as_deref(),
        Some(&["menubar".to_string(), "dock".to_string()][..])
    );
    assert_eq!(sec.off.as_deref(), Some(&["dock".to_string()][..]));
    assert_eq!(sec.alone.as_deref(), Some(&["menubar".to_string()][..]));
    assert!(sec.processes.is_none());
}

#[test]
fn only_the_shells_own_chrome_may_start_from_the_development_folder() {
    let (s, p) = parse(r#"{"dev_folder": {"path": "/dev/f", "components": ["dock", "bar"]}}"#);
    assert!(p.is_empty());
    let f = s.dev_folder.unwrap();
    assert_eq!((f.path.as_str(), f.components.len()), ("/dev/f", 2));
    let entry = |roles: Vec<ShellRoleName>| ShellEntry {
        name: "x".into(),
        command: vec!["x".into()],
        roles,
    };
    for (roles, allowed) in [
        (vec![ShellRoleName::Dock, ShellRoleName::Session], true),
        (vec![ShellRoleName::Overview, ShellRoleName::Windows], true),
        (vec![ShellRoleName::Dock, ShellRoleName::SessionLock], false),
        (vec![ShellRoleName::Clipboard], false),
        (vec![ShellRoleName::SessionLock], false),
    ] {
        assert_eq!(
            entry(roles.clone()).may_start_from_dev_folder(),
            allowed,
            "{roles:?}"
        );
    }
}

#[test]
fn the_resolution_of_each_mouse_is_stored_by_its_key_and_set_or_forgotten_one_at_a_time() {
    let key = mouse_dpi_key(0x046d, 0xc077, "USB Mouse");
    assert_eq!(key, "046d:c077:USB Mouse");
    let (s, p) = parse(&format!(r#"{{"mouse_dpi": {{"{key}": 1600}}}}"#));
    assert!(p.is_empty());
    assert_eq!(s.mouse_dpi.unwrap()[&key], 1600);

    let mut s = CompositorSettings::default();
    s.set_mouse_dpi("a", Some(800)).unwrap();
    s.set_mouse_dpi("b", Some(1600)).unwrap();
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
    for (json, outputs, expected) in [
        (r#""direct""#, &["DP-1"][..], FractionalScaleMode::Direct),
        (
            r#"{"DP-1": "direct", "*": "supersample"}"#,
            &["HDMI-1", "other"][..],
            FractionalScaleMode::Supersample,
        ),
        (
            r#"{"DP-1": "direct", "*": "supersample"}"#,
            &["HDMI-1", "DP-1"][..],
            FractionalScaleMode::Direct,
        ),
    ] {
        let (s, p) = parse(&format!(r#"{{"fractional_scale_mode": {json}}}"#));
        assert!(p.is_empty(), "{json}");
        assert_eq!(
            s.fractional_scale_mode.unwrap().of(outputs.iter().copied()),
            expected,
            "{json} {outputs:?}"
        );
    }
    assert_eq!(
        FractionalScaleModes::default().of(["x"]),
        FractionalScaleMode::Supersample
    );
}

#[test]
fn a_broken_file_or_a_wrong_key_keeps_the_last_good_settings() {
    let (good, _) = parse(r#"{"corner_radius": 9, "edge_tiling": true}"#);
    for (text, expected) in [("{ not json", ""), ("[1]", "JSON object")] {
        let (s, p) = CompositorSettings::parse(text, Some(&good));
        assert_eq!(p.len(), 1, "{text}");
        assert_eq!(p[0].key, "", "{text}");
        assert!(p[0].message.contains(expected), "{text}");
        assert_eq!(s, good, "{text}");
    }
    assert_eq!(
        CompositorSettings::parse("{ not json", None).0,
        CompositorSettings::default()
    );
    let (s, p) = CompositorSettings::parse(
        r#"{"corner_radius": "x", "edge_tiling": false}"#,
        Some(&good),
    );
    assert_eq!(p.len(), 1);
    assert_eq!(
        s.corner_radius,
        Some(9.0),
        "the wrong key keeps its last good value"
    );
    assert_eq!(s.edge_tiling, Some(false), "the others apply");
}

#[test]
fn a_store_keeps_unknown_keys_writes_only_what_changed_and_reads_back() {
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
    s.fractional_scale_mode =
        parse(r#"{"fractional_scale_mode": {"*": "direct", "DP-1": "supersample"}}"#)
            .0
            .fractional_scale_mode;
    assert!(s.store(&path).unwrap().is_empty());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["zzz_future"]["a"][1], 2);
    assert_eq!(v["bogus_value"], 1);
    assert_eq!(v["edge_tiling"], true);
    assert!(v.get("corner_radius").is_none());
    assert_eq!(CompositorSettings::load(&path, None).unwrap(), (s, vec![]));

    let fresh = dir.path().join("new/compositor.json");
    let s = parse(r#"{"fractional_scale_mode": "direct", "show_menu_bar": false}"#).0;
    s.store(&fresh).unwrap();
    assert!(std::fs::read_to_string(&fresh)
        .unwrap()
        .contains("\"fractional_scale_mode\": \"direct\""));
    assert_eq!(
        CompositorSettings::load(&fresh, None).unwrap(),
        (s, vec![]),
        "a missing file is created"
    );
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
