//! The pure state of the network: devices, their connection state machine, Wi-Fi on or off, and
//! the scan list. No I/O: [`Model::apply`] takes an [`Input`] and returns the [`Effect`]s the
//! host must carry out.
//!
//! ```text
//! Ethernet:  Unavailable --carrier--> Configuring(Addressing) --lease--> Connected
//! Wi-Fi:     Unavailable --radio on--> Disconnected --connect--> Configuring(Authenticating)
//!            --associated--> Configuring(Addressing) --lease--> Connected
//! ```
//!
//! Wi-Fi association is the supplicant's: its events move the Wi-Fi states, and a connection it
//! makes by itself (a known network) is handled like one the user asked for.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use cupertino_netlink::nl80211::ScanResult;
use cupertino_netlink::rfkill::WlanSwitches;
use cupertino_netlink::rtnetlink::{Link, LinkClass};
use cupertino_netlink::{MacAddr, Security, Ssid};

use crate::dhcp::client::Lease;
use crate::supplicant::{ConnectFailure, KnownNetwork, SupplicantEvent};

/// How a device reaches the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Ethernet,
    Wifi,
}

impl DeviceKind {
    /// The name on the bus.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ethernet => "ethernet",
            Self::Wifi => "wifi",
        }
    }
}

/// What a device is busy with while it configures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The supplicant is associating and authenticating.
    Authenticating,
    /// The DHCP client is asking for an address.
    Addressing,
}

/// The state of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    /// Nothing can be done: no carrier, or the radio is off.
    Unavailable,
    /// Ready, with no connection.
    Disconnected,
    Configuring(Stage),
    /// Associated (Wi-Fi) and holding a lease.
    Connected,
}

impl DeviceState {
    /// The name on the bus.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Disconnected => "disconnected",
            Self::Configuring(_) => "configuring",
            Self::Connected => "connected",
        }
    }
}

/// Why a device is not connected, or is not getting an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The supplicant refused the passphrase.
    WrongPassphrase,
    /// The network did not answer or is out of range.
    NetworkNotFound,
    /// The supplicant could not connect, for another reason.
    ConnectionFailed,
    /// The network went away or dropped the device.
    ConnectionLost,
    /// No DHCP server answered. The client keeps asking.
    NoAddress,
}

impl Failure {
    /// The name on the bus.
    pub fn name(self) -> &'static str {
        match self {
            Self::WrongPassphrase => "wrong-passphrase",
            Self::NetworkNotFound => "network-not-found",
            Self::ConnectionFailed => "connection-failed",
            Self::ConnectionLost => "connection-lost",
            Self::NoAddress => "no-address",
        }
    }
}

/// A network device the daemon manages.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub ifindex: u32,
    pub name: String,
    pub kind: DeviceKind,
    pub mac: Option<MacAddr>,
    pub state: DeviceState,
    pub failure: Option<Failure>,
    /// The network a Wi-Fi device is connecting or connected to.
    pub network: Option<Ssid>,
    /// The lease in force.
    pub lease: Option<Lease>,
    /// Access points from the last scan.
    pub scan: Vec<ScanResult>,
    pub known: Vec<KnownNetwork>,
    /// The user asked for the disconnect that is under way: the supplicant's report of it is no loss.
    disconnect_requested: bool,
}

/// A network in the scan list: one entry for every SSID and security, with the strongest access point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub ssid: Ssid,
    pub security: Security,
    pub signal_dbm: Option<i32>,
    pub bssid: MacAddr,
    pub frequency_mhz: u32,
    /// The supplicant has a saved configuration for it.
    pub known: bool,
    /// The device is connected to it.
    pub connected: bool,
}

/// What the host tells the model.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A link appeared or changed. `class` says what kind of link it is.
    LinkUpdated {
        link: Link,
        class: LinkClass,
    },
    LinkRemoved {
        ifindex: u32,
    },
    /// The Wi-Fi kill switches changed.
    Switches(WlanSwitches),
    ScanResults {
        ifindex: u32,
        results: Vec<ScanResult>,
    },
    KnownNetworks {
        ifindex: u32,
        known: Vec<KnownNetwork>,
    },
    Supplicant(SupplicantEvent),
    /// The DHCP client acknowledged a lease (also on every renewal).
    LeaseBound {
        ifindex: u32,
        lease: Lease,
    },
    /// The DHCP client lost its lease.
    LeaseLost {
        ifindex: u32,
    },
    /// No DHCP server answered.
    DhcpGaveUp {
        ifindex: u32,
    },
    /// The user asked to connect to a network of the scan list.
    Connect {
        ifindex: u32,
        ssid: Ssid,
        security: Security,
    },
    Disconnect {
        ifindex: u32,
    },
    /// The user asked to switch Wi-Fi on or off. The new state arrives as [`Input::Switches`].
    SetWirelessEnabled(bool),
}

