//! The Bluetooth adapter on BlueZ, when `cupertino-btd` is not running: the same [`Adapter`] reading and calls as
//! `org.cupertino.Bluetooth1`, on the first adapter. A device's id is its object path.

use std::collections::{HashMap, HashSet};

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use crate::types::{display_name, Adapter, BtDevice};

pub const NAME: &str = "org.bluez";
/// Where the stack's objects live.
pub const PATHS: &str = "/org/bluez";
pub const ADAPTER: &str = "org.bluez.Adapter1";
pub const DEVICE: &str = "org.bluez.Device1";

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// An object of the stack, its properties read every time.
pub fn proxy<'a>(
    connection: &Connection,
    path: &str,
    interface: &'a str,
) -> zbus::Result<Proxy<'a>> {
    Builder::new(connection)
        .destination(NAME)?
        .path(path.to_owned())?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
}

fn objects(connection: &Connection) -> zbus::Result<Objects> {
    proxy(connection, "/", "org.freedesktop.DBus.ObjectManager")?.call("GetManagedObjects", &())
}

/// The adapter everything works with: the first by path, as `bluetoothctl` picks it.
fn adapter(objects: &Objects) -> Option<&OwnedObjectPath> {
    objects
        .iter()
        .filter(|(_, interfaces)| interfaces.contains_key(ADAPTER))
        .map(|(path, _)| path)
        .min_by(|a, b| a.as_str().cmp(b.as_str()))
}

fn text(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

fn flag(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    properties.get(key).and_then(|v| bool::try_from(v).ok())
}

/// The adapter and its devices; `pairing` are the devices whose `Pair` has not returned.
pub fn read(connection: &Connection, pairing: &HashSet<String>) -> zbus::Result<Adapter> {
    let objects = objects(connection)?;
    let Some(adapter_path) = adapter(&objects) else {
        return Ok(Adapter::default());
    };
    let adapter = &objects[adapter_path][ADAPTER];
    let mut devices: Vec<BtDevice> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let d = interfaces.get(DEVICE)?;
            let on: OwnedObjectPath = d.get("Adapter")?.try_clone().ok()?.try_into().ok()?;
            if on != *adapter_path {
                return None;
            }
            Some(BtDevice {
                id: path.to_string(),
                address: text(d, "Address")?,
                name: text(d, "Name").map(|n| display_name(&n)),
                icon: text(d, "Icon").unwrap_or_default(),
                paired: flag(d, "Paired")?,
                connected: flag(d, "Connected")?,
                pairing: pairing.contains(path.as_str()),
                // BlueZ has no such state: the caller marks the devices it is connecting
                connecting: false,
            })
        })
        .collect();
    devices.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Adapter {
        available: true,
        powered: flag(adapter, "Powered").unwrap_or(false),
        discovering: flag(adapter, "Discovering").unwrap_or(false),
        devices,
    })
}

fn the_adapter(connection: &Connection) -> zbus::Result<Proxy<'static>> {
    let objects = objects(connection)?;
    let path = adapter(&objects).ok_or_else(|| {
        zbus::Error::from(zbus::fdo::Error::Failed(
            "There is no Bluetooth adapter.".into(),
        ))
    })?;
    proxy(connection, path.as_str(), ADAPTER)
}

/// A device of the adapter by its id; any other path is refused, so a call never reaches another object.
fn device(connection: &Connection, id: &str) -> zbus::Result<Proxy<'static>> {
    let objects = objects(connection)?;
    let known = ObjectPath::try_from(id)
        .ok()
        .and_then(|path| objects.get(&OwnedObjectPath::from(path)))
        .is_some_and(|interfaces| interfaces.contains_key(DEVICE));
    if !known {
        return Err(zbus::fdo::Error::Failed("The device is no longer available.".into()).into());
    }
    proxy(connection, id, DEVICE)
}

pub fn set_powered(connection: &Connection, on: bool) -> zbus::Result<()> {
    the_adapter(connection)?
        .set_property("Powered", on)
        .map_err(zbus::Error::from)
}

pub fn discovery(connection: &Connection, on: bool) -> zbus::Result<()> {
    let method = if on {
        "StartDiscovery"
    } else {
        "StopDiscovery"
    };
    the_adapter(connection)?.call(method, &())
}

pub fn connect(connection: &Connection, id: &str) -> zbus::Result<()> {
    device(connection, id)?.call("Connect", &())
}

pub fn disconnect(connection: &Connection, id: &str) -> zbus::Result<()> {
    device(connection, id)?.call("Disconnect", &())
}

pub fn remove(connection: &Connection, id: &str) -> zbus::Result<()> {
    device(connection, id)?;
    let path = ObjectPath::try_from(id).map_err(zbus::Error::from)?;
    the_adapter(connection)?.call("RemoveDevice", &(path,))
}

/// The device to pair, checked; [`pair`] runs on the pairing's own thread.
pub fn pairing(connection: &Connection, id: &str) -> zbus::Result<Proxy<'static>> {
    device(connection, id)
}

/// Pair, then trust the device, so its services connect again later without asking (as BlueZ's own clients do).
pub fn pair(device: &Proxy<'_>) -> zbus::Result<()> {
    device.call::<_, _, ()>("Pair", &())?;
    device
        .set_property("Trusted", true)
        .map_err(zbus::Error::from)
}
