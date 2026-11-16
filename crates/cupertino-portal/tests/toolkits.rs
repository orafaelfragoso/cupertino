//! The toolkit settings (overlay scroll bars, the title bar buttons) reach GTK's own places: GSettings (through the real `gsettings`, with its key-file backend in a
//! scratch folder) and the GTK settings files, a linked one included. A value the person sets there since is kept.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use cupertino_portal::toolkits::{write_gsettings, write_settings_ini, ToolkitSettings, Written};
use cupertino_test_bus::missing_tool;

const INTERFACE: &str = "org.gnome.desktop.interface";
const WINDOW: &str = "org.gnome.desktop.wm.preferences";

/// `gsettings` writing into the key file under `config` instead of the user's settings database.
fn gsettings(config: &Path) -> Command {
    let mut command = Command::new("gsettings");
    command
        .env("GSETTINGS_BACKEND", "keyfile")
        .env("XDG_CONFIG_HOME", config);
    command
}

/// Whether `gsettings` and the schema of the desktop's interface settings are installed.
fn schema_installed() -> bool {
    Command::new("gsettings")
        .args(["list-keys", INTERFACE])
        .output()
        .is_ok_and(|o| {
            o.status.success() && String::from_utf8_lossy(&o.stdout).contains("overlay-scrolling")
        })
}

fn read(config: &Path, schema: &str, key: &str) -> String {
    let output = gsettings(config)
        .args(["get", schema, key])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn set(config: &Path, schema: &str, key: &str, value: &str) {
    let status = gsettings(config)
        .args(["set", schema, key, value])
        .status()
        .unwrap();
    assert!(status.success());
}

fn overlay(overlay_scroll_bars: bool) -> ToolkitSettings {
    ToolkitSettings {
        overlay_scroll_bars,
    }
}

#[test]
fn gsettings_takes_whether_scroll_bars_overlay_the_content_each_change_and_where_the_buttons_go() {
    if !schema_installed() {
        return missing_tool("gsettings with the org.gnome.desktop.interface schema");
    }
    let config = tempfile::tempdir().unwrap();
    let make = || gsettings(config.path());
    let mut written = Written::default();
    for value in [false, true, false] {
        write_gsettings(overlay(value), &make, &mut written).unwrap();
        assert_eq!(
            read(config.path(), INTERFACE, "overlay-scrolling"),
            value.to_string()
        );
        assert_eq!(
            read(config.path(), WINDOW, "button-layout"),
            "'close,minimize,maximize:'"
        );
    }
}

#[test]
fn gsettings_keeps_a_value_the_person_set_since_until_the_desktop_s_own_changes() {
    if !schema_installed() {
        return missing_tool("gsettings with the org.gnome.desktop.interface schema");
    }
    let config = tempfile::tempdir().unwrap();
    let state = config.path().join("state/toolkit-settings");
    let make = || gsettings(config.path());
    let start = || Written::load(Some(state.clone()));

    let mut written = start();
    write_gsettings(overlay(false), &make, &mut written).unwrap();
    written.save().unwrap();
    // the person sets their own, in another desktop for instance
    set(config.path(), WINDOW, "button-layout", "'menu:close'");
    set(config.path(), INTERFACE, "overlay-scrolling", "true");

    // the portal starts again with the same settings: theirs stay
    let mut written = start();
    write_gsettings(overlay(false), &make, &mut written).unwrap();
    assert_eq!(read(config.path(), WINDOW, "button-layout"), "'menu:close'");
    assert_eq!(read(config.path(), INTERFACE, "overlay-scrolling"), "true");

    // the desktop's scroll bar setting changes: it goes in, and the next change follows it
    write_gsettings(overlay(true), &make, &mut written).unwrap();
    write_gsettings(overlay(false), &make, &mut written).unwrap();
    assert_eq!(read(config.path(), INTERFACE, "overlay-scrolling"), "false");
    assert_eq!(read(config.path(), WINDOW, "button-layout"), "'menu:close'");
}

#[test]
fn the_gtk_settings_files_take_the_value_and_keep_the_person_s_own_lines() {
    let config = tempfile::tempdir().unwrap();
    let mine = config.path().join("dotfiles/gtk3.ini");
    std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
    std::fs::write(&mine, "[Settings]\ngtk-theme-name=Mine\n").unwrap();
    std::fs::create_dir_all(config.path().join("gtk-3.0")).unwrap();
    std::os::unix::fs::symlink(&mine, config.path().join("gtk-3.0/settings.ini")).unwrap();

    let mut written = Written::default();
    write_settings_ini(config.path(), overlay(false), &mut written).unwrap();
    assert_eq!(
        std::fs::read_to_string(&mine).unwrap(),
        "[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling=false\ngtk-decoration-layout=close,minimize,maximize:\n"
    );
    assert!(
        config
            .path()
            .join("gtk-3.0/settings.ini")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link stays"
    );
    assert_eq!(
        std::fs::read_to_string(config.path().join("gtk-4.0/settings.ini")).unwrap(),
        "[Settings]\ngtk-overlay-scrolling=false\ngtk-decoration-layout=close,minimize,maximize:\n",
        "a missing file is made"
    );
    write_settings_ini(config.path(), overlay(true), &mut written).unwrap();
    assert_eq!(
        std::fs::read_to_string(&mine).unwrap(),
        "[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling=true\ngtk-decoration-layout=close,minimize,maximize:\n"
    );
}

#[test]
fn a_gtk_settings_file_keeps_its_permissions_and_the_person_s_own_values_since() {
    let config = tempfile::tempdir().unwrap();
    let file = config.path().join("gtk-3.0/settings.ini");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "[Settings]\ngtk-theme-name=Mine\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let state = config.path().join("state/toolkit-settings");

    let mut written = Written::load(Some(state.clone()));
    write_settings_ini(config.path(), overlay(false), &mut written).unwrap();
    written.save().unwrap();
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // the person sets their own button order, twice in the group: GTK takes the last
    std::fs::write(
        &file,
        "[Settings]\ngtk-decoration-layout=close:\ngtk-overlay-scrolling=false\ngtk-decoration-layout=menu:close\n",
    )
    .unwrap();
    let mut written = Written::load(Some(state));
    write_settings_ini(config.path(), overlay(true), &mut written).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "[Settings]\ngtk-decoration-layout=close:\ngtk-overlay-scrolling=true\ngtk-decoration-layout=menu:close\n",
        "the changed scroll bar setting goes in, the person's button order stays"
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
