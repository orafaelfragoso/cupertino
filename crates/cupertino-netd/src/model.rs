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

use crate::netlink::nl80211::ScanResult;
use crate::netlink::rfkill::WlanSwitches;
use crate::netlink::rtnetlink::{Link, LinkClass};
use crate::netlink::{MacAddr, Security, Ssid};

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
