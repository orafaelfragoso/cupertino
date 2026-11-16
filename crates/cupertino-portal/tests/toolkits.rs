//! The toolkit settings (overlay scroll bars, the title bar buttons) reach GTK's own places: GSettings (through the real `gsettings`, with its key-file backend in a
//! scratch folder) and the GTK settings files, a linked one included.

use std::path::Path;
use std::process::Command;

use cupertino_portal::toolkits::{write_gsettings, write_settings_ini, ToolkitSettings};
use cupertino_test_bus::missing_tool;

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
        .args(["list-keys", "org.gnome.desktop.interface"])
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

#[test]
fn gsettings_takes_whether_scroll_bars_overlay_the_content_each_change_and_where_the_buttons_go() {
    if !schema_installed() {
        return missing_tool("gsettings with the org.gnome.desktop.interface schema");
    }
    let config = tempfile::tempdir().unwrap();
    let make = || gsettings(config.path());
    for overlay in [false, true, false] {
        write_gsettings(
            ToolkitSettings {
                overlay_scroll_bars: overlay,
            },
            &make,
        )
        .unwrap();
        assert_eq!(
            read(
                config.path(),
                "org.gnome.desktop.interface",
                "overlay-scrolling"
            ),
            overlay.to_string()
        );
        assert_eq!(
            read(
                config.path(),
                "org.gnome.desktop.wm.preferences",
                "button-layout"
            ),
            "'close,minimize,maximize:'"
        );
    }
}

#[test]
fn the_gtk_settings_files_take_the_value_and_keep_the_person_s_own_lines() {
    let config = tempfile::tempdir().unwrap();
    let mine = config.path().join("dotfiles/gtk3.ini");
    std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
    std::fs::write(&mine, "[Settings]\ngtk-theme-name=Mine\n").unwrap();
    std::fs::create_dir_all(config.path().join("gtk-3.0")).unwrap();
    std::os::unix::fs::symlink(&mine, config.path().join("gtk-3.0/settings.ini")).unwrap();

    let overlay = |overlay_scroll_bars| ToolkitSettings {
        overlay_scroll_bars,
    };
    write_settings_ini(config.path(), overlay(false)).unwrap();
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
    write_settings_ini(config.path(), overlay(true)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&mine).unwrap(),
        "[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling=true\ngtk-decoration-layout=close,minimize,maximize:\n"
    );
}
