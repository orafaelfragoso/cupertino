//! What the bar's menus hold: an exported menu the App menu emptied, the Bluetooth panel and the system menu's labels.

use cupertino_keyboard::rewrite::ClientClass;
use cupertino_menubar::extras::{
    BluetoothDevice, BluetoothState, ExtraAction, ExtraKind, Extras, SessionCaps, SessionState,
    SystemAction,
};
use cupertino_menubar::fallback::{self, Chords};
use cupertino_menubar::item::{ItemId, MenuItem, PanelRow};
use cupertino_menubar::merge::{Export, Exporter};
use cupertino_menubar::recent::RecentItems;

fn hidden(id: i32, label: &str) -> MenuItem {
    MenuItem {
        visible: false,
        ..MenuItem::new(id, label)
    }
}

/// The merged menus of an exporter's `tree`.
fn merged(tree: &MenuItem) -> Vec<MenuItem> {
    let chords = Chords::default();
    let export = Export {
        tree,
        exporter: Exporter::DbusMenu,
        names: ["T3 Code", "t3code"],
        class: ClientClass::Other,
    };
    let system = fallback::menus(1, Some(0), true, true, false, &[], &chords, true);
    let (_, menus) = export.merge(
        fallback::app_menu("T3 Code", &chords),
        system,
        chords.primary,
    );
    menus
}

/// The titles of the merged menus of an exporter's `tree`.
fn merged_titles(tree: &MenuItem) -> Vec<String> {
    merged(tree).into_iter().map(|m| m.label).collect()
}

#[test]
fn electrons_toggle_full_screen_stands_for_the_view_menus_full_screen_item() {
    let view = MenuItem::submenu(
        4,
        "View",
        vec![
            MenuItem::new(21, "Reload"),
            MenuItem::separator(32),
            MenuItem::new(33, "Toggle Full Screen"),
        ],
    );
    let tree = MenuItem::submenu(0, "Root", vec![view]);
    let view = merged(&tree)
        .into_iter()
        .find(|m| m.label == "View")
        .unwrap();
    let labels: Vec<&str> = view
        .children
        .iter()
        .filter(|c| !c.separator)
        .map(|c| c.label.as_str())
        .collect();
    assert_eq!(labels, ["Reload", "Toggle Full Screen"]);
}

#[test]
fn a_file_menu_left_with_only_hidden_items_once_settings_and_quit_leave_it_is_not_shown() {
    // Electron's File menu: Settings..., a separator, a hidden Close Developer Tools and Quit
    let file = MenuItem::submenu(
        2,
        "File",
        vec![
            MenuItem::new(7, "Settings..."),
            MenuItem::separator(8),
            hidden(9, "Close Developer Tools"),
            MenuItem::new(10, "Quit"),
        ],
    );
    let edit = MenuItem::submenu(3, "Edit", vec![MenuItem::new(11, "Undo")]);
    let tree = MenuItem::submenu(0, "Root", vec![file, edit]);
    let titles = merged_titles(&tree);
    assert!(!titles.iter().any(|t| t == "File"), "{titles:?}");
    assert!(titles.iter().any(|t| t == "Edit"), "{titles:?}");
}

#[test]
fn a_file_menu_with_an_item_of_its_own_left_is_shown() {
    let file = MenuItem::submenu(
        2,
        "File",
        vec![MenuItem::new(6, "New Window"), MenuItem::new(10, "Quit")],
    );
    let tree = MenuItem::submenu(0, "Root", vec![file]);
    assert!(merged_titles(&tree).iter().any(|t| t == "File"));
}

fn bluetooth(powered: bool, devices: Vec<BluetoothDevice>) -> Extras {
    Extras {
        bluetooth: Some(BluetoothState { powered, devices }),
        ..Extras::default()
    }
}

fn device(id: &str, name: &str, connected: bool) -> BluetoothDevice {
    BluetoothDevice {
        id: id.to_owned(),
        address: "00:11:22:33:44:55".to_owned(),
        name: Some(name.to_owned()),
        paired: true,
        connected,
    }
}

fn panels(extras: &Extras) -> Vec<PanelRow> {
    extras
        .menu(ExtraKind::Bluetooth)
        .rows
        .into_iter()
        .filter_map(|r| r.panel)
        .collect()
}

#[test]
fn the_bluetooth_panel_without_devices_says_so_and_opens_the_settings() {
    let extras = bluetooth(true, Vec::new());
    let rows = panels(&extras);
    assert_eq!(
        rows,
        vec![
            PanelRow::Header {
                title: "Bluetooth".to_owned(),
                on: true
            },
            PanelRow::Section {
                title: "Devices".to_owned()
            },
            PanelRow::Note {
                text: "No devices".to_owned()
            },
            PanelRow::Link {
                title: "Bluetooth Settings\u{2026}".to_owned()
            },
        ]
    );
    let menu = extras.menu(ExtraKind::Bluetooth);
    let settings = menu.rows.last().unwrap().id;
    assert_eq!(
        menu.action(settings),
        Some(&ExtraAction::System(SystemAction::BluetoothSettings))
    );
}

#[test]
fn the_bluetooth_panel_lists_paired_devices_by_name_and_connects_or_disconnects_them() {
    let extras = bluetooth(
        true,
        vec![
            device("b", "Speaker", false),
            device("a", "Headphones", true),
        ],
    );
    let menu = extras.menu(ExtraKind::Bluetooth);
    let devices: Vec<(ItemId, PanelRow)> = menu
        .rows
        .iter()
        .filter_map(|r| match &r.panel {
            Some(p @ PanelRow::Device { .. }) => Some((r.id, p.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        devices.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>(),
        vec![
            PanelRow::Device {
                name: "Headphones".to_owned(),
                connected: true
            },
            PanelRow::Device {
                name: "Speaker".to_owned(),
                connected: false
            },
        ]
    );
    assert_eq!(
        menu.action(devices[0].0),
        Some(&ExtraAction::DisconnectDevice("a".to_owned()))
    );
    assert_eq!(
        menu.action(devices[1].0),
        Some(&ExtraAction::ConnectDevice("b".to_owned()))
    );
}

#[test]
fn the_bluetooth_panel_while_off_shows_its_switch_and_the_settings() {
    let rows = panels(&bluetooth(false, vec![device("a", "Headphones", false)]));
    assert_eq!(rows.len(), 2);
    assert!(matches!(rows[0], PanelRow::Header { on: false, .. }));
    assert!(matches!(rows[1], PanelRow::Link { .. }));
}

#[test]
fn the_system_menu_labels_carry_no_ellipsis() {
    let extras = Extras {
        session: Some(SessionState {
            user_name: "Ada".to_owned(),
            caps: SessionCaps::default(),
        }),
        ..Extras::default()
    };
    let menu = extras.system_menu(&Chords::default(), &RecentItems::default(), None);
    let labels: Vec<&str> = menu
        .rows
        .iter()
        .filter(|r| !r.separator)
        .map(|r| r.label.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "About This Computer",
            "Settings",
            "Recent Items",
            "Force Quit",
            "Sleep",
            "Restart",
            "Shut Down",
            "Lock Screen",
            "Log Out Ada",
            "Login Window",
        ]
    );
}
