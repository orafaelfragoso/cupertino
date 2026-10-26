//! The panes' writes keep what they do not change (ST-15, ST-16): comments, pins, corner and gesture lines and unknown
//! keys of `dock.conf`; key bindings, the shell list and unknown keys of `compositor.json`; and a write that fails leaves
//! the file as it was.

use cupertino_settings::controls::Value;
use cupertino_settings::live::Live;
use cupertino_settings::panes::{change, ControlId, Effect};
use cupertino_settings::settings::{File, Paths, Settings, StoreError};

fn paths(dir: &std::path::Path) -> Paths {
    Paths {
        dock: Some(dir.join("dock.conf")),
        compositor: Some(dir.join("compositor.json")),
        menubar: Some(dir.join("menubar.conf")),
        keyboard: Some(dir.join("keyboard.conf")),
        power: Some(dir.join("power.conf")),
        accessibility: Some(dir.join("accessibility.conf")),
    }
}

/// Commit `value` of `id` and write what it changed.
fn commit(settings: &mut Settings, paths: &Paths, id: ControlId, value: Value) {
    for effect in change(id, value, settings, &Live::default()) {
        match effect {
            Effect::Store(file) => settings.store(paths, file).unwrap(),
            other => panic!("no session or service call in these tests: {other:?}"),
        }
    }
}

#[test]
fn a_dock_write_keeps_pins_comments_corners_and_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    std::fs::write(dir.path().join("dock.conf"), "# my dock\nfoot\npin = firefox\ncorner_top_left = lock\nswipe_up = none\nfuture_key = 7\ntile_size = 48\n").unwrap();
    let mut s = Settings::load(&p, 1080.0).unwrap();
    commit(&mut s, &p, ControlId::DockSize, Value::Fraction(0.5));
    commit(&mut s, &p, ControlId::AutoHide, Value::Bool(true));
    let text = std::fs::read_to_string(dir.path().join("dock.conf")).unwrap();
    for kept in ["# my dock", "foot", "pin = firefox", "corner_top_left = lock", "swipe_up = none", "future_key = 7"] {
        assert!(text.contains(kept), "{kept:?} kept in:\n{text}");
    }
    assert!(text.contains("tile_size = 72") && !text.contains("tile_size = 48"), "{text}");
    assert!(text.contains("autohide = on"), "{text}");
    assert!(!dir.path().join("dock.conf.tmp").exists(), "the temporary file is renamed away");
    assert_eq!(Settings::load(&p, 1080.0).unwrap().dock, s.dock, "what was written reads back");
}

#[test]
fn a_commit_rereads_the_file_so_another_writers_change_survives() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    std::fs::write(dir.path().join("dock.conf"), "autohide = off\ntile_size = 48\n").unwrap();
    let mut s = Settings::load(&p, 1080.0).unwrap();
    // the dock's menu turns auto-hide on after the window read the file
    std::fs::write(dir.path().join("dock.conf"), "autohide = on\ntile_size = 48\n").unwrap();
    // the window's commit: read again, change the one key, store
    s.reload(&p).unwrap();
    commit(&mut s, &p, ControlId::DockSize, Value::Fraction(0.5));
    let text = std::fs::read_to_string(dir.path().join("dock.conf")).unwrap();
    assert!(text.contains("autohide = on") && text.contains("tile_size = 72"), "{text}");
}

#[test]
fn a_compositor_write_keeps_bindings_the_shell_and_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let before = r#"{"keybindings": {"Super+t": "spawn-terminal"}, "shell": [{"name": "dock", "command": ["cupertino-dock"]}], "future": {"a": 1}, "mouse_speed": 0.5}"#;
    std::fs::write(dir.path().join("compositor.json"), before).unwrap();
    let mut s = Settings::load(&p, 1080.0).unwrap();
    commit(&mut s, &p, ControlId::MouseSpeed, Value::Index(9));
    let after: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("compositor.json")).unwrap()).unwrap();
    assert_eq!(after["mouse_speed"], 3.0);
    assert_eq!(after["keybindings"]["Super+t"], "spawn-terminal");
    assert_eq!(after["shell"][0]["command"][0], "cupertino-dock");
    assert_eq!(after["future"]["a"], 1);
}

#[test]
fn a_write_that_fails_leaves_the_file_and_names_the_problem() {
    let dir = tempfile::tempdir().unwrap();
    // the menu bar's file lies in a directory that is a regular file: neither the temporary file nor the file can be made
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "").unwrap();
    let p = Paths { menubar: Some(blocker.join("menubar.conf")), ..paths(dir.path()) };
    let mut s = Settings::load(&Paths { menubar: None, ..p.clone() }, 1080.0).unwrap();
    change(ControlId::ClockSeconds, Value::Bool(true), &mut s, &Live::default());
    let error = s.store(&p, File::MenuBar).unwrap_err();
    assert!(matches!(error, StoreError::Io(File::MenuBar, _)), "{error:?}");
    assert!(error.to_string().starts_with("The change could not be saved"), "{error}");
    assert!(matches!(s.store(&Paths::default(), File::Dock), Err(StoreError::NoPath(File::Dock))));
}