/// What the model asks the host to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    SetLinkUp {
        ifindex: u32,
        up: bool,
    },
    /// Start the DHCP client of the device (nothing if it runs).
    StartDhcp {
        ifindex: u32,
    },
    /// Stop the DHCP client of the device, releasing nothing.
    StopDhcp {
        ifindex: u32,
    },
    /// Put the lease's address and routes on the device.
    Configure {
        ifindex: u32,
        lease: Lease,
    },
    /// Take the lease's address and routes off the device.
    Deconfigure {
        ifindex: u32,
        lease: Lease,
    },
    /// Write the resolver configuration: these servers and search domain, in this order.
    WriteDns {
        servers: Vec<Ipv4Addr>,
        domain: Option<String>,
    },
    ConnectNetwork {
        ifindex: u32,
        ssid: Ssid,
        security: Security,
    },
    DisconnectNetwork {
        ifindex: u32,
    },
    /// Switch the Wi-Fi device on in the supplicant.
    PowerWifi {
        ifindex: u32,
    },
    /// Read the kernel's cached scan list and the supplicant's saved networks of the device.
    RefreshNetworks {
        ifindex: u32,
    },
    BlockWifi(bool),
}

/// The pure state.
#[derive(Debug, Default)]
pub struct Model {
    devices: BTreeMap<u32, Device>,
    switches: WlanSwitches,
    /// The DNS configuration last written, to write only on change.
    dns: Option<(Vec<Ipv4Addr>, Option<String>)>,
}

/// The route metric of a device's default route: Ethernet before Wi-Fi, as NetworkManager's
/// defaults 100 and 600 order them. approximate to those values.
pub fn route_metric(kind: DeviceKind) -> u32 {
    match kind {
        DeviceKind::Ethernet => 100,
        DeviceKind::Wifi => 600,
    }
}

