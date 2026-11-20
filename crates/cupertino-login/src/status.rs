//! What the status items at the top right of the lock and login screens show: the Wi-Fi radio, the battery, the
//! input source and, on the login screen, a line about the machine. [`read`] reads them on Kit's I/O pool; [`follow`]
//! says when a reading is out of date: the network or the power daemon changed. The views draw what the last reading
//! said.

use std::time::Duration;

use cupertino_defaults::login::HostInfo;

/// How long a reading of the status items should take on the I/O pool: a few calls on the system bus, each bounded by
/// the radios' own timeout, and two small files.
pub const READ_DEADLINE: Duration = Duration::from_secs(10);

/// The Wi-Fi radio as its item shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiState {
    /// The radio is off.
    Off,
    /// On, joined to no network.
    NotConnected,
    /// Joined, with the signal's strength in bars, 0 (not reported) to 3.
    Connected { bars: u8 },
}

/// The battery as its item shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatteryState {
    /// 0 to 100.
    pub percentage: f64,
    pub charging: bool,
}

/// One reading of the status items; `None` where the machine has no such thing or it could not be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusItems {
    pub wifi: Option<WifiState>,
    pub battery: Option<BatteryState>,
    /// The input source's short name, as "US".
    pub input: Option<String>,
    /// The line about the machine the login screen's `host-info` asks for.
    pub host: Option<String>,
}

/// The strength, 0 (not reported) to 3 bars, of a signal in dBm, as the menu bar shows it.
pub fn signal_bars(dbm: Option<i32>) -> u8 {
    match dbm {
        None | Some(i32::MIN) => 0,
        Some(d) if d >= -55 => 3,
        Some(d) if d >= -67 => 2,
        Some(_) => 1,
    }
}

/// The short name of an input source given as an xkb layout with an optional `(variant)` ("de(nodeadkeys)": "DE");
/// `None` for an empty one or a decoded layout file, which has no short name.
pub fn input_label(source: &str) -> Option<String> {
    let layout = source.split(['(', ',']).next()?.trim();
    if layout.is_empty() || layout.starts_with("decoded:") {
        return None;
    }
    Some(layout.to_uppercase())
}

/// The input source the item shows: the first of the person's `keyboard.conf`, else the session's default xkb layout
/// (`XKB_DEFAULT_LAYOUT`), else "us" (approximate: the source in use is the server's to switch). Reads a file: run it on
/// the I/O pool.
pub fn current_input() -> Option<String> {
    use cupertino_defaults::keyboard::KeyboardSettings;
    let first = KeyboardSettings::path()
        .and_then(|path| KeyboardSettings::load(&path, None).ok())
        .and_then(|(settings, _)| settings.input_sources)
        .and_then(|sources| sources.into_iter().next())
        .map(|source| source.layout);
    let layout = first
        .filter(|l| !l.is_empty())
        .or_else(|| std::env::var("XKB_DEFAULT_LAYOUT").ok())
        .unwrap_or_else(|| "us".to_owned());
    input_label(&layout)
}

/// The version line of `os-release` text: `PRETTY_NAME`, else `NAME` and `VERSION_ID`.
pub fn os_version(os_release: &str) -> Option<String> {
    let value = |key: &str| {
        os_release.lines().find_map(|line| {
            let rest = line.strip_prefix(key)?.strip_prefix('=')?;
            let rest = rest.trim().trim_matches('"').trim_matches('\'');
            (!rest.is_empty()).then(|| rest.to_owned())
        })
    };
    value("PRETTY_NAME").or_else(|| match (value("NAME"), value("VERSION_ID")) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (name, _) => name,
    })
}

/// The longest line about the machine the item shows, in characters.
pub const MAX_HOST_CHARS: usize = 64;

/// The line about the machine `info` asks for, read now: its host name, its system's version or its first IPv4 address
/// that is not a loopback one. Reads a file: run it on the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
pub fn host_text(info: HostInfo) -> Option<String> {
    let text = match info {
        HostInfo::Hostname => hostname(),
        HostInfo::Version => std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| os_version(&text)),
        HostInfo::Ip => first_address(),
    }?;
    Some(text.chars().take(MAX_HOST_CHARS).collect())
}

fn hostname() -> Option<String> {
    cupertino_kit::host::name()
}

fn first_address() -> Option<String> {
    cupertino_kit::host::ipv4_addresses()
        .ok()?
        .into_iter()
        .find(|ip| !ip.is_loopback())
        .map(|ip| ip.to_string())
}

#[cfg(feature = "dbus")]
pub use bus::{follow, read};

