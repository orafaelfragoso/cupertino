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
        system_accessibility: None,
        data_control: Some(dir.join("data-control.conf")),
        screen_sharing: None,
        desktop_control: None,
        notifications: Some(dir.join("notifications.conf")),
        screen_reader: None,
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
    for kept in [
        "# my dock",
        "foot",
        "pin = firefox",
        "corner_top_left = lock",
        "swipe_up = none",
        "future_key = 7",
    ] {
        assert!(text.contains(kept), "{kept:?} kept in:\n{text}");
    }
    assert!(
        text.contains("tile_size = 72") && !text.contains("tile_size = 48"),
        "{text}"
    );
    assert!(text.contains("autohide = on"), "{text}");
    assert!(
        !dir.path().join("dock.conf.tmp").exists(),
        "the temporary file is renamed away"
    );
    assert_eq!(
        Settings::load(&p, 1080.0).unwrap().dock,
        s.dock,
        "what was written reads back"
    );
}

#[test]
fn a_commit_rereads_the_file_so_another_writers_change_survives() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    std::fs::write(
        dir.path().join("dock.conf"),
        "autohide = off\ntile_size = 48\n",
    )
    .unwrap();
    let mut s = Settings::load(&p, 1080.0).unwrap();
    // the dock's menu turns auto-hide on after the window read the file
    std::fs::write(
        dir.path().join("dock.conf"),
        "autohide = on\ntile_size = 48\n",
    )
    .unwrap();
    // the window's commit: read again, change the one key, store
    s.reload(&p).unwrap();
    commit(&mut s, &p, ControlId::DockSize, Value::Fraction(0.5));
    let text = std::fs::read_to_string(dir.path().join("dock.conf")).unwrap();
    assert!(
        text.contains("autohide = on") && text.contains("tile_size = 72"),
        "{text}"
    );
}

#[test]
fn a_compositor_write_keeps_bindings_the_shell_and_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let before = r#"{"keybindings": {"Super+t": "spawn-terminal"}, "shell": [{"name": "dock", "command": ["cupertino-dock"]}], "future": {"a": 1}, "mouse_speed": 0.5}"#;
    std::fs::write(dir.path().join("compositor.json"), before).unwrap();
    let mut s = Settings::load(&p, 1080.0).unwrap();
    commit(&mut s, &p, ControlId::MouseSpeed, Value::Index(9));
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("compositor.json")).unwrap())
            .unwrap();
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
    let p = Paths {
        menubar: Some(blocker.join("menubar.conf")),
        ..paths(dir.path())
    };
    let mut s = Settings::load(
        &Paths {
            menubar: None,
            ..p.clone()
        },
        1080.0,
    )
    .unwrap();
    change(
        ControlId::ClockSeconds,
        Value::Bool(true),
        &mut s,
        &Live::default(),
    );
    let error = s.store(&p, File::MenuBar).unwrap_err();
    assert!(
        matches!(error, StoreError::Io(File::MenuBar, _)),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "The change could not be saved.",
        "plain text for the pane; the OS error goes to the log"
    );
    assert!(matches!(
        s.store(&Paths::default(), File::Dock),
        Err(StoreError::NoPath(File::Dock))
    ));
}