/// The networks of a scan, one per SSID and security, strongest first (ties by name, then
/// security). Hidden networks have no name to show and are left out.
pub fn dedupe(results: &[ScanResult]) -> Vec<(Ssid, Security, &ScanResult)> {
    let mut best: BTreeMap<(Ssid, Security), &ScanResult> = BTreeMap::new();
    for result in results.iter().filter(|result| !result.ssid.is_hidden()) {
        let slot = best
            .entry((result.ssid.clone(), result.security))
            .or_insert(result);
        // a missing signal is weaker than any signal
        if result.signal_dbm > slot.signal_dbm {
            *slot = result;
        }
    }
    let mut networks: Vec<_> = best
        .into_iter()
        .map(|((ssid, security), result)| (ssid, security, result))
        .collect();
    networks.sort_by(|a, b| {
        b.2.signal_dbm
            .cmp(&a.2.signal_dbm)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });
    networks
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }

    pub fn device(&self, ifindex: u32) -> Option<&Device> {
        self.devices.get(&ifindex)
    }

    /// Wi-Fi is on unless software has switched it off.
    pub fn wireless_enabled(&self) -> bool {
        !self.switches.is_soft_blocked()
    }

    /// A Wi-Fi switch exists.
    pub fn wireless_present(&self) -> bool {
        self.switches.is_present()
    }

    /// The Wi-Fi device the network service controls: the first by index (the daemon serves one).
    pub fn wifi_device(&self) -> Option<&Device> {
        self.devices
            .values()
            .find(|device| device.kind == DeviceKind::Wifi)
    }

    /// The scan list of a Wi-Fi device, with saved and connected networks marked.
    pub fn networks(&self, ifindex: u32) -> Vec<Network> {
        let Some(device) = self.devices.get(&ifindex) else {
            return Vec::new();
        };
        let associated = matches!(
            device.state,
            DeviceState::Configuring(Stage::Addressing) | DeviceState::Connected
        );
        dedupe(&device.scan)
            .into_iter()
            .map(|(ssid, security, best)| Network {
                known: device
                    .known
                    .iter()
                    .any(|known| known.ssid == ssid && known.class == security.into()),
                connected: associated && device.network.as_ref() == Some(&ssid),
                ssid,
                security,
                signal_dbm: best.signal_dbm,
                bssid: best.bssid,
                frequency_mhz: best.frequency_mhz,
            })
            .collect()
    }

    /// Whether the radio and the link let a Wi-Fi device work.
    fn wifi_usable(&self) -> bool {
        !self.switches.is_soft_blocked() && !self.switches.is_hard_blocked()
    }

    /// Takes a device down to `state`: stops its DHCP client and removes its lease.
    fn tear_down(device: &mut Device, state: DeviceState, effects: &mut Vec<Effect>) {
        effects.push(Effect::StopDhcp {
            ifindex: device.ifindex,
        });
        if let Some(lease) = device.lease.take() {
            effects.push(Effect::Deconfigure {
                ifindex: device.ifindex,
                lease,
            });
        }
        device.state = state;
    }

    pub fn apply(&mut self, input: Input) -> Vec<Effect> {
        let mut effects = Vec::new();
        match input {
            Input::LinkUpdated { link, class } => self.link_updated(link, class, &mut effects),
            Input::LinkRemoved { ifindex } => {
                if self.devices.remove(&ifindex).is_some() {
                    effects.push(Effect::StopDhcp { ifindex });
                }
            }
            Input::Switches(switches) => self.switches_changed(switches, &mut effects),
            Input::ScanResults { ifindex, results } => {
                if let Some(device) = self.devices.get_mut(&ifindex) {
                    device.scan = results;
                }
            }
            Input::KnownNetworks { ifindex, known } => {
                if let Some(device) = self.devices.get_mut(&ifindex) {
                    device.known = known;
                }
            }
            Input::Supplicant(event) => self.supplicant_event(event, &mut effects),
            Input::LeaseBound { ifindex, lease } => self.lease_bound(ifindex, lease, &mut effects),
            Input::LeaseLost { ifindex } => {
                if let Some(device) = self.devices.get_mut(&ifindex) {
                    if let Some(lease) = device.lease.take() {
                        effects.push(Effect::Deconfigure { ifindex, lease });
                        device.state = DeviceState::Configuring(Stage::Addressing);
                    }
                }
            }
            Input::DhcpGaveUp { ifindex } => {
                if let Some(device) = self.devices.get_mut(&ifindex) {
                    if device.state == DeviceState::Configuring(Stage::Addressing) {
                        device.failure = Some(Failure::NoAddress);
                    }
                }
            }
            Input::Connect {
                ifindex,
                ssid,
                security,
            } => {
                let usable = self.wifi_usable();
                if let Some(device) = self
                    .devices
                    .get_mut(&ifindex)
                    .filter(|device| device.kind == DeviceKind::Wifi && usable)
                {
                    // leaving a network the device is on: the supplicant reports that on its way
                    let leaving = matches!(
                        device.state,
                        DeviceState::Configuring(Stage::Addressing) | DeviceState::Connected
                    );
                    if device.lease.is_some() || device.state != DeviceState::Disconnected {
                        Self::tear_down(device, DeviceState::Disconnected, &mut effects);
                    }
                    device.state = DeviceState::Configuring(Stage::Authenticating);
                    device.failure = None;
                    device.disconnect_requested = leaving;
                    device.network = Some(ssid.clone());
                    effects.push(Effect::ConnectNetwork {
                        ifindex,
                        ssid,
                        security,
                    });
                }
            }
            Input::Disconnect { ifindex } => {
                if let Some(device) = self
                    .devices
                    .get_mut(&ifindex)
                    .filter(|device| device.kind == DeviceKind::Wifi)
                {
                    if matches!(
                        device.state,
                        DeviceState::Configuring(_) | DeviceState::Connected
                    ) {
                        Self::tear_down(device, DeviceState::Disconnected, &mut effects);
                        device.disconnect_requested = true;
                        device.failure = None;
                        device.network = None;
                        effects.push(Effect::DisconnectNetwork { ifindex });
                    }
                }
            }
            Input::SetWirelessEnabled(enabled) => effects.push(Effect::BlockWifi(!enabled)),
        }
        self.update_dns(&mut effects);
        effects
    }

    fn link_updated(&mut self, link: Link, class: LinkClass, effects: &mut Vec<Effect>) {
        let kind = match class {
            LinkClass::Wired => DeviceKind::Ethernet,
            LinkClass::Wireless => DeviceKind::Wifi,
            LinkClass::Loopback | LinkClass::Virtual => {
                // a link that stopped being ours (a driver was replaced)
                if self.devices.remove(&link.index).is_some() {
                    effects.push(Effect::StopDhcp {
                        ifindex: link.index,
                    });
                }
                return;
            }
        };
        let usable = self.wifi_usable();
        let is_new = !self.devices.contains_key(&link.index);
        let device = self.devices.entry(link.index).or_insert_with(|| Device {
            ifindex: link.index,
            name: link.name.clone(),
            kind,
            mac: link.mac,
            state: DeviceState::Unavailable,
            failure: None,
            network: None,
            lease: None,
            scan: Vec::new(),
            known: Vec::new(),
            disconnect_requested: false,
        });
        device.name = link.name.clone();
        device.mac = link.mac;
        let ifindex = device.ifindex;
        match kind {
            DeviceKind::Ethernet => {
                if !link.flags.is_up() {
                    // an Ethernet device is ours to bring up
                    effects.push(Effect::SetLinkUp { ifindex, up: true });
                }
                if link.flags.is_up() && link.flags.has_carrier() {
                    if device.state == DeviceState::Unavailable {
                        device.state = DeviceState::Configuring(Stage::Addressing);
                        device.failure = None;
                        effects.push(Effect::StartDhcp { ifindex });
                    }
                } else if device.state != DeviceState::Unavailable {
                    Self::tear_down(device, DeviceState::Unavailable, effects);
                    device.failure = None;
                }
            }
            DeviceKind::Wifi => {
                if is_new && usable {
                    device.state = DeviceState::Disconnected;
                    effects.push(Effect::PowerWifi { ifindex });
                    effects.push(Effect::RefreshNetworks { ifindex });
                }
            }
        }
    }

    fn switches_changed(&mut self, switches: WlanSwitches, effects: &mut Vec<Effect>) {
        self.switches = switches;
        let usable = self.wifi_usable();
        for device in self
            .devices
            .values_mut()
            .filter(|device| device.kind == DeviceKind::Wifi)
        {
            let ifindex = device.ifindex;
            if !usable && device.state != DeviceState::Unavailable {
                Self::tear_down(device, DeviceState::Unavailable, effects);
                device.failure = None;
                device.network = None;
                device.scan.clear();
            } else if usable && device.state == DeviceState::Unavailable {
                device.state = DeviceState::Disconnected;
                effects.push(Effect::PowerWifi { ifindex });
                effects.push(Effect::RefreshNetworks { ifindex });
            }
        }
    }

    fn supplicant_event(&mut self, event: SupplicantEvent, effects: &mut Vec<Effect>) {
        let usable = self.wifi_usable();
        let Some(device) = self
            .devices
            .get_mut(&event.ifindex())
            .filter(|device| device.kind == DeviceKind::Wifi && usable)
        else {
            return;
        };
        let ifindex = device.ifindex;
        match event {
            SupplicantEvent::Connecting { .. } => {
                if device.lease.is_none() {
                    device.state = DeviceState::Configuring(Stage::Authenticating);
                    device.failure = None;
                    device.disconnect_requested = false;
                }
            }
            SupplicantEvent::Connected { ssid, .. } => {
                device.network = Some(ssid);
                device.disconnect_requested = false;
                // roaming keeps the lease: only a device without one has to ask
                if device.lease.is_none() {
                    device.state = DeviceState::Configuring(Stage::Addressing);
                    device.failure = None;
                    effects.push(Effect::StartDhcp { ifindex });
                }
            }
            SupplicantEvent::Disconnected { .. } => {
                if device.state == DeviceState::Configuring(Stage::Authenticating)
                    && device.disconnect_requested
                {
                    // the old network going away before the new one is joined
                    device.disconnect_requested = false;
                    return;
                }
                if matches!(
                    device.state,
                    DeviceState::Configuring(_) | DeviceState::Connected
                ) {
                    let lost = !device.disconnect_requested && device.failure.is_none();
                    Self::tear_down(device, DeviceState::Disconnected, effects);
                    device.network = None;
                    if lost {
                        device.failure = Some(Failure::ConnectionLost);
                    }
                }
                device.disconnect_requested = false;
            }
            SupplicantEvent::Failed { reason, .. } => {
                Self::tear_down(device, DeviceState::Disconnected, effects);
                device.network = None;
                device.failure = Some(match reason {
                    ConnectFailure::WrongPassphrase => Failure::WrongPassphrase,
                    ConnectFailure::NotFound => Failure::NetworkNotFound,
                    ConnectFailure::Failed => Failure::ConnectionFailed,
                });
            }
        }
    }

    fn lease_bound(&mut self, ifindex: u32, lease: Lease, effects: &mut Vec<Effect>) {
        let Some(device) = self.devices.get_mut(&ifindex) else {
            return;
        };
        if !matches!(
            device.state,
            DeviceState::Configuring(Stage::Addressing) | DeviceState::Connected
        ) {
            return;
        }
        match device.lease.replace(lease.clone()) {
            Some(old) if configuration_equal(&old, &lease) => {}
            Some(old) => {
                effects.push(Effect::Deconfigure {
                    ifindex,
                    lease: old,
                });
                effects.push(Effect::Configure { ifindex, lease });
            }
            None => effects.push(Effect::Configure { ifindex, lease }),
        }
        device.state = DeviceState::Connected;
        device.failure = None;
    }

    /// The device whose lease sets the resolvers: the connected device with the lowest metric.
    fn primary(&self) -> Option<&Device> {
        self.devices
            .values()
            .filter(|device| device.state == DeviceState::Connected && device.lease.is_some())
            .min_by_key(|device| (route_metric(device.kind), device.ifindex))
    }

    fn update_dns(&mut self, effects: &mut Vec<Effect>) {
        let wanted = self
            .primary()
            .and_then(|device| device.lease.as_ref())
            .map(|lease| (lease.dns_servers.clone(), lease.domain_name.clone()));
        if wanted != self.dns {
            // Nothing is written for "no servers yet": the file keeps the last good configuration
            // until another lease replaces it.
            if let Some((servers, domain)) = &wanted {
                effects.push(Effect::WriteDns {
                    servers: servers.clone(),
                    domain: domain.clone(),
                });
            }
            self.dns = wanted;
        }
    }
}

