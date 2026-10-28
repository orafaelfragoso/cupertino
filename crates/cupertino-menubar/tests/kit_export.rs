//! A CupertinoKit application's exported menus read by the bar's own `org.gtk.Menus` reader, on a private session bus
//! (`dbus-daemon --session`); skipped when the daemon is not available.
#![cfg(feature = "dbus")]

use std::process::{Command, Stdio};

use cupertino_menu_model::standard::{self, Features};
use cupertino_menu_model::{Modifier, ModifierMap, PhysicalModifier};
use cupertino_menubar::dbus::gtk::GtkMenuClient;
use cupertino_menubar::item::MenuItem;
use cupertino_menubar::source::GtkMenuProps;
use cupertino_ui_menus::{Exporter, WindowId};
use zbus::block_on;

struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let address = lines.next()?.to_string();
        let pid = lines.next()?.to_string();
        Some(PrivateBus { address, pid })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = Command::new("kill").arg(&self.pid).status();
    }
}

/// The cupertino profile: Primary is Super.
struct Cupertino;

impl ModifierMap for Cupertino {
    fn physical(&self, m: Modifier) -> Option<PhysicalModifier> {
        Some(match m {
            Modifier::Primary => PhysicalModifier::Super,
            Modifier::Ctrl => PhysicalModifier::Control,
            Modifier::Alt => PhysicalModifier::Alt,
            Modifier::Shift => PhysicalModifier::Shift,
        })
    }

    fn logical(&self, p: PhysicalModifier) -> Option<Modifier> {
        [
            Modifier::Primary,
            Modifier::Ctrl,
            Modifier::Alt,
            Modifier::Shift,
        ]
        .into_iter()
        .find(|m| self.physical(*m) == Some(p))
    }
}

fn find<'a>(items: &'a [MenuItem], label: &str) -> Option<&'a MenuItem> {
    items.iter().find_map(|item| {
        (item.label == label)
            .then_some(item)
            .or_else(|| find(&item.children, label))
    })
}

#[test]
fn the_bar_reads_the_labels_a_kit_application_exports() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let exporting = zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap();
    let mut exporter =
        Exporter::new(exporting, "org.example.Notes", Box::new(Cupertino), |_| {}).unwrap();
    let features = Features {
        about: true,
        new: Some("New my_note".to_string()),
        ..Features::default()
    };
    let exported = exporter
        .export(WindowId(1), &standard::menu_bar("Notes", &features))
        .unwrap();

    block_on(async {
        let reader = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let props = GtkMenuProps {
            bus_name: exported.unique_bus_name,
            application_id: Some(exported.application_id),
            application_object_path: Some(exported.application_object_path),
            window_object_path: Some(exported.window_object_path),
            menubar_object_path: Some(exported.menubar_path),
            app_menu_object_path: Some(exported.app_menu_path),
        };
        let (_client, items) = GtkMenuClient::load(&reader, &props).await.unwrap();

        let file = find(&items, "File").expect("the File menu");
        assert!(file.has_submenu);
        let new = find(&items, "New my_note").expect("the New item, its underscore kept");
        assert_eq!(new.mnemonic, None);
        assert!(new.shortcut.is_some(), "New has its chord");
    });
}
