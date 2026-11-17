//! The pure parts of asking an application for a window over D-Bus.

#![cfg(feature = "dbus")]

use cupertino_apps::activate::{object_path, platform_data};
use zbus::zvariant::Value;

#[test]
fn the_object_path_has_dots_as_slashes_and_dashes_as_underscores() {
    for (id, path) in [
        ("org.gnome.Nautilus", "/org/gnome/Nautilus"),
        ("org.foo-bar.App-2", "/org/foo_bar/App_2"),
    ] {
        assert_eq!(object_path(id), path, "{id}");
    }
}

#[test]
fn the_platform_data_carries_the_token_under_both_keys_or_nothing() {
    let data = platform_data(Some("tok"));
    assert_eq!(data.len(), 2);
    assert_eq!(data["activation-token"], Value::from("tok"));
    assert_eq!(data["desktop-startup-id"], Value::from("tok"));
    assert!(platform_data(None).is_empty());
}
