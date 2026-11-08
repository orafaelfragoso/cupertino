//! The routing files send every interface this backend serves to it, and the rest elsewhere.

use std::collections::{BTreeMap, BTreeSet};

use cupertino_portal::app_chooser::AppChooser;
use cupertino_portal::file_chooser::FileChooser;
use cupertino_portal::screen_cast::ScreenCast;
use cupertino_portal::screenshot::Screenshot;
use cupertino_portal::settings::Settings;
use cupertino_portal::BUS_NAME;
use zbus::object_server::Interface;

/// The routing file the packages install.
const PACKAGED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packaging/files/usr/share/xdg-desktop-portal/cupertino-portals.conf"
));
/// The routing file of a manual install (README, "Installing").
const CRATE: &str = include_str!("../data/cupertino-portals.conf");
/// The interfaces xdg-desktop-portal may ask this backend for.
const PORTAL_FILE: &str = include_str!("../data/cupertino.portal");

/// The backend interfaces this crate implements, by the name its D-Bus objects carry. Colour picking is a method of
/// Screenshot. Request and Session are objects of a call, not interfaces a frontend routes.
fn served() -> BTreeSet<String> {
    [
        Settings::name(),
        FileChooser::name(),
        ScreenCast::name(),
        AppChooser::name(),
        Screenshot::name(),
    ]
    .into_iter()
    .map(|name| name.to_string())
    .collect()
}

/// The backend's short name, as the routing files use it.
fn cupertino() -> &'static str {
    BUS_NAME.rsplit('.').next().unwrap()
}

/// The `[preferred]` keys of a routing file, each with its backends in order.
fn preferred(conf: &str) -> BTreeMap<&str, Vec<&str>> {
    let mut lines = conf.lines().map(str::trim).filter(|l| !l.is_empty());
    assert_eq!(lines.next(), Some("[preferred]"));
    lines
        .map(|line| {
            assert!(!line.starts_with('['), "one section only: {line}");
            let (key, backends) = line.split_once('=').expect("key=value");
            (key, backends.split(';').filter(|b| !b.is_empty()).collect())
        })
        .collect()
}

#[test]
fn the_portal_file_lists_exactly_the_interfaces_served() {
    let listed: BTreeSet<String> = PORTAL_FILE
        .lines()
        .find_map(|l| l.strip_prefix("Interfaces="))
        .unwrap()
        .split(';')
        .filter(|i| !i.is_empty())
        .map(str::to_owned)
        .collect();
    assert_eq!(listed, served());
}

#[test]
fn the_packaged_conf_sends_every_served_interface_here_first() {
    let keys = preferred(PACKAGED);
    for interface in served() {
        let backends = keys
            .get(interface.as_str())
            .unwrap_or_else(|| panic!("{interface} is not routed"));
        assert_eq!(backends.first(), Some(&cupertino()), "{interface}");
    }
}

#[test]
fn the_packaged_conf_falls_back_for_the_rest() {
    let keys = preferred(PACKAGED);
    assert_eq!(keys["default"], ["gtk"], "every other interface");
    // the GTK backend also serves these, so it answers when this backend cannot
    for interface in ["Settings", "FileChooser"] {
        assert_eq!(
            keys[format!("org.freedesktop.impl.portal.{interface}").as_str()],
            [cupertino(), "gtk"]
        );
    }
    assert_eq!(
        keys["org.freedesktop.impl.portal.Secret"],
        ["gnome-keyring"]
    );
    // no interface is sent here that this backend does not serve
    let served = served();
    for (key, backends) in &keys {
        if backends.contains(&cupertino()) {
            assert!(served.contains(*key), "{key} is routed here but not served");
        }
    }
}

#[test]
fn the_manual_install_routes_as_the_packages_do() {
    assert_eq!(CRATE, PACKAGED);
}