/// Whether two leases configure the kernel alike: only the times differ.
fn configuration_equal(a: &Lease, b: &Lease) -> bool {
    a.address == b.address && a.prefix_len == b.prefix_len && a.routers == b.routers
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_netlink::rfkill::{Event as RfkillEvent, Kind, Operation};
    use cupertino_netlink::rtnetlink::{LinkFlags, OperState};

    const IFF_UP: u32 = 0x1;
    const IFF_LOWER_UP: u32 = 0x1_0000;

    fn link(index: u32, name: &str, flags: u32) -> Link {
        Link {
            index,
            name: name.to_owned(),
            hardware_type: 1,
            flags: LinkFlags(flags),
            oper_state: OperState::Up,
            mac: Some(MacAddr([2, 0, 0, 0, 0, index as u8])),
            mtu: Some(1500),
            info_kind: None,
            master: None,
        }
    }

    fn switches(soft: bool, hard: bool) -> WlanSwitches {
        let mut switches = WlanSwitches::new();
        switches.apply(&RfkillEvent {
            index: 0,
            kind: Kind::Wlan,
            operation: Operation::Added,
            soft_blocked: soft,
            hard_blocked: hard,
        });
        switches
    }

    fn lease(address: [u8; 4], dns: [u8; 4]) -> Lease {
        Lease {
            address: Ipv4Addr::from(address),
            prefix_len: 24,
            routers: vec![Ipv4Addr::new(10, 0, 0, 1)],
            dns_servers: vec![Ipv4Addr::from(dns)],
            domain_name: None,
            server: Ipv4Addr::new(10, 0, 0, 1),
            duration: None,
        }
    }

    fn result(ssid: &str, bssid: u8, security: Security, signal: Option<i32>) -> ScanResult {
        ScanResult {
            ifindex: 3,
            bssid: MacAddr([2, 0, 0, 0, 1, bssid]),
            ssid: Ssid(ssid.as_bytes().to_vec()),
            frequency_mhz: 2412,
            signal_dbm: signal,
            security,
            associated: false,
            seen_ms_ago: None,
        }
    }

    fn state(model: &Model, ifindex: u32) -> DeviceState {
        model.device(ifindex).unwrap().state
    }

    /// A model with Ethernet device 2 and Wi-Fi device 3, radio on.
    fn model() -> Model {
        let mut model = Model::new();
        model.apply(Input::Switches(switches(false, false)));
        model.apply(Input::LinkUpdated {
            link: link(3, "wlan0", IFF_UP | IFF_LOWER_UP),
            class: LinkClass::Wireless,
        });
        model.apply(Input::LinkUpdated {
            link: link(2, "eth0", IFF_UP),
            class: LinkClass::Wired,
        });
        model
    }

    #[test]
    fn the_scan_list_has_one_entry_per_ssid_and_security_with_the_strongest_access_point() {
        let results = [
            result("home", 1, Security::Wpa2, Some(-70)),
            result("home", 2, Security::Wpa2, Some(-45)),
            result("home", 3, Security::Wpa2, None),
            result("home", 4, Security::Open, Some(-80)),
            result("cafe", 5, Security::Open, Some(-60)),
            result("", 6, Security::Wpa2, Some(-30)),
        ];
        let list: Vec<_> = dedupe(&results)
            .into_iter()
            .map(|(ssid, security, best)| {
                (ssid.to_text(), security, best.bssid.0[5], best.signal_dbm)
            })
            .collect();
        assert_eq!(
            list,
            [
                ("home".to_owned(), Security::Wpa2, 2, Some(-45)),
                ("cafe".to_owned(), Security::Open, 5, Some(-60)),
                ("home".to_owned(), Security::Open, 4, Some(-80))
            ]
        );
    }

    #[test]
    fn equal_signals_sort_by_name_and_unknown_signals_come_last() {
        let results = [
            result("b", 1, Security::Open, Some(-50)),
            result("a", 2, Security::Wpa2, Some(-50)),
            result("z", 3, Security::Open, None),
        ];
        let names: Vec<_> = dedupe(&results)
            .into_iter()
            .map(|(ssid, ..)| ssid.to_text())
            .collect();
        assert_eq!(names, ["a", "b", "z"]);
    }

    #[test]
    fn networks_are_marked_known_and_connected() {
        let mut model = model();
        model.apply(Input::ScanResults {
            ifindex: 3,
            results: vec![
                result("home", 1, Security::Wpa2, Some(-50)),
                result("cafe", 2, Security::Open, Some(-60)),
            ],
        });
        model.apply(Input::KnownNetworks {
            ifindex: 3,
            known: vec![KnownNetwork {
                ssid: Ssid(b"home".to_vec()),
                class: Security::Wpa2.into(),
                auto_connect: true,
            }],
        });
        model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        let flags: Vec<_> = model
            .networks(3)
            .into_iter()
            .map(|network| (network.ssid.to_text(), network.known, network.connected))
            .collect();
        assert_eq!(
            flags,
            [
                ("home".to_owned(), true, true),
                ("cafe".to_owned(), false, false)
            ]
        );
    }

    #[test]
    fn a_saved_wpa2_network_is_not_the_open_network_of_the_same_name() {
        let mut model = model();
        model.apply(Input::ScanResults {
            ifindex: 3,
            results: vec![result("home", 1, Security::Open, Some(-50))],
        });
        model.apply(Input::KnownNetworks {
            ifindex: 3,
            known: vec![KnownNetwork {
                ssid: Ssid(b"home".to_vec()),
                class: Security::Wpa2.into(),
                auto_connect: true,
            }],
        });
        assert!(!model.networks(3)[0].known);
    }

    #[test]
    fn a_cable_with_a_carrier_starts_dhcp_and_a_lease_connects() {
        let mut model = Model::new();
        let effects = model.apply(Input::LinkUpdated {
            link: link(2, "eth0", IFF_UP),
            class: LinkClass::Wired,
        });
        assert_eq!(effects, []);
        assert_eq!(state(&model, 2), DeviceState::Unavailable);
        let effects = model.apply(Input::LinkUpdated {
            link: link(2, "eth0", IFF_UP | IFF_LOWER_UP),
            class: LinkClass::Wired,
        });
        assert_eq!(effects, [Effect::StartDhcp { ifindex: 2 }]);
        assert_eq!(
            state(&model, 2),
            DeviceState::Configuring(Stage::Addressing)
        );

        let lease = lease([10, 0, 0, 9], [10, 0, 0, 1]);
        let effects = model.apply(Input::LeaseBound {
            ifindex: 2,
            lease: lease.clone(),
        });
        assert_eq!(
            effects,
            [
                Effect::Configure { ifindex: 2, lease },
                Effect::WriteDns {
                    servers: vec![Ipv4Addr::new(10, 0, 0, 1)],
                    domain: None
                }
            ]
        );
        assert_eq!(state(&model, 2), DeviceState::Connected);
    }

    #[test]
    fn a_link_that_is_down_is_brought_up() {
        let mut model = Model::new();
        let effects = model.apply(Input::LinkUpdated {
            link: link(2, "eth0", 0),
            class: LinkClass::Wired,
        });
        assert_eq!(
            effects,
            [Effect::SetLinkUp {
                ifindex: 2,
                up: true
            }]
        );
    }

    #[test]
    fn unplugging_the_cable_removes_the_lease() {
        let mut model = Model::new();
        model.apply(Input::LinkUpdated {
            link: link(2, "eth0", IFF_UP | IFF_LOWER_UP),
            class: LinkClass::Wired,
        });
        let lease = lease([10, 0, 0, 9], [10, 0, 0, 1]);
        model.apply(Input::LeaseBound {
            ifindex: 2,
            lease: lease.clone(),
        });
        let effects = model.apply(Input::LinkUpdated {
            link: link(2, "eth0", IFF_UP),
            class: LinkClass::Wired,
        });
        assert_eq!(
            effects,
            [
                Effect::StopDhcp { ifindex: 2 },
                Effect::Deconfigure { ifindex: 2, lease }
            ]
        );
        assert_eq!(state(&model, 2), DeviceState::Unavailable);
    }

    #[test]
    fn a_renewal_with_the_same_configuration_changes_nothing_and_a_new_address_replaces_the_old() {
        let mut model = Model::new();
        model.apply(Input::LinkUpdated {
            link: link(2, "eth0", IFF_UP | IFF_LOWER_UP),
            class: LinkClass::Wired,
        });
        let first = lease([10, 0, 0, 9], [10, 0, 0, 1]);
        model.apply(Input::LeaseBound {
            ifindex: 2,
            lease: first.clone(),
        });
        let mut renewed = first.clone();
        renewed.duration = Some(std::time::Duration::from_secs(60));
        assert_eq!(
            model.apply(Input::LeaseBound {
                ifindex: 2,
                lease: renewed
            }),
            []
        );
        let moved = lease([10, 0, 0, 10], [10, 0, 0, 1]);
        let effects = model.apply(Input::LeaseBound {
            ifindex: 2,
            lease: moved.clone(),
        });
        assert_eq!(
            effects,
            [
                Effect::Deconfigure {
                    ifindex: 2,
                    lease: renewed_of(&first)
                },
                Effect::Configure {
                    ifindex: 2,
                    lease: moved
                }
            ]
        );
    }

    /// The first lease as the model stored it after the renewal (times differ, address the same).
    fn renewed_of(lease: &Lease) -> Lease {
        Lease {
            duration: Some(std::time::Duration::from_secs(60)),
            ..lease.clone()
        }
    }

    #[test]
    fn dns_follows_the_connected_device_with_the_lowest_metric() {
        let mut model = model();
        model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        let wifi_effects = model.apply(Input::LeaseBound {
            ifindex: 3,
            lease: lease([10, 0, 1, 9], [10, 0, 1, 1]),
        });
        assert!(wifi_effects.contains(&Effect::WriteDns {
            servers: vec![Ipv4Addr::new(10, 0, 1, 1)],
            domain: None
        }));
        model.apply(Input::LinkUpdated {
            link: link(2, "eth0", IFF_UP | IFF_LOWER_UP),
            class: LinkClass::Wired,
        });
        let wired_effects = model.apply(Input::LeaseBound {
            ifindex: 2,
            lease: lease([10, 0, 0, 9], [10, 0, 0, 1]),
        });
        assert!(wired_effects.contains(&Effect::WriteDns {
            servers: vec![Ipv4Addr::new(10, 0, 0, 1)],
            domain: None
        }));
    }

    #[test]
    fn the_radio_switch_takes_wifi_down_and_brings_it_back() {
        let mut model = model();
        assert_eq!(state(&model, 3), DeviceState::Disconnected);
        assert_eq!(
            model.apply(Input::SetWirelessEnabled(false)),
            [Effect::BlockWifi(true)]
        );
        // the model changes when the kernel reports the switch, not before
        assert!(model.wireless_enabled());
        model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        let lease = lease([10, 0, 1, 9], [10, 0, 1, 1]);
        model.apply(Input::LeaseBound {
            ifindex: 3,
            lease: lease.clone(),
        });

        let effects = model.apply(Input::Switches(switches(true, false)));
        assert_eq!(
            effects,
            [
                Effect::StopDhcp { ifindex: 3 },
                Effect::Deconfigure { ifindex: 3, lease }
            ]
        );
        assert!(!model.wireless_enabled());
        assert_eq!(state(&model, 3), DeviceState::Unavailable);
        assert_eq!(model.device(3).unwrap().network, None);

        assert_eq!(
            model.apply(Input::SetWirelessEnabled(true)),
            [Effect::BlockWifi(false)]
        );
        let effects = model.apply(Input::Switches(switches(false, false)));
        assert_eq!(
            effects,
            [
                Effect::PowerWifi { ifindex: 3 },
                Effect::RefreshNetworks { ifindex: 3 }
            ]
        );
        assert!(model.wireless_enabled());
        assert_eq!(state(&model, 3), DeviceState::Disconnected);
    }

    #[test]
    fn a_hardware_block_makes_wifi_unavailable_while_the_setting_stays_on() {
        let mut model = model();
        model.apply(Input::Switches(switches(false, true)));
        assert!(model.wireless_enabled());
        assert_eq!(state(&model, 3), DeviceState::Unavailable);
    }

    #[test]
    fn a_wifi_device_that_appears_while_the_radio_is_off_is_unavailable() {
        let mut model = Model::new();
        model.apply(Input::Switches(switches(true, false)));
        let effects = model.apply(Input::LinkUpdated {
            link: link(3, "wlan0", IFF_UP),
            class: LinkClass::Wireless,
        });
        assert_eq!(effects, []);
        assert_eq!(state(&model, 3), DeviceState::Unavailable);
    }

    #[test]
    fn connecting_goes_through_authenticating_and_addressing_to_connected() {
        let mut model = model();
        let ssid = Ssid(b"home".to_vec());
        let effects = model.apply(Input::Connect {
            ifindex: 3,
            ssid: ssid.clone(),
            security: Security::Wpa2,
        });
        assert_eq!(
            effects,
            [Effect::ConnectNetwork {
                ifindex: 3,
                ssid: ssid.clone(),
                security: Security::Wpa2
            }]
        );
        assert_eq!(
            state(&model, 3),
            DeviceState::Configuring(Stage::Authenticating)
        );
        let effects = model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: ssid.clone(),
        }));
        assert_eq!(effects, [Effect::StartDhcp { ifindex: 3 }]);
        assert_eq!(
            state(&model, 3),
            DeviceState::Configuring(Stage::Addressing)
        );
        model.apply(Input::LeaseBound {
            ifindex: 3,
            lease: lease([10, 0, 1, 9], [10, 0, 1, 1]),
        });
        assert_eq!(state(&model, 3), DeviceState::Connected);
        assert_eq!(model.device(3).unwrap().failure, None);
    }

    #[test]
    fn a_wrong_passphrase_ends_disconnected_with_the_reason() {
        let mut model = model();
        let ssid = Ssid(b"home".to_vec());
        model.apply(Input::Connect {
            ifindex: 3,
            ssid,
            security: Security::Wpa2,
        });
        model.apply(Input::Supplicant(SupplicantEvent::Failed {
            ifindex: 3,
            reason: ConnectFailure::WrongPassphrase,
        }));
        let device = model.device(3).unwrap();
        assert_eq!(
            (device.state, device.failure, device.network.clone()),
            (
                DeviceState::Disconnected,
                Some(Failure::WrongPassphrase),
                None
            )
        );
        // a new attempt clears the reason
        model.apply(Input::Connect {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
            security: Security::Wpa2,
        });
        assert_eq!(model.device(3).unwrap().failure, None);
    }

    #[test]
    fn losing_the_access_point_while_connected_removes_the_lease_and_records_the_loss() {
        let mut model = model();
        model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        let lease = lease([10, 0, 1, 9], [10, 0, 1, 1]);
        model.apply(Input::LeaseBound {
            ifindex: 3,
            lease: lease.clone(),
        });
        let effects = model.apply(Input::Supplicant(SupplicantEvent::Disconnected {
            ifindex: 3,
        }));
        assert_eq!(
            effects,
            [
                Effect::StopDhcp { ifindex: 3 },
                Effect::Deconfigure { ifindex: 3, lease }
            ]
        );
        let device = model.device(3).unwrap();
        assert_eq!(
            (device.state, device.failure),
            (DeviceState::Disconnected, Some(Failure::ConnectionLost))
        );
    }

    #[test]
    fn a_disconnect_the_user_asked_for_is_no_loss() {
        let mut model = model();
        model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        let effects = model.apply(Input::Disconnect { ifindex: 3 });
        assert_eq!(
            effects,
            [
                Effect::StopDhcp { ifindex: 3 },
                Effect::DisconnectNetwork { ifindex: 3 }
            ]
        );
        model.apply(Input::Supplicant(SupplicantEvent::Disconnected {
            ifindex: 3,
        }));
        let device = model.device(3).unwrap();
        assert_eq!(
            (device.state, device.failure),
            (DeviceState::Disconnected, None)
        );
    }

    #[test]
    fn a_failed_attempt_followed_by_the_disconnect_report_keeps_the_first_reason() {
        let mut model = model();
        model.apply(Input::Connect {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
            security: Security::Wpa2,
        });
        model.apply(Input::Supplicant(SupplicantEvent::Failed {
            ifindex: 3,
            reason: ConnectFailure::NotFound,
        }));
        model.apply(Input::Supplicant(SupplicantEvent::Disconnected {
            ifindex: 3,
        }));
        assert_eq!(
            model.device(3).unwrap().failure,
            Some(Failure::NetworkNotFound)
        );
    }

    #[test]
    fn a_known_network_the_supplicant_joins_by_itself_connects_like_any_other() {
        let mut model = model();
        model.apply(Input::Supplicant(SupplicantEvent::Connecting {
            ifindex: 3,
        }));
        assert_eq!(
            state(&model, 3),
            DeviceState::Configuring(Stage::Authenticating)
        );
        let effects = model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        assert_eq!(effects, [Effect::StartDhcp { ifindex: 3 }]);
    }

    #[test]
    fn switching_networks_does_not_count_the_old_one_going_away_as_a_loss() {
        let mut model = model();
        model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        let lease = lease([10, 0, 1, 9], [10, 0, 1, 1]);
        model.apply(Input::LeaseBound {
            ifindex: 3,
            lease: lease.clone(),
        });
        let effects = model.apply(Input::Connect {
            ifindex: 3,
            ssid: Ssid(b"cafe".to_vec()),
            security: Security::Open,
        });
        assert_eq!(
            effects,
            [
                Effect::StopDhcp { ifindex: 3 },
                Effect::Deconfigure { ifindex: 3, lease },
                Effect::ConnectNetwork {
                    ifindex: 3,
                    ssid: Ssid(b"cafe".to_vec()),
                    security: Security::Open
                }
            ]
        );
        model.apply(Input::Supplicant(SupplicantEvent::Disconnected {
            ifindex: 3,
        }));
        let device = model.device(3).unwrap();
        assert_eq!(
            (device.state, device.failure),
            (DeviceState::Configuring(Stage::Authenticating), None)
        );
    }

    #[test]
    fn a_saved_network_the_supplicant_gives_up_on_returns_the_device_to_disconnected() {
        let mut model = model();
        model.apply(Input::Supplicant(SupplicantEvent::Connecting {
            ifindex: 3,
        }));
        model.apply(Input::Supplicant(SupplicantEvent::Disconnected {
            ifindex: 3,
        }));
        let device = model.device(3).unwrap();
        assert_eq!(
            (device.state, device.failure),
            (DeviceState::Disconnected, Some(Failure::ConnectionLost))
        );
    }

    #[test]
    fn roaming_between_access_points_keeps_the_lease() {
        let mut model = model();
        model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        model.apply(Input::LeaseBound {
            ifindex: 3,
            lease: lease([10, 0, 1, 9], [10, 0, 1, 1]),
        });
        assert_eq!(
            model.apply(Input::Supplicant(SupplicantEvent::Connected {
                ifindex: 3,
                ssid: Ssid(b"home".to_vec())
            })),
            []
        );
        assert_eq!(state(&model, 3), DeviceState::Connected);
    }

    #[test]
    fn no_dhcp_answer_is_a_failure_that_clears_when_a_lease_arrives() {
        let mut model = model();
        model.apply(Input::Supplicant(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        }));
        model.apply(Input::DhcpGaveUp { ifindex: 3 });
        let device = model.device(3).unwrap();
        assert_eq!(
            (device.state, device.failure),
            (
                DeviceState::Configuring(Stage::Addressing),
                Some(Failure::NoAddress)
            )
        );
        model.apply(Input::LeaseBound {
            ifindex: 3,
            lease: lease([10, 0, 1, 9], [10, 0, 1, 1]),
        });
        assert_eq!(model.device(3).unwrap().failure, None);
    }

    #[test]
    fn a_lease_that_arrives_for_a_device_that_is_not_configuring_is_ignored() {
        let mut model = model();
        assert_eq!(
            model.apply(Input::LeaseBound {
                ifindex: 3,
                lease: lease([10, 0, 1, 9], [10, 0, 1, 1])
            }),
            []
        );
        assert_eq!(state(&model, 3), DeviceState::Disconnected);
    }

    #[test]
    fn connecting_while_the_radio_is_off_does_nothing() {
        let mut model = model();
        model.apply(Input::Switches(switches(true, false)));
        assert_eq!(
            model.apply(Input::Connect {
                ifindex: 3,
                ssid: Ssid(b"home".to_vec()),
                security: Security::Wpa2
            }),
            []
        );
    }
}
