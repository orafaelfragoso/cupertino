//! The Bluetooth adapter on `org.cupertino.Bluetooth1` (`cupertino-btd`), read from the service every time.

use std::collections::HashMap;

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedValue;

use crate::bus::Bounded;
use crate::types::{display_name, Adapter, BtDevice};

pub const NAME: &str = "org.cupertino.Bluetooth1";
pub const PATH: &str = "/org/cupertino/Bluetooth1";

fn proxy(connection: &Connection) -> zbus::Result<Proxy<'static>> {
    Builder::new(connection)
        .destination(NAME)?
        .path(PATH)?
        .interface(NAME)?
        .cache_properties(CacheProperties::No)
        .build()
}

/// A device of `Devices`; one without its always-present keys is skipped.
fn device(d: &HashMap<String, OwnedValue>) -> Option<BtDevice> {
    let text = |k: &str| {
        d.get(k)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
    };
    let flag = |k: &str| d.get(k).and_then(|v| bool::try_from(v).ok());
    Some(BtDevice {
        id: text("Id")?,
        address: text("Address")?,
        name: text("Name").map(|n| display_name(&n)),
        icon: text("Icon")?,
        paired: flag("Paired")?,
        connected: flag("Connected")?,
        pairing: flag("Pairing")?,
        // an older daemon does not say
        connecting: flag("Connecting").unwrap_or(false),
    })
}

/// The adapter and its devices.
pub fn adapter(connection: &Connection) -> zbus::Result<Adapter> {
    let p = proxy(connection)?;
    let devices: Vec<HashMap<String, OwnedValue>> = p.read("Devices")?;
    Ok(Adapter {
        available: p.read("Available")?,
        powered: p.read("Powered")?,
        discovering: p.read("Discovering")?,
        devices: devices.iter().filter_map(device).collect(),
    })
}

pub fn set_powered(connection: &Connection, on: bool) -> zbus::Result<()> {
    proxy(connection)?.write("Powered", on)
}

pub fn connect(connection: &Connection, id: &str) -> zbus::Result<()> {
    proxy(connection)?.ask("Connect", &(id,))
}

pub fn disconnect(connection: &Connection, id: &str) -> zbus::Result<()> {
    proxy(connection)?.ask("Disconnect", &(id,))
}
