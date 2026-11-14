//! The Wi-Fi and Network panes on NetworkManager, when `cupertino-netd` is not running: the same [`Wifi`] reading
//! and calls as `org.cupertino.Network1`, worked out from NetworkManager's objects.

use std::collections::HashMap;

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::live::{display_name, Device, KnownNetwork, Network, Vpn, Wifi};

pub const NAME: &str = "org.freedesktop.NetworkManager";
pub const PATH: &str = "/org/freedesktop/NetworkManager";
pub const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";

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

/// The key of an SSID in [`Network::ssid`]: its text when it is UTF-8, else a NUL then its bytes in hex, so two SSIDs
/// that differ only in bytes that are not text stay apart and a Join reaches the one chosen.
pub fn ssid_key(ssid: &[u8]) -> String {
    match std::str::from_utf8(ssid) {
        Ok(text) if !text.starts_with('\0') => text.to_string(),
        _ => std::iter::once('\0')
            .chain(ssid.iter().flat_map(|b| {
                [b >> 4, b & 0xf].map(|n| char::from_digit(u32::from(n), 16).expect("a nibble"))
            }))
            .collect(),
    }
}

/// The SSID bytes of a key of [`ssid_key`].
fn ssid_bytes(key: &str) -> Vec<u8> {
    match key.strip_prefix('\0') {
        Some(hex) => hex
            .as_bytes()
            .chunks(2)
            .filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect(),
        None => key.as_bytes().to_vec(),
    }
}

struct AccessPoint {
    path: OwnedObjectPath,
    ssid: Vec<u8>,
    strength: u8,
    security: &'static str,
}

fn access_point(connection: &Connection, path: OwnedObjectPath) -> zbus::Result<AccessPoint> {
    let p = proxy(connection, path.as_str(), ACCESS_POINT)?;
    Ok(AccessPoint {
        ssid: p.get_property("Ssid")?,
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
                ssid = display_name(&String::from_utf8_lossy(&ap.ssid));
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
fn saved(connection: &Connection) -> zbus::Result<Vec<(Vec<u8>, OwnedObjectPath)>> {
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
            Some((ssid.try_clone().ok()?.try_into().ok()?, path))
        })
        .collect())
}

/// One saved Wi-Fi connection: its network's SSID bytes, its object and whether it joins by itself.
struct SavedWifi {
    ssid: Vec<u8>,
    path: OwnedObjectPath,
    auto_join: bool,
    secured: bool,
}

/// The saved Wi-Fi connections, in range or not.
fn saved_wifi(connection: &Connection) -> zbus::Result<Vec<SavedWifi>> {
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
            let auto_join = settings
                .get("connection")
                .and_then(|c| c.get("autoconnect"))
                .and_then(|v| bool::try_from(&**v).ok())
                .unwrap_or(true);
            Some(SavedWifi {
                ssid: ssid.try_clone().ok()?.try_into().ok()?,
                secured: settings.contains_key("802-11-wireless-security"),
                auto_join,
                path,
            })
        })
        .collect())
}

/// Take the saved connections of the network `ssid` ([`ssid_key`]) away, so the machine no longer joins it.
pub fn forget(connection: &Connection, ssid: &str) -> zbus::Result<()> {
    let ssid = ssid_bytes(ssid);
    for saved in saved_wifi(connection)?
        .into_iter()
        .filter(|s| s.ssid == ssid)
    {
        proxy(connection, saved.path.as_str(), CONNECTION)?.call::<_, _, ()>("Delete", &())?;
    }
    Ok(())
}

