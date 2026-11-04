// A network and a Bluetooth daemon for the semantics test of the window (`src/shell/tests/semantics.rs`), which
// includes this file: two wired and wireless devices, a known and an other network, a paired and a nearby device.
// Test fakes stay out of `src`, where every D-Bus interface is one a crate serves (`ci/contracts.py`).

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, OwnedValue, Value as Variant};

pub struct FakeNetwork;

#[zbus::interface(name = "org.cupertino.Network1")]
impl FakeNetwork {
    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<(u32, String, String, String, String, String, String)> {
        let device = |index, name: &str, kind: &str, ssid: &str, address: &str| {
            let text = |s: &str| s.to_owned();
            let state = text("connected");
            (
                index,
                text(name),
                text(kind),
                state,
                text(ssid),
                String::new(),
                text(address),
            )
        };
        vec![
            device(1, "eth0", "ethernet", "", "10.0.0.3"),
            device(2, "wlan0", "wifi", "home", "10.0.0.2"),
        ]
    }

    #[zbus(property)]
    fn state(&self) -> String {
        "connected".into()
    }

    fn get_networks(&self) -> Vec<(String, i32, String, bool, bool)> {
        vec![
            ("home".into(), -40, "wpa2".into(), true, true),
            ("cafe".into(), -70, "open".into(), false, false),
        ]
    }

    fn scan(&self) {}
}

fn bt_device(id: &str, name: &str, paired: bool) -> HashMap<String, OwnedValue> {
    let v = |x: Variant<'_>| x.try_to_owned().unwrap();
    HashMap::from([
        ("Id".to_string(), v(Variant::from(id))),
        ("Address".to_string(), v(Variant::from("00:11:22:33:44:55"))),
        ("Name".to_string(), v(Variant::from(name))),
        ("Icon".to_string(), v(Variant::from("input-keyboard"))),
        ("Paired".to_string(), v(Variant::from(paired))),
        ("Connected".to_string(), v(Variant::from(paired))),
        ("Pairing".to_string(), v(Variant::from(false))),
    ])
}

pub struct FakeBluetooth;

#[zbus::interface(name = "org.cupertino.Bluetooth1")]
impl FakeBluetooth {
    #[zbus(property)]
    fn available(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn powered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn discovering(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<HashMap<String, OwnedValue>> {
        vec![
            bt_device("hp", "Headphones", true),
            bt_device("kb", "Keyboard", false),
        ]
    }

    fn start_discovery(&self) {}

    fn stop_discovery(&self) {}

    fn register_agent(&self, _path: ObjectPath<'_>) {}

    fn unregister_agent(&self) {}
}
