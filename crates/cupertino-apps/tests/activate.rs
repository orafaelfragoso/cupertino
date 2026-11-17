//! The pure parts of asking an application for a window over D-Bus.

#![cfg(feature = "dbus")]

use cupertino_apps::activate::{object_path, platform_data, ACTIVATE_TIMEOUT, INTERFACE};
use std::time::Duration;
use zbus::zvariant::Value;

#[test]
fn the_object_path_has_the_dots_of_the_id_as_slashes() {
    assert_eq!(object_path("org.gnome.Nautilus"), "/org/gnome/Nautilus");
}

#[test]
fn the_object_path_has_the_dashes_of_the_id_as_underscores() {
    assert_eq!(object_path("org.foo-bar.App-2"), "/org/foo_bar/App_2");
}

#[test]
fn the_platform_data_carries_the_token_twice() {
    let data = platform_data(Some("tok"));
    assert_eq!(data.len(), 2);
    assert_eq!(data["activation-token"], Value::from("tok"));
    assert_eq!(data["desktop-startup-id"], Value::from("tok"));
}

#[test]
fn the_platform_data_is_empty_without_a_token() {
    assert!(platform_data(None).is_empty());
}

#[test]
fn an_application_has_three_seconds_to_answer() {
    assert_eq!(ACTIVATE_TIMEOUT, Duration::from_secs(3));
    assert_eq!(INTERFACE, "org.freedesktop.Application");
}