/// Let the machine join the network `ssid` by itself, or stop it.
pub fn set_auto_join(connection: &Connection, ssid: &str, on: bool) -> zbus::Result<()> {
    let ssid = ssid_bytes(ssid);
    for saved in saved_wifi(connection)?
        .into_iter()
        .filter(|s| s.ssid == ssid)
    {
        let p = proxy(connection, saved.path.as_str(), CONNECTION)?;
        let mut settings: HashMap<String, HashMap<String, OwnedValue>> =
            p.call("GetSettings", &())?;
        settings
            .entry("connection".to_owned())
            .or_default()
            .insert("autoconnect".to_owned(), Value::from(on).try_into()?);
        p.call::<_, _, ()>("Update", &(settings,))?;
    }
    Ok(())
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
            let key = ssid_key(&ap.ssid);
            match networks.iter_mut().find(|n| n.ssid == key) {
                Some(n) => {
                    n.signal = n.signal.max(signal);
                    n.connected |= connected;
                }
                None => networks.push(Network {
                    name: display_name(&String::from_utf8_lossy(&ap.ssid)),
                    ssid: key,
                    signal,
                    security: ap.security.to_string(),
                    saved: saved.iter().any(|(s, _)| *s == ap.ssid),
                    connected,
                }),
            }
        }
    }
    networks.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)));
    let known = saved_wifi(connection)?
        .into_iter()
        .map(|s| {
            let key = ssid_key(&s.ssid);
            let seen = networks.iter().find(|n| n.ssid == key);
            KnownNetwork {
                name: display_name(&String::from_utf8_lossy(&s.ssid)),
                auto_join: Some(s.auto_join),
                in_range: seen.is_some(),
                connected: seen.is_some_and(|n| n.connected),
                secured: s.secured,
                ssid: key,
            }
        })
        .collect();
    let state: u32 = nm.get_property("State")?;
    Ok(Wifi {
        known,
        can_forget: true,
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
fn strongest(connection: &Connection, ssid: &[u8]) -> zbus::Result<(OwnedObjectPath, AccessPoint)> {
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
        .ok_or_else(|| {
            let name = display_name(&String::from_utf8_lossy(ssid));
            zbus::fdo::Error::Failed(format!("“{name}” is out of range.")).into()
        })
}

/// Join the network of key `ssid` ([`ssid_key`]): its saved connection, else a new one with `passphrase` for a WPA
/// personal network.
pub fn join(connection: &Connection, ssid: &str, passphrase: &str) -> zbus::Result<()> {
    let nm = proxy(connection, PATH, NAME)?;
    let ssid = ssid_bytes(ssid);
    let (device, ap) = strongest(connection, &ssid)?;
    if let Some((_, saved)) = saved(connection)?.into_iter().find(|(s, _)| *s == ssid) {
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

/// The VPN connections the system holds, and which of them are active.
pub fn vpns(connection: &Connection) -> zbus::Result<Vec<Vpn>> {
    let nm = proxy(connection, PATH, NAME)?;
    let mut active: Vec<(String, u32)> = Vec::new();
    let actives: Vec<OwnedObjectPath> = nm.get_property("ActiveConnections")?;
    for path in actives {
        let a = proxy(connection, path.as_str(), ACTIVE)?;
        if let Ok(uuid) = a.get_property::<String>("Uuid") {
            active.push((uuid, a.get_property("State").unwrap_or(0)));
        }
    }
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
            let c = settings.get("connection")?;
            let kind: String = c.get("type")?.try_clone().ok()?.try_into().ok()?;
            if kind != "vpn" && kind != "wireguard" {
                return None;
            }
            let text = |k: &str| -> Option<String> { c.get(k)?.try_clone().ok()?.try_into().ok() };
            let id = text("uuid")?;
            let state = active.iter().find(|(u, _)| *u == id).map(|(_, s)| *s);
            Some(Vpn {
                name: display_name(&text("id").unwrap_or_else(|| id.clone())),
                kind: vpn_kind(&kind, settings.get("vpn")).to_owned(),
                // NM_ACTIVE_CONNECTION_STATE_ACTIVATED is 2; ACTIVATING 1 and DEACTIVATING 3 are in between
                active: state == Some(2),
                busy: matches!(state, Some(1) | Some(3)),
                id,
            })
        })
        .collect())
}

/// What a VPN connection is called in the list: the plug-in's name where NetworkManager gives one.
fn vpn_kind(kind: &str, vpn: Option<&HashMap<String, OwnedValue>>) -> &'static str {
    if kind == "wireguard" {
        return "WireGuard";
    }
    let service: Option<String> = vpn
        .and_then(|v| v.get("service-type"))
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| v.try_into().ok());
    match service.as_deref().and_then(|s| s.rsplit('.').next()) {
        Some("openvpn") => "OpenVPN",
        Some("openconnect") => "OpenConnect",
        Some("vpnc") => "Cisco",
        Some("pptp") => "PPTP",
        Some("l2tp") => "L2TP",
        Some("strongswan") => "IPsec",
        _ => "VPN",
    }
}

/// Connect the VPN `id` (its UUID), or disconnect it.
pub fn set_vpn(connection: &Connection, id: &str, on: bool) -> zbus::Result<()> {
    let nm = proxy(connection, PATH, NAME)?;
    if on {
        let paths: Vec<OwnedObjectPath> =
            proxy(connection, SETTINGS_PATH, SETTINGS)?.call("ListConnections", &())?;
        for path in paths {
            let settings: HashMap<String, HashMap<String, OwnedValue>> =
                proxy(connection, path.as_str(), CONNECTION)?.call("GetSettings", &())?;
            let uuid: Option<String> = settings
                .get("connection")
                .and_then(|c| c.get("uuid"))
                .and_then(|v| v.try_clone().ok())
                .and_then(|v| v.try_into().ok());
            if uuid.as_deref() == Some(id) {
                let none = OwnedObjectPath::try_from("/")?;
                return nm
                    .call::<_, _, OwnedObjectPath>("ActivateConnection", &(path, &none, &none))
                    .map(drop);
            }
        }
        return Err(zbus::fdo::Error::Failed("That VPN connection is gone.".into()).into());
    }
    let actives: Vec<OwnedObjectPath> = nm.get_property("ActiveConnections")?;
    for path in actives {
        let uuid: String = proxy(connection, path.as_str(), ACTIVE)?.get_property("Uuid")?;
        if uuid == id {
            nm.call::<_, _, ()>("DeactivateConnection", &(path,))?;
        }
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

    #[test]
    fn ssids_that_differ_only_in_bytes_that_are_not_text_keep_their_own_keys() {
        let (a, b) = (b"caf\xe9".as_slice(), b"caf\xe8".as_slice());
        assert_eq!(String::from_utf8_lossy(a), String::from_utf8_lossy(b));
        assert_ne!(ssid_key(a), ssid_key(b));
        for ssid in [
            a,
            b,
            b"home".as_slice(),
            "café".as_bytes(),
            b"\0x".as_slice(),
            b"".as_slice(),
        ] {
            assert_eq!(ssid_bytes(&ssid_key(ssid)), ssid, "{ssid:?}");
        }
        assert_eq!(ssid_key(b"home"), "home");
    }
}
