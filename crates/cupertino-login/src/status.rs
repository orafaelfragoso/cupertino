//! What the status items at the top right of the lock and login screens show: the Wi-Fi radio, the battery, the
//! input source and, on the login screen, a line about the machine. [`poll`] reads them on a thread of its own every
//! [`POLL_INTERVAL`]; the views draw what the last reading said.

use std::time::Duration;

use cupertino_defaults::login::HostInfo;

/// How often the status items are read again.
pub const POLL_INTERVAL: Duration = Duration::from_secs(10);

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
/// (`XKB_DEFAULT_LAYOUT`), else "us" (approximate: the source in use is the server's to switch).
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
/// that is not a loopback one.
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
    let mut buf = [0u8; 256];
    // Safety: the length passed is the buffer's; the name is nul terminated within it on success.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let end = buf.iter().position(|&b| b == 0)?;
    (ok && end > 0).then(|| String::from_utf8_lossy(&buf[..end]).into_owned())
}

fn first_address() -> Option<String> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // Safety: `getifaddrs` fills `list`, which is freed below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return None;
    }
    let mut found = None;
    let mut at = list;
    while !at.is_null() {
        // Safety: `at` walks the list `getifaddrs` gave, each entry valid until it is freed.
        let entry = unsafe { &*at };
        at = entry.ifa_next;
        if entry.ifa_addr.is_null() {
            continue;
        }
        // Safety: the address is valid; its family says how to read it.
        let family = unsafe { (*entry.ifa_addr).sa_family };
        if i32::from(family) != libc::AF_INET {
            continue;
        }
        // Safety: an AF_INET address is a `sockaddr_in`.
        let address = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
        let ip = std::net::Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
        if !ip.is_loopback() {
            found = Some(ip.to_string());
            break;
        }
    }
    // Safety: `list` came from `getifaddrs` and is freed once.
    unsafe { libc::freeifaddrs(list) };
    found
}

#[cfg(feature = "dbus")]
mod bus {
    use super::{signal_bars, BatteryState, WifiState};
    use zbus::blocking::Connection;
    use zbus::zvariant::OwnedValue;

    const UPOWER: &str = "org.freedesktop.UPower";
    const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
    const DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";
    const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
    /// UPower's device states that mean power comes in: charging and fully charged.
    const CHARGING: u32 = 1;
    const FULLY_CHARGED: u32 = 4;

    /// The Wi-Fi radio now, through whichever service serves the network; `None` without one or a Wi-Fi device.
    pub fn wifi(system: &Connection) -> Option<WifiState> {
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
    pub fn battery(system: &Connection) -> Option<BatteryState> {
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
}

/// Read the status items now and every [`POLL_INTERVAL`] on a thread of its own, the Wi-Fi radio and the battery over
/// the system bus, and hand each reading to `send`; the thread ends when `send` says the screen is gone. `host` is the
/// line about the machine to show, if any; `input` whether the input source shows.
#[cfg(feature = "dbus")]
pub fn poll(
    host: Option<HostInfo>,
    input: bool,
    send: impl Fn(StatusItems) -> bool + Send + 'static,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("status items".into())
        .spawn(move || {
            let system = zbus::blocking::Connection::system().ok();
            loop {
                let items = StatusItems {
                    wifi: system.as_ref().and_then(bus::wifi),
                    battery: system.as_ref().and_then(bus::battery),
                    input: input.then(current_input).flatten(),
                    host: host.and_then(host_text),
                };
                if !send(items) {
                    return;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        })
        .map(|_| ())
}
