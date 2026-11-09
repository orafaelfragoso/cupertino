//! Every event the server sends its shell processes and applications through its own protocols and the toolkit menu
//! protocols fits a Wayland message: each string or array argument has a cap, and the event's encoded size with every
//! such argument at its cap is at most 4096 bytes. An event with a string or an array that has no row here fails the
//! test, so a new one needs its cap stated.

use cupertino_protocols::server::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::server::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::server::screenshot::cupertino_screenshot_v1::CupertinoScreenshotV1;
use cupertino_protocols::server::scroll::cupertino_scroll_v1::CupertinoScrollV1;
use cupertino_server::menu_bar::{
    BACKDROP_SPREAD_MAX_BANDS, MAX_APPLICATION_ID_BYTES, MAX_BUS_NAME_BYTES, MAX_OBJECT_PATH_BYTES,
    MAX_TEXT_BYTES,
};
use cupertino_server::protocols::gtk_shell::gtk_shell1::GtkShell1;
use cupertino_server::toplevel_icon::MAX_ICON_NAME;
use wayland_backend::protocol::{ArgumentType, Interface};
use wayland_protocols_plasma::appmenu::server::org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager;
use wayland_server::Resource;

/// The longest message the wire allows; a longer event ends the receiving client's connection.
const MAX_MESSAGE_BYTES: usize = 4096;

/// The longest chord the bar is sent: four modifier names and the longest keysym name, well under this. The server sends
/// only chords of its own key bindings, written by `KeyCombo` (`cupertino-input`), so no client sets one.
const MAX_CHORD_BYTES: usize = 128;

/// Length of the foreign toplevel identifier: 32 random alphanumeric characters, which Smithay generates for each
/// toplevel; no client sets it.
const TOPLEVEL_IDENTIFIER_BYTES: usize = 32;

/// Every event with a string or an array argument, with the longest each of those arguments can be, in order.
const CAPS: &[(&str, &str, &[usize])] = &[
    ("cupertino_window_manager_v1", "window", &[MAX_TEXT_BYTES]),
    (
        "cupertino_menu_bar_v1",
        "active_app",
        &[MAX_TEXT_BYTES, MAX_TEXT_BYTES],
    ),
    (
        "cupertino_menu_bar_v1",
        "menu_dbusmenu",
        &[MAX_BUS_NAME_BYTES, MAX_OBJECT_PATH_BYTES],
    ),
    (
        "cupertino_menu_bar_v1",
        "menu_gtk",
        &[
            MAX_BUS_NAME_BYTES,
            MAX_APPLICATION_ID_BYTES,
            MAX_OBJECT_PATH_BYTES,
            MAX_OBJECT_PATH_BYTES,
            MAX_OBJECT_PATH_BYTES,
            MAX_OBJECT_PATH_BYTES,
        ],
    ),
    ("cupertino_menu_bar_v1", "app_window", &[MAX_TEXT_BYTES]),
    ("cupertino_menu_bar_v1", "shortcut", &[MAX_CHORD_BYTES]),
    // a 16-bit spread per band
    (
        "cupertino_menu_bar_v1",
        "backdrop_spread",
        &[BACKDROP_SPREAD_MAX_BANDS * 2],
    ),
    (
        "cupertino_dock_v1",
        "window",
        &[MAX_TEXT_BYTES, MAX_TEXT_BYTES],
    ),
    ("cupertino_dock_v1", "launching", &[MAX_TEXT_BYTES]),
    ("cupertino_dock_v1", "switcher_entry", &[MAX_TEXT_BYTES]),
    ("cupertino_dock_v1", "overview_changed", &[MAX_TEXT_BYTES]),
    ("cupertino_dock_v1", "attention", &[MAX_TEXT_BYTES]),
    ("cupertino_dock_v1", "window_icon", &[MAX_ICON_NAME]),
    ("cupertino_dock_v1", "unresponsive", &[MAX_TEXT_BYTES]),
    ("cupertino_dock_v1", "app_resident", &[MAX_TEXT_BYTES]),
    (
        "cupertino_app_menu_v1",
        "menu_reader",
        &[MAX_BUS_NAME_BYTES],
    ),
    (
        "cupertino_screenshot_v1",
        "window",
        &[TOPLEVEL_IDENTIFIER_BYTES],
    ),
    // never sent by the server; each of the 5 states or 4 edge constraints at most once, 4 bytes each
    ("gtk_surface1", "configure", &[5 * 4]),
    ("gtk_surface1", "configure_edges", &[4 * 4]),
];

/// The bytes a string of `len` bytes takes on the wire: its length, then the string, its NUL and padding to 4.
fn wire_string(len: usize) -> usize {
    4 + (len + 1).next_multiple_of(4)
}

/// The bytes an array of `len` bytes takes on the wire: its length, then the bytes padded to 4.
fn wire_array(len: usize) -> usize {
    4 + len.next_multiple_of(4)
}

/// The interfaces of each global the table covers and every interface their requests and events create.
fn interfaces() -> Vec<&'static Interface> {
    let mut found: Vec<&'static Interface> = Vec::new();
    let mut next = vec![
        CupertinoShellV1::interface(),
        CupertinoScreenshotV1::interface(),
        CupertinoAppEffectsV1::interface(),
        CupertinoScrollV1::interface(),
        GtkShell1::interface(),
        OrgKdeKwinAppmenuManager::interface(),
    ];
    while let Some(interface) = next.pop() {
        if found.iter().any(|i| i.name == interface.name) {
            continue;
        }
        found.push(interface);
        next.extend(
            interface
                .requests
                .iter()
                .chain(interface.events)
                .filter_map(|m| m.child_interface),
        );
    }
    found
}

#[test]
fn every_event_with_a_string_or_an_array_fits_a_message_at_its_caps() {
    let mut missing = Vec::new();
    let mut too_long = Vec::new();
    let mut covered = Vec::new();
    for interface in interfaces() {
        for event in interface.events {
            let sized = |a: &&ArgumentType| matches!(a, ArgumentType::Str(_) | ArgumentType::Array);
            let count = event.signature.iter().filter(sized).count();
            if count == 0 {
                continue;
            }
            let name = format!("{}.{}", interface.name, event.name);
            let Some(&(_, _, caps)) = CAPS
                .iter()
                .find(|(i, e, _)| *i == interface.name && *e == event.name)
            else {
                missing.push(name);
                continue;
            };
            assert_eq!(caps.len(), count, "{name}: one cap per string or array");
            let mut caps = caps.iter().copied();
            let size = 8 + event
                .signature
                .iter()
                .map(|a| match a {
                    ArgumentType::Str(_) => wire_string(caps.next().unwrap()),
                    ArgumentType::Array => wire_array(caps.next().unwrap()),
                    ArgumentType::Fd => 0,
                    _ => 4,
                })
                .sum::<usize>();
            eprintln!("{name}: {size}");
            if size > MAX_MESSAGE_BYTES {
                too_long.push((name.clone(), size));
            }
            covered.push((interface.name, event.name));
        }
    }
    assert!(
        missing.is_empty(),
        "events with a string or an array and no row: {missing:?}"
    );
    assert!(
        too_long.is_empty(),
        "events over {MAX_MESSAGE_BYTES} bytes at their caps: {too_long:?}"
    );
    let stale: Vec<_> = CAPS
        .iter()
        .filter(|(i, e, _)| !covered.contains(&(*i, *e)))
        .collect();
    assert!(
        stale.is_empty(),
        "rows naming no event with a string or an array: {stale:?}"
    );
}
