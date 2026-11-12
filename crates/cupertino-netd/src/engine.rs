//! The engine joins the pure [`Model`] to the world: it turns kernel, rfkill, supplicant and DHCP
//! events into model inputs and carries out the effects the model returns.
//!
//! It does no polling of its own and reads no clock. The host loop (`main`) waits on the
//! descriptors, calls the `on_*` method that matches, and passes the monotonic time. A failure of
//! one effect never stops the others: it is kept and returned by [`Engine::take_errors`], and
//! the model moves to the state that matches ("no address", "connection failed").

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::os::fd::BorrowedFd;
use std::path::PathBuf;

use cupertino_netlink::nl80211::{Event as WirelessEvent, ScanDump};
use cupertino_netlink::rfkill::{Event as RfkillEvent, WlanSwitches};
use cupertino_netlink::rtnetlink::{
    AddressChange, Family, Link, RouteChange, RouteEvent, PROTOCOL_DHCP,
};
use cupertino_netlink::{Security, Ssid};

use crate::dhcp::client::{Action, Client, ClientConfig, Timestamp};
use crate::dhcp::message::Message;
use crate::dhcp::transport::Transport;
use crate::dns;
use crate::error::{Error, Result};
use crate::model::{route_metric, Device, DeviceKind, DeviceState, Effect, Input, Model};
use crate::snapshot::Snapshot;
use crate::supplicant::{ConnectFailure, Interface, NetworkClass, Supplicant, SupplicantEvent};

/// What the engine asks of the machine. [`crate::host::SystemHost`] is the real one.
pub trait Host {
    fn links(&mut self) -> Result<Vec<Link>>;
    /// The interface indexes nl80211 lists.
    fn wireless_indexes(&mut self) -> Result<Vec<u32>>;
    /// The kernel's current list of access points of a Wi-Fi device.
    fn scan_results(&mut self, ifindex: u32) -> Result<ScanDump>;
    fn set_link_up(&mut self, ifindex: u32, up: bool) -> Result<()>;
    fn add_address(&mut self, change: &AddressChange) -> Result<()>;
    fn remove_address(&mut self, change: &AddressChange) -> Result<()>;
    fn add_route(&mut self, change: &RouteChange) -> Result<()>;
    fn remove_route(&mut self, change: &RouteChange) -> Result<()>;
    /// Sets the software switch of every Wi-Fi radio.
    fn set_wireless_blocked(&mut self, blocked: bool) -> Result<()>;
    /// A transport for the DHCP client of a device.
    fn open_transport(&mut self, ifindex: u32) -> Result<Box<dyn Transport>>;
    /// A random number for the first DHCP transaction id.
    fn random_u32(&mut self) -> Result<u32>;
}

/// A request from the bus, after the caller has been authorised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SetWirelessEnabled(bool),
    /// Scan on the Wi-Fi device.
    Scan,
    /// Connect to the network named `ssid`. `passphrase` is for a network that has no saved
    /// credentials.
    Connect {
        ssid: String,
        passphrase: Option<String>,
    },
    Disconnect,
    /// Take the saved credentials of the network named `ssid` away.
    Forget(String),
    /// Let the supplicant join the saved network named `ssid` by itself, or stop it.
    SetAutoJoin {
        ssid: String,
        on: bool,
    },
}

