//! The network on `org.cupertino.Network1` (`cupertino-netd`): its radio, devices and networks, read from the service
//! every time.

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;

use crate::types::{display_name, Device, KnownNetwork, Link, Network, Wifi};

pub const NAME: &str = "org.cupertino.Network1";
pub const PATH: &str = "/org/cupertino/Network1";

/// A row of `Devices`: index, name, kind, state, SSID, failure, address.
type DeviceRecord = (u32, String, String, String, String, String, String);
/// A row of `GetNetworks`: SSID, signal (dBm, `i32::MIN` unknown), security, saved, connected.
type NetworkRecord = (String, i32, String, bool, bool);

fn proxy(connection: &Connection) -> zbus::Result<Proxy<'static>> {
    Builder::new(connection)
        .destination(NAME)?
        .path(PATH)?
        .interface(NAME)?
        .cache_properties(CacheProperties::No)
        .build()
}

fn device((index, name, kind, state, ssid, failure, address): DeviceRecord) -> Device {
    Device {
        index,
        name,
        kind,
        state,
        ssid: display_name(&ssid),
        failure,
        address,
    }
}

/// The whole reading: the radio, the devices, the networks in range and the saved ones.
pub fn wifi(connection: &Connection) -> zbus::Result<Wifi> {
    let p = proxy(connection)?;
    let devices: Vec<DeviceRecord> = p.get_property("Devices")?;
    let networks: Vec<NetworkRecord> = p.call("GetNetworks", &())?;
    // a daemon that lists its saved networks (and can forget them) says so by answering
    let listed: Option<Vec<(String, bool)>> = p.call("GetKnownNetworks", &()).ok();
    let in_list = |ssid: &str| networks.iter().find(|n| n.0 == ssid);
    let known: Vec<KnownNetwork> = match &listed {
        Some(listed) => listed
            .iter()
            .map(|(ssid, auto_join)| {
                let seen = in_list(ssid);
                KnownNetwork {
                    ssid: ssid.clone(),
                    name: display_name(ssid),
                    auto_join: Some(*auto_join),
                    in_range: seen.is_some(),
                    connected: seen.is_some_and(|n| n.4),
                    secured: seen.is_none_or(|n| n.2 != "open"),
                }
            })
            .collect(),
        None => networks
            .iter()
            .filter(|n| n.3)
            .map(|n| KnownNetwork {
                ssid: n.0.clone(),
                name: display_name(&n.0),
                auto_join: None,
                in_range: true,
                connected: n.4,
                secured: n.2 != "open",
            })
            .collect(),
    };
    Ok(Wifi {
        known,
        can_forget: listed.is_some(),
        enabled: p.get_property("WirelessEnabled")?,
        devices: devices.into_iter().map(device).collect(),
        state: p.get_property("State")?,
        networks: networks
            .into_iter()
            .map(|(ssid, signal, security, saved, connected)| Network {
                name: display_name(&ssid),
                ssid,
                signal,
                security,
                saved,
                connected,
            })
            .collect(),
        joining: None,
        failure: None,
    })
}

/// The radio and its first Wi-Fi device, without the networks.
pub fn link(connection: &Connection) -> zbus::Result<Link> {
    let p = proxy(connection)?;
    let devices: Vec<DeviceRecord> = p.get_property("Devices")?;
    Ok(Link {
        enabled: p.get_property("WirelessEnabled")?,
        device: devices.into_iter().find(|d| d.2 == "wifi").map(device),
    })
}

pub fn set_wireless_enabled(connection: &Connection, on: bool) -> zbus::Result<()> {
    proxy(connection)?
        .set_property("WirelessEnabled", on)
        .map_err(zbus::Error::from)
}

/// `Connect`: with an empty passphrase an open network, or one whose credentials the service holds.
pub fn join(connection: &Connection, ssid: &str, passphrase: &str) -> zbus::Result<()> {
    proxy(connection)?.call("Connect", &(ssid, passphrase))
}

pub fn disconnect(connection: &Connection) -> zbus::Result<()> {
    proxy(connection)?.call("Disconnect", &())
}

pub fn scan(connection: &Connection) -> zbus::Result<()> {
    proxy(connection)?.call("Scan", &())
}
