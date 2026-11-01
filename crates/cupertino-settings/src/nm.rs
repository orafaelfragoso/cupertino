//! The Wi-Fi and Network panes on NetworkManager, when `cupertino-netd` is not running: the same [`Wifi`] reading
//! and calls as `org.cupertino.Network1`, worked out from NetworkManager's objects.

use std::collections::HashMap;

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::live::{Device, Network, Wifi};

pub const NAME: &str = "org.freedesktop.NetworkManager";
pub const PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";

/// `NM_DEVICE_TYPE_ETHERNET` and `NM_DEVICE_TYPE_WIFI`; other kinds are not listed.
const ETHERNET: u32 = 1;
const WIFI: u32 = 2;

/// `NM_802_11_AP_FLAGS_PRIVACY` and the key-management bits of `NM80211ApSecurityFlags`.
const PRIVACY: u32 = 0x1;
const KEY_MGMT_PSK: u32 = 0x100;
const KEY_MGMT_802_1X: u32 = 0x200;
const KEY_MGMT_SAE: u32 = 0x400;
const KEY_MGMT_OWE: u32 = 0x800;

/// The most access points and saved connections read, so a crowded air or a long history cannot stall the pane.
const MAX_ACCESS_POINTS: usize = 256;
const MAX_CONNECTIONS: usize = 256;

fn proxy<'a>(connection: &Connection, path: &str, interface: &'a str) -> zbus::Result<Proxy<'a>> {
    Builder::new(connection)
        .destination(NAME)?
        .path(path.to_owned())?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
}

/// What the pane calls a device state, as `cupertino-netd` names them (`NMDeviceState`).
fn device_state(state: u32) -> &'static str {
    match state {
        0..=20 => "unavailable",
        30 | 110 | 120 => "disconnected",
        100 => "connected",
        _ => "configuring",
    }
}

/// The overall state (`NMState`), in `cupertino-netd`'s words.
fn overall_state(state: u32) -> &'static str {
    match state {
        10 => "asleep",
        20 | 30 => "disconnected",
        40 => "connecting",
        50..=70 => "connected",
        _ => "unknown",
    }
}

/// An access point's security, as `cupertino-netd` names it.
pub fn security(flags: u32, wpa: u32, rsn: u32) -> &'static str {
    let keys = wpa | rsn;
    if keys & KEY_MGMT_802_1X != 0 {
        "enterprise"
    } else if rsn & KEY_MGMT_SAE != 0 && rsn & KEY_MGMT_PSK != 0 {
        "wpa2-wpa3"
    } else if rsn & KEY_MGMT_SAE != 0 {
        "wpa3"
    } else if rsn & KEY_MGMT_PSK != 0 {
        "wpa2"
    } else if wpa & KEY_MGMT_PSK != 0 {
        "wpa"
    } else if rsn & KEY_MGMT_OWE != 0 {
        "owe"
    } else if flags & PRIVACY != 0 {
        "wep"
    } else {
        "open"
    }
}

/// NetworkManager's signal strength (0 to 100) as the dBm `Network::signal` holds, the usual linear mapping.
pub fn dbm(strength: u8) -> i32 {
    i32::from(strength.min(100)) / 2 - 100
}

struct AccessPoint {
    path: OwnedObjectPath,
    ssid: String,
    strength: u8,
    security: &'static str,
}

fn access_point(connection: &Connection, path: OwnedObjectPath) -> zbus::Result<AccessPoint> {
    let p = proxy(connection, path.as_str(), ACCESS_POINT)?;
    let ssid: Vec<u8> = p.get_property("Ssid")?;
    Ok(AccessPoint {
        ssid: String::from_utf8_lossy(&ssid).into_owned(),
        strength: p.get_property("Strength")?,
        security: security(
            p.get_property("Flags")?,
            p.get_property("WpaFlags")?,
            p.get_property("RsnFlags")?,
        ),
        path,
    })
}

struct WifiDevice {
    path: OwnedObjectPath,
    active: Option<OwnedObjectPath>,
    access_points: Vec<AccessPoint>,
}