/// Why a [`Command`] was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    /// The machine has no Wi-Fi device.
    NoWifiDevice,
    /// The radio is off or blocked.
    WifiUnavailable,
    /// The last scan did not list the network.
    UnknownNetwork(String),
    /// The network needs a passphrase and none is saved or given.
    PassphraseRequired(String),
    /// The stack cannot connect to a network with this security.
    UnsupportedSecurity(Security),
    /// Carrying the command out failed.
    Failed(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoWifiDevice => write!(f, "this machine has no Wi-Fi device"),
            Self::WifiUnavailable => write!(f, "Wi-Fi is switched off or blocked"),
            Self::UnknownNetwork(ssid) => {
                write!(f, "the last scan did not list a network named {ssid:?}")
            }
            Self::PassphraseRequired(ssid) => write!(f, "{ssid:?} needs a passphrase"),
            Self::UnsupportedSecurity(security) => write!(
                f,
                "connecting to a network with {security:?} security is not supported"
            ),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// The DHCP client of a device and the way it talks.
struct Runner {
    client: Client,
    transport: Box<dyn Transport>,
}

pub struct Engine<H: Host, S: Supplicant> {
    host: H,
    supplicant: S,
    model: Model,
    runners: BTreeMap<u32, Runner>,
    /// The Wi-Fi interface indexes, as of the last read.
    wireless: Vec<u32>,
    switches: WlanSwitches,
    resolv_path: PathBuf,
    /// The passphrase of the connection being started: kept for the one call that needs it.
    passphrase: Option<String>,
    errors: Vec<Error>,
}

impl<H: Host, S: Supplicant> Engine<H, S> {
    pub fn new(host: H, supplicant: S, resolv_path: PathBuf) -> Self {
        Self {
            host,
            supplicant,
            model: Model::new(),
            runners: BTreeMap::new(),
            wireless: Vec::new(),
            switches: WlanSwitches::new(),
            resolv_path,
            passphrase: None,
            errors: Vec::new(),
        }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn supplicant(&self) -> &S {
        &self.supplicant
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot::of(&self.model)
    }

    /// The failures since the last call.
    pub fn take_errors(&mut self) -> Vec<Error> {
        std::mem::take(&mut self.errors)
    }

    /// The descriptors of the DHCP sockets, by interface index.
    pub fn dhcp_fds(&self) -> Vec<(u32, BorrowedFd<'_>)> {
        self.runners
            .iter()
            .filter_map(|(ifindex, runner)| runner.transport.as_fd().map(|fd| (*ifindex, fd)))
            .collect()
    }

    /// When [`Engine::on_timer`] next has something to do.
    pub fn next_deadline(&self) -> Option<Timestamp> {
        self.runners
            .values()
            .filter_map(|runner| runner.client.next_deadline())
            .min()
    }

    /// Reads the links, and the Wi-Fi radio state is given by [`Engine::on_rfkill`]. Call at
    /// start and again when the kernel reports it dropped events.
    pub fn resync(&mut self, now: Timestamp) {
        let result = self.read_links();
        let mut inputs = VecDeque::new();
        match result {
            Ok(links) => {
                let present: Vec<u32> = links.iter().map(|link| link.index).collect();
                for gone in self
                    .model
                    .devices()
                    .map(|device| device.ifindex)
                    .filter(|ifindex| !present.contains(ifindex))
                    .collect::<Vec<_>>()
                {
                    inputs.push_back(Input::LinkRemoved { ifindex: gone });
                }
                for link in links {
                    let class = link.class(&self.wireless);
                    inputs.push_back(Input::LinkUpdated { link, class });
                }
            }
            Err(error) => self.errors.push(error),
        }
        self.run(now, inputs);
    }

    fn read_links(&mut self) -> Result<Vec<Link>> {
        self.wireless = self.host.wireless_indexes()?;
        self.host.links()
    }

    pub fn on_route_events(&mut self, now: Timestamp, events: Vec<RouteEvent>) {
        let mut inputs = VecDeque::new();
        for event in events {
            match event {
                RouteEvent::LinkNew(link) => {
                    // a Wi-Fi interface that has just appeared is not in the list yet
                    if !self.wireless.contains(&link.index)
                        && self.model.device(link.index).is_none()
                    {
                        match self.host.wireless_indexes() {
                            Ok(indexes) => self.wireless = indexes,
                            Err(error) => self.errors.push(error),
                        }
                    }
                    let class = link.class(&self.wireless);
                    inputs.push_back(Input::LinkUpdated { link, class });
                }
                RouteEvent::LinkDeleted(link) => inputs.push_back(Input::LinkRemoved {
                    ifindex: link.index,
                }),
                // The kernel's addresses and routes are not the model's: the model knows what the
                // DHCP client configured.
                RouteEvent::AddressNew(_)
                | RouteEvent::AddressDeleted(_)
                | RouteEvent::RouteNew(_)
                | RouteEvent::RouteDeleted(_) => {}
            }
        }
        self.run(now, inputs);
    }

    /// A scan ended: read the kernel's list. The supplicant's own state changes arrive through
    /// [`Engine::on_supplicant`].
    pub fn on_wireless_events(&mut self, now: Timestamp, events: Vec<WirelessEvent>) {
        let mut inputs = VecDeque::new();
        for event in events {
            if let WirelessEvent::ScanResultsReady { ifindex } = event {
                if self.model.device(ifindex).is_some() {
                    self.refresh_scan(ifindex, &mut inputs);
                }
            }
        }
        self.run(now, inputs);
    }

    /// Records rfkill events: the Wi-Fi switches change when the kernel says so.
    pub fn on_rfkill(&mut self, now: Timestamp, events: &[RfkillEvent]) {
        for event in events {
            self.switches.apply(event);
        }
        self.run(
            now,
            VecDeque::from([Input::Switches(self.switches.clone())]),
        );
    }

    pub fn on_supplicant(&mut self, now: Timestamp) {
        let mut inputs = VecDeque::new();
        for event in self.supplicant.events() {
            match event {
                Ok(event) => inputs.push_back(Input::Supplicant(event)),
                Err(error) => self.errors.push(error),
            }
        }
        self.run(now, inputs);
    }

    /// The DHCP socket of `ifindex` is readable.
    pub fn on_dhcp_readable(&mut self, now: Timestamp, ifindex: u32) {
        let mut inputs = VecDeque::new();
        // A runner outlives its device only until the removal stops it; name the index then.
        let name = self.model.device(ifindex).map_or_else(
            || format!("interface {ifindex}"),
            |device| device.name.clone(),
        );
        while let Some(runner) = self.runners.get_mut(&ifindex) {
            let bytes = match runner.transport.receive() {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(error) => {
                    self.errors.push(error);
                    break;
                }
            };
            match Message::decode(&bytes) {
                Ok(message) => {
                    let actions = runner.client.receive(now, &message);
                    self.act(ifindex, actions, &mut inputs);
                }
                Err(source) => self.errors.push(Error::Dhcp {
                    interface: name.clone(),
                    source,
                }),
            }
        }
        self.run(now, inputs);
    }

    /// Runs the DHCP timers that are due.
    pub fn on_timer(&mut self, now: Timestamp) {
        let mut inputs = VecDeque::new();
        for ifindex in self.runners.keys().copied().collect::<Vec<_>>() {
            let Some(runner) = self.runners.get_mut(&ifindex) else {
                continue;
            };
            let actions = runner.client.tick(now);
            self.act(ifindex, actions, &mut inputs);
        }
        self.run(now, inputs);
    }

    /// Carries out a request from the bus. The command is refused before it changes anything when
    /// it cannot work; a failure while carrying it out comes back as [`CommandError::Failed`].
    pub fn command(
        &mut self,
        now: Timestamp,
        command: Command,
    ) -> std::result::Result<(), CommandError> {
        let input = match command {
            Command::SetWirelessEnabled(enabled) => Input::SetWirelessEnabled(enabled),
            Command::Scan => {
                let interface = self.usable_wifi()?;
                return self
                    .supplicant
                    .scan(Interface {
                        index: interface.0,
                        name: &interface.1,
                    })
                    .map_err(|error| CommandError::Failed(error.to_string()));
            }
            Command::Disconnect => Input::Disconnect {
                ifindex: self.usable_wifi()?.0,
            },
            Command::Forget(ssid) => return self.change_saved(now, ssid, None),
            Command::SetAutoJoin { ssid, on } => return self.change_saved(now, ssid, Some(on)),
            Command::Connect { ssid, passphrase } => {
                let (ifindex, _) = self.usable_wifi()?;
                let wanted = Ssid(ssid.clone().into_bytes());
                let network = self
                    .model
                    .networks(ifindex)
                    .into_iter()
                    .find(|network| network.ssid == wanted)
                    .ok_or_else(|| CommandError::UnknownNetwork(ssid.clone()))?;
                let passphrase = passphrase.filter(|passphrase| !passphrase.is_empty());
                match NetworkClass::from(network.security) {
                    NetworkClass::Open => self.passphrase = None,
                    NetworkClass::Psk if network.known || passphrase.is_some() => {
                        self.passphrase = passphrase
                    }
                    NetworkClass::Psk => return Err(CommandError::PassphraseRequired(ssid)),
                    // a saved enterprise profile connects without anything from us
                    NetworkClass::Enterprise if network.known => self.passphrase = None,
                    NetworkClass::Wep | NetworkClass::Enterprise | NetworkClass::Unsupported => {
                        return Err(CommandError::UnsupportedSecurity(network.security))
                    }
                }
                Input::Connect {
                    ifindex,
                    ssid: network.ssid,
                    security: network.security,
                }
            }
        };
        let errors_before = self.errors.len();
        self.run(now, VecDeque::from([input]));
        // a failure of this command is the caller's to hear about; earlier ones stay in the list
        match self.errors.get(errors_before) {
            Some(error) => Err(CommandError::Failed(error.to_string())),
            None => Ok(()),
        }
    }

    /// Forgets (`auto_join` none) or sets the auto-join of every saved profile of the network named `ssid`, then reads
    /// the saved list again.
    fn change_saved(
        &mut self,
        now: Timestamp,
        ssid: String,
        auto_join: Option<bool>,
    ) -> std::result::Result<(), CommandError> {
        let (ifindex, name) = self.usable_wifi()?;
        let wanted = Ssid(ssid.clone().into_bytes());
        let profiles: Vec<NetworkClass> = self
            .model
            .device(ifindex)
            .map(|device| {
                device
                    .known
                    .iter()
                    .filter(|known| known.ssid == wanted)
                    .map(|known| known.class)
                    .collect()
            })
            .unwrap_or_default();
        if profiles.is_empty() {
            return Err(CommandError::UnknownNetwork(ssid));
        }
        for class in profiles {
            match auto_join {
                None => self.supplicant.forget(&wanted, class),
                Some(on) => self.supplicant.set_auto_connect(&wanted, class, on),
            }
            .map_err(|error| CommandError::Failed(error.to_string()))?;
        }
        let known = self
            .supplicant
            .known_networks(Interface {
                index: ifindex,
                name: &name,
            })
            .map_err(|error| CommandError::Failed(error.to_string()))?;
        self.run(
            now,
            VecDeque::from([Input::KnownNetworks { ifindex, known }]),
        );
        Ok(())
    }

    /// The Wi-Fi device (index and name) if it can work.
    fn usable_wifi(&self) -> std::result::Result<(u32, String), CommandError> {
        let device = self.model.wifi_device().ok_or(CommandError::NoWifiDevice)?;
        if device.state == DeviceState::Unavailable {
            return Err(CommandError::WifiUnavailable);
        }
        Ok((device.ifindex, device.name.clone()))
    }

    /// Applies inputs and carries out the effects, until nothing is left to do.
    fn run(&mut self, now: Timestamp, mut queue: VecDeque<Input>) {
        while let Some(input) = queue.pop_front() {
            for effect in self.model.apply(input) {
                if let Err(error) = self.execute(now, effect, &mut queue) {
                    self.errors.push(error);
                }
            }
        }
    }

    fn name_of(&self, ifindex: u32) -> Result<String> {
        self.model
            .device(ifindex)
            .map(|device| device.name.clone())
            .ok_or_else(|| Error::SupplicantDevice(format!("interface {ifindex}")))
    }

    fn execute(
        &mut self,
        now: Timestamp,
        effect: Effect,
        queue: &mut VecDeque<Input>,
    ) -> Result<()> {
        match effect {
            Effect::SetLinkUp { ifindex, up } => self.host.set_link_up(ifindex, up),
            Effect::StartDhcp { ifindex } => {
                let result = self.start_dhcp(now, ifindex, queue);
                if result.is_err() {
                    // without a client the device cannot get an address: say so
                    queue.push_back(Input::DhcpGaveUp { ifindex });
                }
                result
            }
            Effect::StopDhcp { ifindex } => {
                self.runners.remove(&ifindex);
                Ok(())
            }
            Effect::Configure { ifindex, lease } => {
                let change = AddressChange {
                    index: ifindex,
                    address: IpAddr::V4(lease.address),
                    prefix_len: lease.prefix_len,
                };
                let kind = self
                    .model
                    .device(ifindex)
                    .ok_or(Error::UnknownDevice(ifindex))?
                    .kind;
                self.host.add_address(&change)?;
                // Only the first router: a second default route on the same device and metric
                // would never be used.
                match lease.routers.first() {
                    Some(router) => self.host.add_route(&default_route(ifindex, *router, kind)),
                    None => Ok(()),
                }
            }
            Effect::Deconfigure { ifindex, lease } => {
                let kind = self
                    .model
                    .device(ifindex)
                    .ok_or(Error::UnknownDevice(ifindex))?
                    .kind;
                let route = match lease.routers.first() {
                    Some(router) => self
                        .host
                        .remove_route(&default_route(ifindex, *router, kind)),
                    None => Ok(()),
                };
                let address = self.host.remove_address(&AddressChange {
                    index: ifindex,
                    address: IpAddr::V4(lease.address),
                    prefix_len: lease.prefix_len,
                });
                route.and(address)
            }
            Effect::WriteDns { servers, domain } => {
                dns::write(&self.resolv_path, &servers, domain.as_deref())
            }
            Effect::ConnectNetwork {
                ifindex,
                ssid,
                security,
            } => {
                let name = self.name_of(ifindex)?;
                let passphrase = self.passphrase.take();
                let result = self.supplicant.connect(
                    Interface {
                        index: ifindex,
                        name: &name,
                    },
                    &ssid,
                    security.into(),
                    passphrase.as_deref(),
                );
                if result.is_err() {
                    // the attempt never began: the device must not wait for it
                    queue.push_back(Input::Supplicant(SupplicantEvent::Failed {
                        ifindex,
                        reason: ConnectFailure::Failed,
                    }));
                }
                result
            }
            Effect::DisconnectNetwork { ifindex } => {
                let name = self.name_of(ifindex)?;
                self.supplicant.disconnect(Interface {
                    index: ifindex,
                    name: &name,
                })
            }
            Effect::PowerWifi { ifindex } => {
                let name = self.name_of(ifindex)?;
                self.supplicant.power_on(Interface {
                    index: ifindex,
                    name: &name,
                })
            }
            Effect::RefreshNetworks { ifindex } => {
                // the two reads are independent: one failing does not hide the other
                let name = self.name_of(ifindex)?;
                self.refresh_scan(ifindex, queue);
                match self.supplicant.known_networks(Interface {
                    index: ifindex,
                    name: &name,
                }) {
                    Ok(known) => queue.push_back(Input::KnownNetworks { ifindex, known }),
                    Err(error) => self.errors.push(error),
                }
                Ok(())
            }
            Effect::BlockWifi(blocked) => self.host.set_wireless_blocked(blocked),
        }
    }

    fn refresh_scan(&mut self, ifindex: u32, queue: &mut VecDeque<Input>) {
        match self.host.scan_results(ifindex) {
            Ok(dump) => {
                // the access points that did decode still count
                self.errors.extend(dump.errors.into_iter().map(|source| {
                    Error::Netlink(cupertino_netlink::Error::Parse {
                        origin: "scan results",
                        source,
                    })
                }));
                queue.push_back(Input::ScanResults {
                    ifindex,
                    results: dump.results,
                });
            }
            Err(error) => self.errors.push(error),
        }
    }

    fn start_dhcp(
        &mut self,
        now: Timestamp,
        ifindex: u32,
        queue: &mut VecDeque<Input>,
    ) -> Result<()> {
        if self.runners.contains_key(&ifindex) {
            return Ok(());
        }
        let device: &Device = self
            .model
            .device(ifindex)
            .ok_or_else(|| Error::SupplicantDevice(format!("interface {ifindex}")))?;
        let hardware = device.mac.ok_or_else(|| {
            Error::SupplicantDevice(format!("{} has no hardware address", device.name))
        })?;
        let transport = self.host.open_transport(ifindex)?;
        let xid_seed = self.host.random_u32()?;
        let mut client = Client::new(ClientConfig { hardware, xid_seed });
        let actions = client.start(now);
        self.runners.insert(ifindex, Runner { client, transport });
        self.act(ifindex, actions, queue);
        Ok(())
    }

    /// Carries out what a DHCP client asked for.
    fn act(&mut self, ifindex: u32, actions: Vec<Action>, queue: &mut VecDeque<Input>) {
        for action in actions {
            match action {
                Action::Send {
                    message,
                    destination,
                } => {
                    let Some(runner) = self.runners.get_mut(&ifindex) else {
                        continue;
                    };
                    if let Err(error) = runner.transport.send(
                        message.client_address,
                        destination,
                        &message.encode(),
                    ) {
                        self.errors.push(error);
                    }
                }
                Action::Bound(lease) => queue.push_back(Input::LeaseBound { ifindex, lease }),
                Action::Lost(_) => queue.push_back(Input::LeaseLost { ifindex }),
                Action::GaveUp => queue.push_back(Input::DhcpGaveUp { ifindex }),
            }
        }
    }
}

/// The default route a lease installs. The metric orders devices so Ethernet is preferred.
fn default_route(ifindex: u32, router: Ipv4Addr, kind: DeviceKind) -> RouteChange {
    RouteChange {
        family: Family::V4,
        destination: None,
        gateway: Some(IpAddr::V4(router)),
        index: ifindex,
        metric: Some(route_metric(kind)),
        protocol: PROTOCOL_DHCP,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::Duration;

    use cupertino_netlink::nl80211::ScanResult;
    use cupertino_netlink::rtnetlink::{LinkFlags, OperState};
    use cupertino_netlink::MacAddr;

    use super::*;
    use crate::dhcp::message::{MessageType, Options};
    use crate::supplicant::fake::{Call, FakeSupplicant};

    const IFF_UP: u32 = 0x1;
    const IFF_LOWER_UP: u32 = 0x1_0000;
    const ETH_MAC: MacAddr = MacAddr([2, 0, 0, 0, 0, 2]);

    /// What the fake host saw, shared with the test.
    #[derive(Default)]
    struct World {
        links: Vec<Link>,
        wireless: Vec<u32>,
        scan: Vec<ScanResult>,
        added_addresses: Vec<AddressChange>,
        removed_addresses: Vec<AddressChange>,
        added_routes: Vec<RouteChange>,
        removed_routes: Vec<RouteChange>,
        blocked: Vec<bool>,
        /// Datagrams the DHCP server sent, per interface, not yet read.
        inbox: VecDeque<Vec<u8>>,
        /// Messages the client sent, decoded.
        sent: Vec<(Ipv4Addr, Ipv4Addr, Message)>,
        fail_block: bool,
    }

    struct FakeHost(Rc<RefCell<World>>);

    struct FakeTransport(Rc<RefCell<World>>);

    impl Transport for FakeTransport {
        fn send(&mut self, source: Ipv4Addr, destination: Ipv4Addr, message: &[u8]) -> Result<()> {
            self.0
                .borrow_mut()
                .sent
                .push((source, destination, Message::decode(message).unwrap()));
            Ok(())
        }

        fn receive(&mut self) -> Result<Option<Vec<u8>>> {
            Ok(self.0.borrow_mut().inbox.pop_front())
        }

        fn as_fd(&self) -> Option<BorrowedFd<'_>> {
            None
        }
    }

    impl Host for FakeHost {
        fn links(&mut self) -> Result<Vec<Link>> {
            Ok(self.0.borrow().links.clone())
        }

        fn wireless_indexes(&mut self) -> Result<Vec<u32>> {
            Ok(self.0.borrow().wireless.clone())
        }

        fn scan_results(&mut self, _ifindex: u32) -> Result<ScanDump> {
            Ok(ScanDump {
                results: self.0.borrow().scan.clone(),
                errors: Vec::new(),
            })
        }

        fn set_link_up(&mut self, _ifindex: u32, _up: bool) -> Result<()> {
            Ok(())
        }

        fn add_address(&mut self, change: &AddressChange) -> Result<()> {
            self.0.borrow_mut().added_addresses.push(*change);
            Ok(())
        }

        fn remove_address(&mut self, change: &AddressChange) -> Result<()> {
            self.0.borrow_mut().removed_addresses.push(*change);
            Ok(())
        }

        fn add_route(&mut self, change: &RouteChange) -> Result<()> {
            self.0.borrow_mut().added_routes.push(*change);
            Ok(())
        }

        fn remove_route(&mut self, change: &RouteChange) -> Result<()> {
            self.0.borrow_mut().removed_routes.push(*change);
            Ok(())
        }

        fn set_wireless_blocked(&mut self, blocked: bool) -> Result<()> {
            let mut world = self.0.borrow_mut();
            if world.fail_block {
                return Err(Error::Syscall {
                    call: "write",
                    errno: rustix::io::Errno::ACCESS,
                });
            }
            world.blocked.push(blocked);
            Ok(())
        }

        fn open_transport(&mut self, _ifindex: u32) -> Result<Box<dyn Transport>> {
            Ok(Box::new(FakeTransport(self.0.clone())))
        }

        fn random_u32(&mut self) -> Result<u32> {
            Ok(0x1000)
        }
    }

    fn link(index: u32, name: &str, flags: u32, mac: MacAddr) -> Link {
        Link {
            index,
            name: name.to_owned(),
            hardware_type: 1,
            flags: LinkFlags(flags),
            oper_state: OperState::Up,
            mac: Some(mac),
            mtu: Some(1500),
            info_kind: None,
            master: None,
        }
    }

    fn at(seconds: u64) -> Timestamp {
        Duration::from_secs(seconds)
    }

    fn engine(
        world: &Rc<RefCell<World>>,
        dir: &std::path::Path,
    ) -> Engine<FakeHost, FakeSupplicant> {
        Engine::new(
            FakeHost(world.clone()),
            FakeSupplicant::default(),
            dir.join("resolv.conf"),
        )
    }

    /// The server's answer to the last message the client sent.
    fn server_reply(world: &Rc<RefCell<World>>, message_type: MessageType) -> Vec<u8> {
        let world = world.borrow();
        let (_, _, request) = world.sent.last().unwrap();
        let mut reply = Message::new(message_type, request.xid, request.client_hardware);
        reply.your_address = Ipv4Addr::new(10, 0, 0, 9);
        reply.options = Options {
            server_id: Some(Ipv4Addr::new(10, 0, 0, 1)),
            subnet_mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
            routers: vec![Ipv4Addr::new(10, 0, 0, 1)],
            dns_servers: vec![Ipv4Addr::new(10, 0, 0, 53)],
            lease_time: Some(3600),
            ..Options::default()
        };
        reply.encode()
    }

    #[test]
    fn a_cable_gets_an_address_a_default_route_and_the_resolver_from_dhcp() {
        let dir = tempfile::tempdir().unwrap();
        let world = Rc::new(RefCell::new(World {
            links: vec![link(2, "eth0", IFF_UP | IFF_LOWER_UP, ETH_MAC)],
            ..World::default()
        }));
        let mut engine = engine(&world, dir.path());
        engine.resync(at(0));
        assert_eq!(
            engine.model().device(2).unwrap().state,
            DeviceState::Configuring(crate::model::Stage::Addressing)
        );
        assert_eq!(
            world
                .borrow()
                .sent
                .last()
                .map(|(source, destination, message)| (
                    *source,
                    *destination,
                    message.message_type
                )),
            Some((
                Ipv4Addr::UNSPECIFIED,
                Ipv4Addr::BROADCAST,
                MessageType::Discover
            ))
        );

        let offer = server_reply(&world, MessageType::Offer);
        world.borrow_mut().inbox.push_back(offer);
        engine.on_dhcp_readable(at(1), 2);
        assert_eq!(
            world
                .borrow()
                .sent
                .last()
                .map(|(_, _, message)| message.message_type),
            Some(MessageType::Request)
        );
        let ack = server_reply(&world, MessageType::Ack);
        world.borrow_mut().inbox.push_back(ack);
        engine.on_dhcp_readable(at(1), 2);

        let world = world.borrow();
        assert_eq!(
            world.added_addresses,
            [AddressChange {
                index: 2,
                address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)),
                prefix_len: 24
            }]
        );
        assert_eq!(
            world.added_routes,
            [RouteChange {
                family: Family::V4,
                destination: None,
                gateway: Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
                index: 2,
                metric: Some(100),
                protocol: PROTOCOL_DHCP
            }]
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("resolv.conf")).unwrap(),
            "# written by cupertino-netd\nnameserver 10.0.0.53\n"
        );
        assert_eq!(
            engine.model().device(2).unwrap().state,
            DeviceState::Connected
        );
        assert!(engine.take_errors().is_empty());
    }

    #[test]
    fn unplugging_the_cable_takes_the_address_and_the_route_away() {
        let dir = tempfile::tempdir().unwrap();
        let world = Rc::new(RefCell::new(World {
            links: vec![link(2, "eth0", IFF_UP | IFF_LOWER_UP, ETH_MAC)],
            ..World::default()
        }));
        let mut engine = engine(&world, dir.path());
        engine.resync(at(0));
        let offer = server_reply(&world, MessageType::Offer);
        world.borrow_mut().inbox.push_back(offer);
        engine.on_dhcp_readable(at(1), 2);
        let ack = server_reply(&world, MessageType::Ack);
        world.borrow_mut().inbox.push_back(ack);
        engine.on_dhcp_readable(at(1), 2);

        engine.on_route_events(
            at(5),
            vec![RouteEvent::LinkNew(link(2, "eth0", IFF_UP, ETH_MAC))],
        );
        let world = world.borrow();
        assert_eq!(world.removed_addresses.len(), 1);
        assert_eq!(world.removed_routes.len(), 1);
        assert_eq!(
            engine.model().device(2).unwrap().state,
            DeviceState::Unavailable
        );
        assert_eq!(engine.next_deadline(), None);
    }

    #[test]
    fn a_silent_server_is_retried_by_the_timer_and_reported_after_the_schedule() {
        let dir = tempfile::tempdir().unwrap();
        let world = Rc::new(RefCell::new(World {
            links: vec![link(2, "eth0", IFF_UP | IFF_LOWER_UP, ETH_MAC)],
            ..World::default()
        }));
        let mut engine = engine(&world, dir.path());
        engine.resync(at(0));
        assert_eq!(engine.next_deadline(), Some(at(4)));
        // the schedule is 4, 8, 16, 32 and 64 s (RFC 2131, 4.1); after the last the client reports
        // and discovers again
        for when in [4, 12, 28, 60, 124] {
            engine.on_timer(at(when));
        }
        assert_eq!(world.borrow().sent.len(), 6);
        assert_eq!(
            engine.model().device(2).unwrap().failure,
            Some(crate::model::Failure::NoAddress)
        );
    }

    fn wifi_world() -> Rc<RefCell<World>> {
        let result = |ssid: &str, security: Security, signal: i32| ScanResult {
            ifindex: 3,
            bssid: MacAddr([2, 0, 0, 0, 1, signal.unsigned_abs() as u8]),
            ssid: Ssid(ssid.as_bytes().to_vec()),
            frequency_mhz: 2412,
            signal_dbm: Some(signal),
            security,
            associated: false,
            seen_ms_ago: None,
        };
        Rc::new(RefCell::new(World {
            links: vec![link(
                3,
                "wlan0",
                IFF_UP | IFF_LOWER_UP,
                MacAddr([2, 0, 0, 0, 0, 3]),
            )],
            wireless: vec![3],
            scan: vec![
                result("home", Security::Wpa2, -50),
                result("cafe", Security::Open, -60),
                result("old", Security::Wep, -70),
            ],
            ..World::default()
        }))
    }

    fn started_wifi(
        dir: &std::path::Path,
    ) -> (Rc<RefCell<World>>, Engine<FakeHost, FakeSupplicant>) {
        let world = wifi_world();
        let mut engine = engine(&world, dir);
        engine.on_rfkill(
            at(0),
            &[RfkillEvent {
                index: 0,
                kind: cupertino_netlink::rfkill::Kind::Wlan,
                operation: cupertino_netlink::rfkill::Operation::Added,
                soft_blocked: false,
                hard_blocked: false,
            }],
        );
        engine.resync(at(0));
        (world, engine)
    }

    #[test]
    fn a_wifi_device_is_powered_and_its_scan_list_read_when_it_appears() {
        let dir = tempfile::tempdir().unwrap();
        let (_, engine) = started_wifi(dir.path());
        assert_eq!(engine.supplicant().calls, [Call::PowerOn(3)]);
        let names: Vec<_> = engine
            .model()
            .networks(3)
            .into_iter()
            .map(|network| network.ssid.to_text())
            .collect();
        assert_eq!(names, ["home", "cafe", "old"]);
    }

    #[test]
    fn connecting_hands_the_passphrase_to_the_supplicant_once_and_dhcp_starts_when_it_joins() {
        let dir = tempfile::tempdir().unwrap();
        let (world, mut engine) = started_wifi(dir.path());
        engine
            .command(
                at(1),
                Command::Connect {
                    ssid: "home".to_owned(),
                    passphrase: Some("correct horse".to_owned()),
                },
            )
            .unwrap();
        assert_eq!(
            engine.supplicant().calls.last(),
            Some(&Call::Connect {
                ifindex: 3,
                ssid: Ssid(b"home".to_vec()),
                class: NetworkClass::Psk,
                passphrase: Some("correct horse".to_owned())
            })
        );
        assert_eq!(
            engine.model().device(3).unwrap().state,
            DeviceState::Configuring(crate::model::Stage::Authenticating)
        );

        engine.supplicant.queued.push(SupplicantEvent::Connected {
            ifindex: 3,
            ssid: Ssid(b"home".to_vec()),
        });
        engine.on_supplicant(at(2));
        assert_eq!(
            world
                .borrow()
                .sent
                .last()
                .map(|(_, _, message)| message.message_type),
            Some(MessageType::Discover)
        );

        // the next connection does not reuse the passphrase
        engine
            .command(
                at(3),
                Command::Connect {
                    ssid: "cafe".to_owned(),
                    passphrase: Some("ignored".to_owned()),
                },
            )
            .unwrap();
        assert_eq!(
            engine.supplicant().calls.last(),
            Some(&Call::Connect {
                ifindex: 3,
                ssid: Ssid(b"cafe".to_vec()),
                class: NetworkClass::Open,
                passphrase: None
            })
        );
    }

    #[test]
    fn a_wrong_passphrase_ends_with_the_reason_and_no_dhcp() {
        let dir = tempfile::tempdir().unwrap();
        let (world, mut engine) = started_wifi(dir.path());
        engine
            .command(
                at(1),
                Command::Connect {
                    ssid: "home".to_owned(),
                    passphrase: Some("nope".to_owned()),
                },
            )
            .unwrap();
        engine.supplicant.queued.push(SupplicantEvent::Failed {
            ifindex: 3,
            reason: ConnectFailure::WrongPassphrase,
        });
        engine.on_supplicant(at(2));
        let device = engine.model().device(3).unwrap();
        assert_eq!(
            (device.state, device.failure),
            (
                DeviceState::Disconnected,
                Some(crate::model::Failure::WrongPassphrase)
            )
        );
        assert!(world.borrow().sent.is_empty());
    }

    #[test]
    fn commands_that_cannot_work_are_refused_before_anything_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut engine) = started_wifi(dir.path());
        let connect = |ssid: &str, passphrase: Option<&str>| Command::Connect {
            ssid: ssid.to_owned(),
            passphrase: passphrase.map(str::to_owned),
        };
        assert_eq!(
            engine.command(at(1), connect("home", None)),
            Err(CommandError::PassphraseRequired("home".to_owned()))
        );
        assert_eq!(
            engine.command(at(1), connect("home", Some(""))),
            Err(CommandError::PassphraseRequired("home".to_owned()))
        );
        assert_eq!(
            engine.command(at(1), connect("nowhere", None)),
            Err(CommandError::UnknownNetwork("nowhere".to_owned()))
        );
        assert_eq!(
            engine.command(at(1), connect("old", Some("x"))),
            Err(CommandError::UnsupportedSecurity(Security::Wep))
        );
        assert_eq!(engine.supplicant().calls, [Call::PowerOn(3)]);
    }

    #[test]
    fn a_machine_without_wifi_refuses_wifi_commands() {
        let dir = tempfile::tempdir().unwrap();
        let world = Rc::new(RefCell::new(World {
            links: vec![link(2, "eth0", IFF_UP, ETH_MAC)],
            ..World::default()
        }));
        let mut engine = engine(&world, dir.path());
        engine.resync(at(0));
        assert_eq!(
            engine.command(at(1), Command::Scan),
            Err(CommandError::NoWifiDevice)
        );
    }

    #[test]
    fn switching_wifi_off_writes_the_kernel_switch_and_a_failed_write_reaches_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let (world, mut engine) = started_wifi(dir.path());
        engine
            .command(at(1), Command::SetWirelessEnabled(false))
            .unwrap();
        assert_eq!(world.borrow().blocked, [true]);
        world.borrow_mut().fail_block = true;
        let result = engine.command(at(2), Command::SetWirelessEnabled(true));
        assert_eq!(
            result,
            Err(CommandError::Failed(
                "write: Permission denied (os error 13)".to_owned()
            ))
        );
    }
}