#[cfg(feature = "dbus")]
mod bus {
    use super::{current_input, host_text, signal_bars, BatteryState, StatusItems, WifiState};
    use cupertino_defaults::login::HostInfo;
    use futures_lite::stream::{self, StreamExt};
    use std::sync::Arc;
    use zbus::blocking::Connection;
    use zbus::fdo::{DBusProxy, PropertiesProxy};
    use zbus::proxy::CacheProperties;
    use zbus::zvariant::OwnedValue;

    const UPOWER: &str = "org.freedesktop.UPower";
    const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
    const DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";
    const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
    /// UPower's device states that mean power comes in: charging and fully charged.
    const CHARGING: u32 = 1;
    const FULLY_CHARGED: u32 = 4;

    /// The Wi-Fi radio now, through whichever service serves the network; `None` without one or a Wi-Fi device.
    fn wifi(system: &Connection) -> Option<WifiState> {
        let backend = cupertino_radios::detect(system).ok()?.network?;
        let wifi = backend.wifi(system).ok()?;
        if !wifi.has_wifi_device() {
            return None;
        }
        if !wifi.enabled {
            return Some(WifiState::Off);
        }
        Some(match wifi.networks.iter().find(|n| n.connected) {
            Some(network) => WifiState::Connected {
                bars: signal_bars(Some(network.signal)),
            },
            None => WifiState::NotConnected,
        })
    }

    /// The battery now, from the power daemon's display device; `None` without a battery.
    #[expect(
        clippy::disallowed_methods,
        reason = "runs on the I/O pool (kit::task::io), as the radios' readings beside it"
    )]
    fn battery(system: &Connection) -> Option<BatteryState> {
        let get = |property: &str| -> Option<OwnedValue> {
            system
                .call_method(
                    Some(UPOWER),
                    DISPLAY_DEVICE,
                    Some(PROPERTIES),
                    "Get",
                    &(DEVICE_INTERFACE, property),
                )
                .ok()?
                .body()
                .deserialize()
                .ok()
        };
        let present = get("IsPresent").and_then(|v| bool::try_from(&*v).ok())?;
        if !present {
            return None;
        }
        let percentage = get("Percentage").and_then(|v| f64::try_from(&*v).ok())?;
        let state = get("State")
            .and_then(|v| u32::try_from(&*v).ok())
            .unwrap_or(0);
        Some(BatteryState {
            percentage: percentage.clamp(0.0, 100.0),
            charging: matches!(state, CHARGING | FULLY_CHARGED),
        })
    }

    /// The status items now: the Wi-Fi radio and the battery over `system` (none without it), the input source when
    /// `input` asks for it, and the line about the machine `host` names. It waits for the bus and reads files: run it
    /// on the I/O pool (`kit::task::io` with [`super::READ_DEADLINE`]), with the connection the loop ticks.
    pub fn read(
        system: Option<zbus::Connection>,
        host: Option<HostInfo>,
        input: bool,
    ) -> StatusItems {
        let system = system.map(Connection::from);
        StatusItems {
            wifi: system.as_ref().and_then(wifi),
            battery: system.as_ref().and_then(battery),
            input: input.then(current_input).flatten(),
            host: host.and_then(host_text),
        }
    }

    /// Call `due` whenever a reading of the status items is out of date, until a stream ends; returns why: a signal
    /// of the network or Bluetooth services (`cupertino_radios::watch`), a change of the power daemon's display
    /// device, the power daemon starting or stopping. `due` must not wait: it only notes that a reading is due.
    pub async fn follow(
        system: &zbus::Connection,
        due: impl Fn() + Send + Sync + 'static,
    ) -> String {
        let due = Arc::new(due);
        let radios = Arc::clone(&due);
        if let Err(e) = cupertino_radios::watch::follow(system, Arc::new(move |_| radios())).await {
            return format!("following the network: {e}");
        }
        let device = match PropertiesProxy::builder(system)
            .destination(UPOWER)
            .and_then(|b| b.path(DISPLAY_DEVICE))
        {
            Ok(builder) => builder.cache_properties(CacheProperties::No).build().await,
            Err(e) => Err(e),
        };
        let device = match device {
            Ok(device) => device,
            Err(e) => return format!("reaching the power daemon: {e}"),
        };
        let changes = match device.receive_properties_changed().await {
            Ok(changes) => changes.map(drop),
            Err(e) => return format!("subscribing to the power daemon's changes: {e}"),
        };
        let owners = match DBusProxy::new(system).await {
            Ok(bus) => match bus
                .receive_name_owner_changed_with_args(&[(0, UPOWER)])
                .await
            {
                Ok(owners) => owners.map(drop),
                Err(e) => return format!("following the power daemon's name: {e}"),
            },
            Err(e) => return format!("reaching the bus: {e}"),
        };
        let mut news = stream::or(changes, owners);
        while news.next().await.is_some() {
            due();
        }
        "the power daemon's signal streams ended".into()
    }
}