/// The listed devices: each Ethernet and Wi-Fi one, and the Wi-Fi ones' access points.
fn devices(connection: &Connection) -> zbus::Result<(Vec<Device>, Vec<WifiDevice>)> {
    let paths: Vec<OwnedObjectPath> = proxy(connection, PATH, NAME)?.call("GetDevices", &())?;
    let mut devices = Vec::new();
    let mut wifi = Vec::new();
    for (index, path) in paths.into_iter().enumerate() {
        let d = proxy(connection, path.as_str(), DEVICE)?;
        let kind: u32 = d.get_property("DeviceType")?;
        if kind != ETHERNET && kind != WIFI {
            continue;
        }
        let state: u32 = d.get_property("State")?;
        let (_, reason): (u32, u32) = d.get_property("StateReason")?;
        let mut ssid = String::new();
        if kind == WIFI {
            let w = proxy(connection, path.as_str(), WIRELESS)?;
            let active: OwnedObjectPath = w.get_property("ActiveAccessPoint")?;
            let active = (active.as_str() != "/").then_some(active);
            let listed: Vec<OwnedObjectPath> = w.call("GetAllAccessPoints", &())?;
            let access_points: Vec<AccessPoint> = listed
                .into_iter()
                .take(MAX_ACCESS_POINTS)
                .filter_map(|ap| access_point(connection, ap).ok())
                .collect();
            if let Some(ap) = access_points
                .iter()
                .find(|ap| Some(&ap.path) == active.as_ref())
            {
                ssid = ap.ssid.clone();
            }
            wifi.push(WifiDevice {
                path,
                active,
                access_points,
            });
        }
        devices.push(Device {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            name: d.get_property("Interface")?,
            kind: if kind == WIFI { "wifi" } else { "ethernet" }.to_string(),
            state: device_state(state).to_string(),
            ssid,
            failure: if state == 120 {
                format!("failed (reason {reason})")
            } else {
                String::new()
            },
            address: d.get_property("HwAddress")?,
        });
    }
    Ok((devices, wifi))
}

/// The SSIDs of the saved Wi-Fi connections, with the connection of each.
fn saved(connection: &Connection) -> zbus::Result<Vec<(String, OwnedObjectPath)>> {
    let paths: Vec<OwnedObjectPath> =
        proxy(connection, SETTINGS_PATH, SETTINGS)?.call("ListConnections", &())?;
    Ok(paths
        .into_iter()
        .take(MAX_CONNECTIONS)
        .filter_map(|path| {
            let settings: HashMap<String, HashMap<String, OwnedValue>> =
                proxy(connection, path.as_str(), CONNECTION)
                    .ok()?
                    .call("GetSettings", &())
                    .ok()?;
            let ssid = settings.get("802-11-wireless")?.get("ssid")?;
            let ssid: Vec<u8> = ssid.try_clone().ok()?.try_into().ok()?;
            Some((String::from_utf8_lossy(&ssid).into_owned(), path))
        })
        .collect())
}

/// The reading the panes show.
pub fn wifi(connection: &Connection) -> zbus::Result<Wifi> {
    let nm = proxy(connection, PATH, NAME)?;
    let (devices, radios) = devices(connection)?;
    let saved = saved(connection)?;
    let mut networks: Vec<Network> = Vec::new();
    for radio in &radios {
        for ap in &radio.access_points {
            if ap.ssid.is_empty() {
                continue;
            }
            let connected = Some(&ap.path) == radio.active.as_ref();
            let signal = dbm(ap.strength);
            match networks.iter_mut().find(|n| n.ssid == ap.ssid) {
                Some(n) => {
                    n.signal = n.signal.max(signal);
                    n.connected |= connected;
                }
                None => networks.push(Network {
                    ssid: ap.ssid.clone(),
                    signal,
                    security: ap.security.to_string(),
                    saved: saved.iter().any(|(s, _)| *s == ap.ssid),
                    connected,
                }),
            }
        }
    }
    networks.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)));
    let state: u32 = nm.get_property("State")?;
    Ok(Wifi {
        enabled: nm.get_property("WirelessEnabled")?,
        devices,
        state: overall_state(state).to_string(),
        networks,
        joining: None,
        failure: None,
    })
}