#[test]
fn a_typed_mouse_resolution_keeps_what_the_server_wrote_since_the_last_read() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let file = dir.path().join("compositor.json");
    std::fs::write(&file, r#"{"appearance": "light", "corner_radius": 10}"#).unwrap();
    let mut s = Settings::load(&p, 1080.0).unwrap();
    std::fs::write(
        &file,
        r#"{"appearance": "dark", "corner_radius": 10, "mouse_dpi": {"1:2:Other": 800}}"#,
    )
    .unwrap();
    s.set_mouse_dpi(&p, "1:1:Mouse", 1600).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains(r#""appearance": "dark""#), "{text}");
    assert!(text.contains(r#""1:2:Other": 800"#), "{text}");
    assert!(text.contains(r#""1:1:Mouse": 1600"#), "{text}");
    assert!(s.set_mouse_dpi(&p, "1:1:Mouse", 49).is_err());
    assert_eq!(
        s.compositor
            .mouse_dpi
            .as_ref()
            .and_then(|m| m.get("1:1:Mouse")),
        Some(&1600),
        "a refused value leaves the settings as they were"
    );
}

#[test]
fn adding_a_folder_and_choosing_a_photo_write_over_what_the_server_wrote_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let file = dir.path().join("compositor.json");
    std::fs::write(
        &file,
        r#"{"appearance": "light", "wallpaper_dark": "/old-dark.jpg"}"#,
    )
    .unwrap();
    let mut s = Settings::load(&p, 1080.0).unwrap();
    std::fs::write(
        &file,
        r#"{"appearance": "dark", "wallpaper_dark": "/old-dark.jpg", "wallpaper_folders": ["/one"]}"#,
    )
    .unwrap();
    s.add_wallpaper_folder(&p, std::path::Path::new("/two"))
        .unwrap();
    s.add_wallpaper_folder(&p, std::path::Path::new("/two"))
        .unwrap();
    s.set_wallpaper(&p, std::path::Path::new("/home/u/me.png"))
        .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains(r#""appearance": "dark""#), "{text}");
    assert!(
        text.contains(r#""/one""#) && text.contains(r#""/two""#),
        "{text}"
    );
    assert_eq!(
        text.matches("\"/two\"").count(),
        1,
        "a folder is listed once: {text}"
    );
    assert!(text.contains(r#""wallpaper": "/home/u/me.png""#), "{text}");
    assert!(
        !text.contains("old-dark"),
        "a single picture forgets the dark one: {text}"
    );
}

#[test]
fn a_relative_folder_or_picture_is_refused_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let mut s = Settings::load(&p, 1080.0).unwrap();
    assert!(s
        .add_wallpaper_folder(&p, std::path::Path::new("Pictures"))
        .is_err());
    assert!(s.set_wallpaper(&p, std::path::Path::new("me.png")).is_err());
    assert!(!dir.path().join("compositor.json").exists());
    assert_eq!(s.compositor.wallpaper, None);
    assert_eq!(s.compositor.wallpaper_folders, None);
}

#[test]
fn the_notifications_pane_writes_the_default_and_each_apps_choice_and_keeps_the_rest() {
    use cupertino_defaults::notifications::{NotificationSettings, Policy, Previews};
    let dir = tempfile::tempdir().unwrap();
    let p = paths(dir.path());
    let file = dir.path().join("notifications.conf");
    std::fs::write(
        &file,
        "# mine\nshow_previews = never\napp.org.example.Mail.show_on_lock_screen = off\nfuture_key = 1\n",
    )
    .unwrap();
    let live = Live {
        notifying_apps: vec![("org.example.Chat".into(), "Chat".into())],
        ..Live::default()
    };
    let mut s = Settings::load(&p, 1080.0).unwrap();
    let mut commit = |id, value| {
        for effect in change(id, value, &mut s, &live) {
            match effect {
                Effect::Store(file) => s.store(&p, file).unwrap(),
                other => panic!("only a write: {other:?}"),
            }
        }
    };
    commit(ControlId::NotificationsLockScreen, Value::Bool(false));
    // listed by name: Chat (from the centre), then Mail (from the file)
    commit(ControlId::NotificationAppPreviews(0), Value::Index(1));
    commit(ControlId::NotificationAppLockScreen(0), Value::Index(1));
    commit(ControlId::NotificationAppLockScreen(1), Value::Index(0));
    let text = std::fs::read_to_string(&file).unwrap();
    for kept in ["# mine", "future_key = 1", "show_previews = never"] {
        assert!(text.contains(kept), "{kept:?} kept in:\n{text}");
    }
    assert!(
        !text.contains("app.org.example.Mail"),
        "Default drops the app's key:\n{text}"
    );
    let (read, problems) = NotificationSettings::load(&file, None).unwrap();
    assert!(
        problems.iter().all(|p| p.message.contains("future_key")),
        "{problems:?}"
    );
    assert_eq!(
        read.policy("org.example.Chat"),
        Policy {
            show_on_lock_screen: true,
            show_previews: Previews::Always
        }
    );
    assert_eq!(
        read.policy("org.example.Mail"),
        Policy {
            show_on_lock_screen: false,
            show_previews: Previews::Never
        },
        "the default"
    );
}