pub fn set_wireless_enabled(connection: &Connection, on: bool) -> zbus::Result<()> {
    proxy(connection, PATH, NAME)?
        .set_property("WirelessEnabled", on)
        .map_err(zbus::Error::from)
}

/// The Wi-Fi device to join with and the strongest access point named `ssid` it sees.
fn strongest(connection: &Connection, ssid: &str) -> zbus::Result<(OwnedObjectPath, AccessPoint)> {
    let (_, radios) = devices(connection)?;
    radios
        .into_iter()
        .flat_map(|r| {
            let device = r.path;
            r.access_points
                .into_iter()
                .filter(|ap| ap.ssid == ssid)
                .map(move |ap| (device.clone(), ap))
        })
        .max_by_key(|(_, ap)| ap.strength)
        .ok_or_else(|| zbus::fdo::Error::Failed(format!("“{ssid}” is out of range.")).into())
}

/// Join `ssid`: its saved connection, else a new one with `passphrase` for a WPA personal network.
pub fn join(connection: &Connection, ssid: &str, passphrase: &str) -> zbus::Result<()> {
    let nm = proxy(connection, PATH, NAME)?;
    let (device, ap) = strongest(connection, ssid)?;
    if let Some((_, saved)) = saved(connection)?.into_iter().find(|(s, _)| s == ssid) {
        return nm
            .call::<_, _, OwnedObjectPath>("ActivateConnection", &(saved, &device, &ap.path))
            .map(drop);
    }
    let key_mgmt = match ap.security {
        "open" => None,
        "wpa" | "wpa2" | "wpa2-wpa3" => Some("wpa-psk"),
        "wpa3" => Some("sae"),
        other => {
            return Err(zbus::fdo::Error::Failed(format!(
                "Joining a {other} network is not supported here."
            ))
            .into())
        }
    };
    let mut settings: HashMap<&str, HashMap<&str, Value<'_>>> = HashMap::new();
    if let Some(key_mgmt) = key_mgmt {
        settings.insert(
            "802-11-wireless-security",
            HashMap::from([
                ("key-mgmt", Value::from(key_mgmt)),
                ("psk", Value::from(passphrase)),
            ]),
        );
    }
    nm.call::<_, _, (OwnedObjectPath, OwnedObjectPath)>(
        "AddAndActivateConnection",
        &(settings, &device, &ap.path),
    )
    .map(drop)
}

/// Disconnect every Wi-Fi device that is connected.
pub fn disconnect(connection: &Connection) -> zbus::Result<()> {
    let (_, radios) = devices(connection)?;
    for radio in radios.iter().filter(|r| r.active.is_some()) {
        proxy(connection, radio.path.as_str(), DEVICE)?.call::<_, _, ()>("Disconnect", &())?;
    }
    Ok(())
}

/// Ask every Wi-Fi device to scan.
pub fn scan(connection: &Connection) -> zbus::Result<()> {
    let (_, radios) = devices(connection)?;
    for radio in &radios {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        proxy(connection, radio.path.as_str(), WIRELESS)?
            .call::<_, _, ()>("RequestScan", &(options,))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_and_signal_are_named_as_the_network_daemon_names_them() {
        assert_eq!(security(0, 0, 0), "open");
        assert_eq!(security(PRIVACY, 0, 0), "wep");
        assert_eq!(security(PRIVACY, KEY_MGMT_PSK, 0), "wpa");
        assert_eq!(security(PRIVACY, KEY_MGMT_PSK, KEY_MGMT_PSK), "wpa2");
        assert_eq!(security(PRIVACY, 0, KEY_MGMT_SAE), "wpa3");
        assert_eq!(
            security(PRIVACY, 0, KEY_MGMT_PSK | KEY_MGMT_SAE),
            "wpa2-wpa3"
        );
        assert_eq!(security(PRIVACY, 0, KEY_MGMT_802_1X), "enterprise");
        assert_eq!(security(0, 0, KEY_MGMT_OWE), "owe");
        assert_eq!((dbm(0), dbm(100), dbm(255)), (-100, -50, -50));
    }
}
