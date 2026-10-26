//! The pure model of the adapters and their devices.
//!
//! The model owns no socket, file, clock or bus. Everything that happens to it is a call that
//! passes the time in and returns [`Effect`]s: management packets from the kernel
//! ([`Model::on_packet`]), requests from callers ([`Model::request`]), answers from the pairing
//! agent ([`Model::agent_answered`]), stored keys ([`Model::keys_loaded`]) and time passing
//! ([`Model::tick`]). The caller carries the effects out: it sends commands, writes keys,
//! signals clients and answers the requests that finished.
//!
//! Several adapters are tracked, but the bus shows and controls the default one, the lowest
//! controller index that is ready.

use std::collections::BTreeMap;
use std::time::Duration;

use cupertino_bluetooth::reply::{parse_connections, parse_device, parse_settings, ControllerInfo};
use cupertino_bluetooth::{Address, AddressTypes, ClassOfDevice, Command, ControllerIndex, DeviceId, Event, IoCapability, Opcode, Packet, Settings, Status};

use crate::device::{device_key, Device, DeviceInfo, DeviceRef};
use crate::keys::KeySet;
use crate::pairing::{Action, PairError, Pairing, PairingId};
use crate::request::{AdapterProps, AgentAnswer, AgentCall, Change, Effect, Request, RequestError, Ticket};

/// How long a discovery the user started runs before the daemon stops it. approximate: the
/// kernel's own discovery ends after one interleaved round of a few seconds, and the daemon starts
/// another until this time is up, so a shell that crashes with discovery on does not leave the
/// radio scanning for good.
pub const DISCOVERY_DURATION: Duration = Duration::from_secs(60);

/// How long a connection attempt may take. approximate: the controller's own page timeout is a
/// few seconds, but a device that is out of range or asleep may take several attempts.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the controller may take to answer a command before the request that waits for it
/// fails. approximate: the kernel times its own commands out after a few seconds; the margin
/// keeps a request from hanging when a reply is lost.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a device that was only discovered (not paired, not connected) stays listed after it
/// was last reported. approximate.
pub const DEVICE_TTL: Duration = Duration::from_secs(120);

/// The IO capability the daemon pairs with: it can show a number and take yes or no, and the user
/// can type a passkey. The kernel turns KeyboardDisplay into DisplayYesNo on a BR/EDR link
/// (`doc/mgmt-protocol.rst`, "Set IO Capability").
const IO_CAPABILITY: IoCapability = IoCapability::KeyboardDisplay;

/// A controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub index: ControllerIndex,
    pub address: Address,
    pub name: String,
    pub supported: Settings,
    pub settings: Settings,
    pub class: ClassOfDevice,
}

/// A request that waits for the controller. Past `deadline` it fails with a timeout.
#[derive(Clone, Copy, Debug)]
enum Pending {
    SetPowered { ticket: Ticket, deadline: Duration },
    StartDiscovery { ticket: Ticket, deadline: Duration },
    StopDiscovery { ticket: Ticket, deadline: Duration },
    Connect { ticket: Ticket, device: DeviceId, deadline: Duration },
    Disconnect { ticket: Ticket, device: DeviceId, deadline: Duration },
    Unpair { ticket: Ticket, device: DeviceId, deadline: Duration },
}

impl Pending {
    fn ticket(&self) -> Ticket {
        match self {
            Self::SetPowered { ticket, .. }
            | Self::StartDiscovery { ticket, .. }
            | Self::StopDiscovery { ticket, .. }
            | Self::Connect { ticket, .. }
            | Self::Disconnect { ticket, .. }
            | Self::Unpair { ticket, .. } => *ticket,
        }
    }

    fn deadline(&self) -> Duration {
        match self {
            Self::SetPowered { deadline, .. }
            | Self::StartDiscovery { deadline, .. }
            | Self::StopDiscovery { deadline, .. }
            | Self::Connect { deadline, .. }
            | Self::Disconnect { deadline, .. }
            | Self::Unpair { deadline, .. } => *deadline,
        }
    }
}

struct AdapterState {
    adapter: Adapter,
    /// Keys are loaded and the controller is set up.
    ready: bool,
    /// This is the default adapter: its changes are announced.
    announce: bool,
    devices: BTreeMap<DeviceId, Device>,
    keys: KeySet,
    pairing: Option<Pairing>,
    /// The request waiting for `pairing`, when this side started it.
    pairing_ticket: Option<Ticket>,
    pairing_sequence: u64,
    /// When set, the user wants discovery on until this time.
    discovery_until: Option<Duration>,
    pending: Vec<Pending>,
}

impl AdapterState {
    fn new(adapter: Adapter) -> Self {
        Self {
            adapter,
            ready: false,
            announce: false,
            devices: BTreeMap::new(),
            keys: KeySet::default(),
            pairing: None,
            pairing_ticket: None,
            pairing_sequence: 0,
            discovery_until: None,
            pending: Vec::new(),
        }
    }

    fn send(&self, out: &mut Vec<Effect>, command: Command) {
        out.push(Effect::Send { index: self.adapter.index, command });
    }

    fn notify(&self, out: &mut Vec<Effect>, change: Change) {
        if self.announce {
            out.push(Effect::Changed(change));
        }
    }

    fn info(&self, device: &Device) -> DeviceInfo {
        device.info(self.pairing.as_ref().is_some_and(|pairing| pairing.device == device.id))
    }

    fn notify_device(&self, out: &mut Vec<Effect>, id: DeviceId) {
        if let Some(device) = self.devices.get(&id) {
            self.notify(out, Change::DeviceChanged(self.info(device)));
        }
    }

    /// Applies `change` to the device, creating it when unknown, and announces what happened.
    fn touch(&mut self, now: Duration, id: DeviceId, out: &mut Vec<Effect>, change: impl FnOnce(&mut Device) -> bool) {
        let created = !self.devices.contains_key(&id);
        let device = self.devices.entry(id).or_insert_with(|| Device::new(id, now));
        device.last_seen = now;
        let changed = change(device);
        if created {
            self.notify(out, Change::DeviceAdded(self.info(&self.devices[&id])));
        } else if changed {
            self.notify_device(out, id);
        }
    }

    fn resolve(&self, reference: DeviceRef) -> Result<DeviceId, RequestError> {
        let mut found = self.devices.keys().filter(|id| id.address == reference.address && reference.kind.is_none_or(|kind| kind == id.kind));
        match (found.next(), found.next()) {
            (Some(id), None) => Ok(*id),
            (None, _) => Err(RequestError::UnknownDevice(reference.kind.map_or_else(|| reference.address.to_string(), |kind| format!("{}/{kind}", reference.address)))),
            (Some(_), Some(_)) => Err(RequestError::AmbiguousDevice(reference.address.to_string())),
        }
    }

    fn take_pending(&mut self, matches: impl Fn(&Pending) -> bool) -> Option<Pending> {
        let position = self.pending.iter().position(matches)?;
        Some(self.pending.remove(position))
    }

    fn complete(&mut self, out: &mut Vec<Effect>, pending: Pending, result: Result<(), RequestError>) {
        out.push(Effect::Complete { ticket: pending.ticket(), result });
    }

    /// Loads the stored keys into the model and sets the controller up.
    fn bring_up(&mut self, now: Duration, keys: KeySet, out: &mut Vec<Effect>) {
        for (id, bond) in keys.bonds() {
            let mut device = Device::new(*id, now);
            device.paired = true;
            device.bonded = true;
            if let Some(name) = &bond.name {
                device.set_stored_name(name);
            }
            self.notify(out, Change::DeviceAdded(self.info(&device)));
            self.devices.insert(*id, device);
        }
        self.send(out, Command::LoadLinkKeys(keys.link_keys()));
        self.send(out, Command::LoadLongTermKeys(keys.long_term_keys()));
        self.keys = keys;
        self.send(out, Command::SetIoCapability(IO_CAPABILITY));
        self.send(out, Command::SetBondable(true));
        self.send(out, Command::SetConnectable(true));
        if self.adapter.settings.powered() {
            self.send(out, Command::GetConnections);
        }
    }

    fn start_request(&mut self, now: Duration, ticket: Ticket, request: Request, out: &mut Vec<Effect>) -> Result<(), RequestError> {
        let powered = self.adapter.settings.powered();
        let need_power = || if powered { Ok(()) } else { Err(RequestError::NotPowered) };
        match request {
            Request::SetPowered(on) if on == powered => out.push(Effect::Complete { ticket, result: Ok(()) }),
            Request::SetPowered(on) => {
                self.send(out, Command::SetPowered(on));
                self.pending.push(Pending::SetPowered { ticket, deadline: now + REPLY_TIMEOUT });
            }
            Request::StartDiscovery => {
                need_power()?;
                if self.discovery_until.is_some() {
                    self.discovery_until = Some(now + DISCOVERY_DURATION);
                    out.push(Effect::Complete { ticket, result: Ok(()) });
                } else {
                    self.send(out, Command::StartDiscovery(AddressTypes::ALL));
                    self.pending.push(Pending::StartDiscovery { ticket, deadline: now + REPLY_TIMEOUT });
                }
            }
            Request::StopDiscovery => {
                // Cleared at once so that a Discovering event that arrives before the reply does
                // not start another round.
                if self.discovery_until.take().is_none() {
                    out.push(Effect::Complete { ticket, result: Ok(()) });
                } else {
                    self.send(out, Command::StopDiscovery(AddressTypes::ALL));
                    self.pending.push(Pending::StopDiscovery { ticket, deadline: now + REPLY_TIMEOUT });
                }
            }
            Request::Pair(reference) => {
                let device = self.resolve(reference)?;
                need_power()?;
                if self.devices[&device].paired {
                    return Err(RequestError::AlreadyPaired);
                }
                if self.pairing.is_some() {
                    return Err(RequestError::Busy);
                }
                self.send(out, Command::PairDevice { device, io_capability: IO_CAPABILITY });
                self.begin_pairing(now, device, true);
                self.pairing_ticket = Some(ticket);
                self.notify_device(out, device);
            }
            Request::Connect(reference) => {
                let device = self.resolve(reference)?;
                need_power()?;
                if self.devices[&device].connected {
                    return Err(RequestError::AlreadyConnected);
                }
                out.push(Effect::OpenLink { index: self.adapter.index, adapter: self.adapter.address, device, ticket });
                self.pending.push(Pending::Connect { ticket, device, deadline: now + CONNECT_TIMEOUT });
            }
            Request::Disconnect(reference) => {
                let device = self.resolve(reference)?;
                need_power()?;
                if !self.devices[&device].connected {
                    return Err(RequestError::NotConnected);
                }
                self.send(out, Command::Disconnect(device));
                self.pending.push(Pending::Disconnect { ticket, device, deadline: now + REPLY_TIMEOUT });
            }
            Request::Remove(reference) => {
                let device = self.resolve(reference)?;
                let known = &self.devices[&device];
                if known.paired || known.connected || self.keys.bond(&device).is_some() {
                    need_power()?;
                    self.send(out, Command::UnpairDevice { device, disconnect: true });
                    self.pending.push(Pending::Unpair { ticket, device, deadline: now + REPLY_TIMEOUT });
                } else {
                    self.devices.remove(&device);
                    self.notify(out, Change::DeviceRemoved(device_key(&device)));
                    out.push(Effect::Complete { ticket, result: Ok(()) });
                }
            }
        }
        Ok(())
    }

    fn begin_pairing(&mut self, now: Duration, device: DeviceId, local: bool) {
        self.pairing_sequence += 1;
        let id = PairingId(u64::from(self.adapter.index.0) << 32 | self.pairing_sequence);
        self.pairing = Some(Pairing::new(id, device, local, now));
    }

    /// The pairing for `device`, started on the remote side's behalf when there is none. `None`
    /// when a pairing with another device is running.
    fn pairing_for(&mut self, now: Duration, device: DeviceId, out: &mut Vec<Effect>) -> Option<&mut Pairing> {
        match &self.pairing {
            Some(pairing) if pairing.device != device => return None,
            Some(_) => {}
            None => {
                self.touch(now, device, out, |_| false);
                self.begin_pairing(now, device, false);
                self.notify_device(out, device);
            }
        }
        self.pairing.as_mut()
    }

    /// Carries out what the pairing machine asked for.
    fn run_pairing(&mut self, actions: Vec<Action>, out: &mut Vec<Effect>) {
        for action in actions {
            match action {
                Action::Send(command) => self.send(out, command),
                Action::Prompt(prompt) => {
                    let Some(pairing) = &self.pairing else { continue };
                    let Some(device) = self.devices.get(&pairing.device) else { continue };
                    out.push(Effect::Agent(AgentCall { pairing: pairing.id, device: self.info(device), prompt }));
                }
                Action::Finish(result) => self.finish_pairing(result, out),
            }
        }
    }

    fn finish_pairing(&mut self, result: Result<(), PairError>, out: &mut Vec<Effect>) {
        let Some(pairing) = self.pairing.take() else { return };
        let ticket = self.pairing_ticket.take();
        if result.is_ok() {
            if let Some(device) = self.devices.get_mut(&pairing.device) {
                device.paired = true;
            }
        } else if let (Err(error), None) = (&result, ticket) {
            // Nobody waits for a pairing the remote side started: say why it failed.
            out.push(Effect::Log(format!("pairing with {} failed: {}", pairing.device.address, RequestError::from(error.clone()))));
        }
        self.notify_device(out, pairing.device);
        if let Some(ticket) = ticket {
            out.push(Effect::Complete { ticket, result: result.map_err(RequestError::from) });
        }
    }

    fn apply_settings(&mut self, settings: Settings, out: &mut Vec<Effect>) {
        let was_powered = self.adapter.settings.powered();
        self.adapter.settings = settings;
        if was_powered && !settings.powered() {
            self.powered_off(out);
        }
    }

    /// The controller is off: nothing is connected, discovery and pairing are over.
    fn powered_off(&mut self, out: &mut Vec<Effect>) {
        self.discovery_until = None;
        let connected: Vec<DeviceId> = self.devices.values().filter(|device| device.connected).map(|device| device.id).collect();
        for id in connected {
            if let Some(device) = self.devices.get_mut(&id) {
                device.connected = false;
            }
            out.push(Effect::CloseLink { adapter: self.adapter.address, device: id });
            self.notify_device(out, id);
        }
        if let Some(pairing) = &mut self.pairing {
            let actions = pairing.on_aborted();
            self.run_pairing(actions, out);
        }
        // The reply to the power-off itself still resolves; the rest cannot finish.
        let waiting = std::mem::take(&mut self.pending);
        for pending in waiting {
            if matches!(pending, Pending::SetPowered { .. }) {
                self.pending.push(pending);
            } else {
                self.complete(out, pending, Err(RequestError::NotPowered));
            }
        }
    }

    /// The controller is gone.
    fn removed(&mut self, out: &mut Vec<Effect>) {
        self.discovery_until = None;
        if let Some(pairing) = &mut self.pairing {
            let actions = pairing.on_aborted();
            self.run_pairing(actions, out);
        }
        for pending in std::mem::take(&mut self.pending) {
            self.complete(out, pending, Err(RequestError::NoAdapter));
        }
        for device in self.devices.values() {
            if device.connected {
                out.push(Effect::CloseLink { adapter: self.adapter.address, device: device.id });
            }
            self.notify(out, Change::DeviceRemoved(device_key(&device.id)));
        }
    }

    fn on_event(&mut self, now: Duration, event: Event, out: &mut Vec<Effect>) {
        match event {
            Event::CommandComplete { opcode, status, data } => self.on_reply(now, opcode, status, &data, out),
            Event::CommandStatus { opcode, status } => {
                if !status.is_success() {
                    self.on_reply(now, opcode, status, &[], out);
                }
            }
            Event::NewSettings(settings) => self.apply_settings(settings, out),
            Event::LocalNameChanged { name, .. } => self.adapter.name = name,
            Event::ClassOfDeviceChanged(class) => self.adapter.class = class,
            Event::ControllerError { code } => out.push(Effect::Log(format!("controller {} reported hardware error {code}", self.adapter.index.0))),
            Event::DeviceFound { device, rssi, flags, eir } => {
                self.touch(now, device, out, |known| known.merge(now, rssi, &eir));
                if flags.confirm_name() {
                    let name_known = self.devices.get(&device).is_some_and(|known| known.name().is_some());
                    self.send(out, Command::ConfirmName { device, name_known });
                }
                self.remember_name(device, out);
            }
            Event::DeviceConnected { device, eir, .. } => {
                self.touch(now, device, out, |known| {
                    let was = known.connected;
                    known.connected = true;
                    known.merge(now, None, &eir) || !was
                });
                self.remember_name(device, out);
                if let Some(pending) = self.take_pending(|p| matches!(p, Pending::Connect { device: d, .. } if *d == device)) {
                    self.complete(out, pending, Ok(()));
                }
            }
            Event::DeviceDisconnected { device, .. } => {
                self.touch(now, device, out, |known| std::mem::replace(&mut known.connected, false));
                out.push(Effect::CloseLink { adapter: self.adapter.address, device });
                if let Some(pending) = self.take_pending(|p| matches!(p, Pending::Disconnect { device: d, .. } if *d == device)) {
                    self.complete(out, pending, Ok(()));
                }
                if let Some(pending) = self.take_pending(|p| matches!(p, Pending::Connect { device: d, .. } if *d == device)) {
                    self.complete(out, pending, Err(RequestError::ConnectFailed));
                }
            }
            Event::ConnectFailed { device, status } => {
                out.push(Effect::CloseLink { adapter: self.adapter.address, device });
                if let Some(pending) = self.take_pending(|p| matches!(p, Pending::Connect { device: d, .. } if *d == device)) {
                    self.complete(out, pending, Err(RequestError::from_status(status)));
                }
            }
            Event::Discovering { active: false, .. } => match self.discovery_until {
                Some(until) if now < until => self.send(out, Command::StartDiscovery(AddressTypes::ALL)),
                _ => self.discovery_until = None,
            },
            Event::Discovering { active: true, .. } => {}
            Event::UserConfirmationRequest { device, just_works, value } => match self.pairing_for(now, device, out) {
                Some(pairing) => {
                    let actions = pairing.on_confirm_request(just_works, value);
                    self.run_pairing(actions, out);
                }
                None => self.send(out, Command::UserConfirmationReply { device, accept: false }),
            },
            Event::UserPasskeyRequest { device } => match self.pairing_for(now, device, out) {
                Some(pairing) => {
                    let actions = pairing.on_passkey_request();
                    self.run_pairing(actions, out);
                }
                None => self.send(out, Command::UserPasskeyReply { device, passkey: None }),
            },
            Event::PasskeyNotify { device, passkey, entered } => {
                if let Some(pairing) = self.pairing_for(now, device, out) {
                    let actions = pairing.on_passkey_notify(passkey, entered);
                    self.run_pairing(actions, out);
                }
            }
            Event::PinCodeRequest { device, .. } => match self.pairing_for(now, device, out) {
                Some(pairing) => {
                    let actions = pairing.on_pin_request();
                    self.run_pairing(actions, out);
                }
                None => self.send(out, Command::PinCodeNegativeReply(device)),
            },
            Event::AuthenticationFailed { device, status } => {
                if let Some(pairing) = self.pairing.as_mut().filter(|pairing| pairing.device == device) {
                    let actions = pairing.on_authentication_failed(status);
                    self.run_pairing(actions, out);
                }
            }
            Event::NewLinkKey { store, key } => {
                self.key_arrived(now, key.device, store, out, |keys| keys.set_link_key(key));
            }
            Event::NewLongTermKey { store, key } => {
                self.key_arrived(now, key.device, store, out, |keys| keys.set_long_term_key(key));
            }
            Event::DeviceUnpaired(device) => {
                if let Some(known) = self.devices.get_mut(&device) {
                    known.paired = false;
                    known.bonded = false;
                }
                self.notify_device(out, device);
                self.forget_keys(device, out);
            }
            Event::IndexAdded | Event::IndexRemoved | Event::Other { .. } => {}
        }
    }

    /// A pairing produced a key. Whether it is stored depends on the kernel's hint.
    fn key_arrived(&mut self, now: Duration, device: DeviceId, store: bool, out: &mut Vec<Effect>, record: impl FnOnce(&mut KeySet)) {
        self.touch(now, device, out, |known| {
            let changed = !known.paired || (store && !known.bonded);
            known.paired = true;
            known.bonded |= store;
            changed
        });
        if store {
            record(&mut self.keys);
            if let Some(name) = self.devices.get(&device).and_then(|known| known.name().map(str::to_owned)) {
                self.keys.set_name(&device, &name);
            }
            out.push(Effect::SaveKeys { address: self.adapter.address, keys: self.keys.clone() });
        }
        if let Some(pairing) = self.pairing.as_mut().filter(|pairing| pairing.device == device) {
            let actions = pairing.on_key();
            self.run_pairing(actions, out);
        }
    }

    /// Stores a name learned for a device that has keys.
    fn remember_name(&mut self, device: DeviceId, out: &mut Vec<Effect>) {
        let Some(name) = self.devices.get(&device).and_then(|known| known.name().map(str::to_owned)) else { return };
        if self.keys.set_name(&device, &name) {
            out.push(Effect::SaveKeys { address: self.adapter.address, keys: self.keys.clone() });
        }
    }

    fn forget_keys(&mut self, device: DeviceId, out: &mut Vec<Effect>) {
        if self.keys.remove(&device) {
            out.push(Effect::SaveKeys { address: self.adapter.address, keys: self.keys.clone() });
        }
    }

    fn on_reply(&mut self, now: Duration, opcode: Opcode, status: Status, data: &[u8], out: &mut Vec<Effect>) {
        let failure = RequestError::from_status(status);
        let result = if status.is_success() { Ok(()) } else { Err(failure.clone()) };
        // Which device the reply is about, when it says: the addressed commands echo it.
        let replied = parse_device(data).ok();
        let about = |pending: &Pending| match (pending, replied) {
            (Pending::Connect { device, .. } | Pending::Disconnect { device, .. } | Pending::Unpair { device, .. }, Some(replied)) => *device == replied,
            _ => true,
        };
        match opcode {
            Opcode::SET_POWERED | Opcode::SET_CONNECTABLE | Opcode::SET_BONDABLE | Opcode::SET_DISCOVERABLE => {
                if status.is_success() {
                    match parse_settings(data) {
                        Ok(settings) => self.apply_settings(settings, out),
                        Err(error) => out.push(Effect::Log(format!("reply to command {:#06x}: {error}", opcode.0))),
                    }
                }
                if opcode == Opcode::SET_POWERED {
                    if let Some(pending) = self.take_pending(|p| matches!(p, Pending::SetPowered { .. })) {
                        self.complete(out, pending, result);
                    }
                } else if !status.is_success() {
                    out.push(Effect::Log(format!("command {:#06x} failed: {status}", opcode.0)));
                }
            }
            Opcode::START_DISCOVERY => {
                if status.is_success() {
                    self.discovery_until = Some(now + DISCOVERY_DURATION);
                } else {
                    self.discovery_until = None;
                }
                match self.take_pending(|p| matches!(p, Pending::StartDiscovery { .. })) {
                    Some(pending) => self.complete(out, pending, result),
                    None if !status.is_success() => out.push(Effect::Log(format!("restarting discovery failed: {status}"))),
                    None => {}
                }
            }
            Opcode::STOP_DISCOVERY => {
                if let Some(pending) = self.take_pending(|p| matches!(p, Pending::StopDiscovery { .. })) {
                    // Rejected: the controller was not discovering, which is what was asked for.
                    let result = if status == Status::Rejected { Ok(()) } else { result };
                    self.complete(out, pending, result);
                }
            }
            Opcode::PAIR_DEVICE => {
                if let Some(pairing) = self.pairing.as_mut().filter(|pairing| replied.is_none_or(|device| device == pairing.device)) {
                    let actions = pairing.on_pair_reply(status);
                    self.run_pairing(actions, out);
                }
            }
            Opcode::UNPAIR_DEVICE => {
                let Some(Pending::Unpair { ticket, device, .. }) = self.take_pending(|p| matches!(p, Pending::Unpair { .. }) && about(p)) else {
                    return out.push(Effect::Log(format!("unexpected reply to Unpair Device: {status}")));
                };
                if status.is_success() {
                    self.forget_keys(device, out);
                    if self.devices.remove(&device).is_some() {
                        self.notify(out, Change::DeviceRemoved(device_key(&device)));
                    }
                }
                out.push(Effect::Complete { ticket, result });
            }
            Opcode::DISCONNECT => {
                if let Some(pending) = self.take_pending(|p| matches!(p, Pending::Disconnect { .. }) && about(p)) {
                    self.complete(out, pending, result);
                }
            }
            Opcode::GET_CONNECTIONS => match status.is_success().then(|| parse_connections(data)) {
                Some(Ok(devices)) => {
                    for device in devices {
                        self.touch(now, device, out, |known| !std::mem::replace(&mut known.connected, true));
                    }
                }
                Some(Err(error)) => out.push(Effect::Log(format!("reply to Get Connections: {error}"))),
                None => out.push(Effect::Log(format!("Get Connections failed: {status}"))),
            },
            other if !status.is_success() => out.push(Effect::Log(format!("command {:#06x} failed: {status}", other.0))),
            _ => {}
        }
    }

    fn on_agent_answer(&mut self, answer: AgentAnswer, out: &mut Vec<Effect>) {
        let Some(pairing) = self.pairing.as_mut().filter(|pairing| pairing.id == answer.pairing) else { return };
        let actions = pairing.on_agent_reply(answer.reply);
        self.run_pairing(actions, out);
    }

    fn tick(&mut self, now: Duration, out: &mut Vec<Effect>) {
        if let Some(pairing) = &mut self.pairing {
            let actions = pairing.on_tick(now);
            self.run_pairing(actions, out);
        }
        if self.discovery_until.is_some_and(|until| until <= now) {
            self.discovery_until = None;
            self.send(out, Command::StopDiscovery(AddressTypes::ALL));
        }
        let expired: Vec<Pending> = self.pending.iter().copied().filter(|p| p.deadline() <= now).collect();
        for pending in expired {
            self.pending.retain(|p| p.ticket() != pending.ticket());
            if let Pending::Connect { device, .. } = pending {
                out.push(Effect::CloseLink { adapter: self.adapter.address, device });
            }
            self.complete(out, pending, Err(RequestError::Timeout));
        }
        let busy = self.pairing.as_ref().map(|pairing| pairing.device);
        let stale: Vec<DeviceId> = self
            .devices
            .values()
            .filter(|device| !device.paired && !device.connected && Some(device.id) != busy && device.last_seen + DEVICE_TTL <= now)
            .map(|device| device.id)
            .collect();
        for id in stale {
            self.devices.remove(&id);
            self.notify(out, Change::DeviceRemoved(device_key(&id)));
        }
    }

    fn next_deadline(&self) -> Option<Duration> {
        let pairing = self.pairing.as_ref().map(Pairing::deadline);
        let waiting = self.pending.iter().map(Pending::deadline);
        let expiry = self.devices.values().filter(|device| !device.paired && !device.connected).map(|device| device.last_seen + DEVICE_TTL);
        pairing.into_iter().chain(self.discovery_until).chain(waiting).chain(expiry).min()
    }
}

/// The adapters and their devices.
pub struct Model {
    read_only: bool,
    adapters: BTreeMap<ControllerIndex, AdapterState>,
}

impl Model {
    /// A read-only model sends nothing but the commands that read, and refuses every request: the
    /// kernel would refuse them anyway to a process without `CAP_NET_ADMIN`.
    pub fn new(read_only: bool) -> Self {
        Self { read_only, adapters: BTreeMap::new() }
    }

    /// The first thing to send: asks which controllers exist.
    pub fn start(&self) -> Vec<Effect> {
        vec![Effect::Send { index: ControllerIndex::NONE, command: Command::ReadIndexList }]
    }

    fn default_index(&self) -> Option<ControllerIndex> {
        self.adapters.iter().find(|(_, state)| state.ready).map(|(index, _)| *index)
    }

    /// Marks which adapter is the default, so that only its changes are announced.
    fn mark_default(&mut self) {
        let default = self.default_index();
        for (index, state) in &mut self.adapters {
            state.announce = Some(*index) == default;
        }
    }

    /// The default adapter.
    pub fn adapter(&self) -> Option<&Adapter> {
        self.default_index().map(|index| &self.adapters[&index].adapter)
    }

    /// The properties of the default adapter that the bus shows.
    pub fn props(&self) -> AdapterProps {
        self.default_index().map_or_else(AdapterProps::default, |index| {
            let state = &self.adapters[&index];
            AdapterProps { available: true, powered: state.adapter.settings.powered(), discovering: state.discovery_until.is_some() }
        })
    }

    /// The devices of the default adapter, by address.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.default_index().map_or_else(Vec::new, |index| {
            let state = &self.adapters[&index];
            state.devices.values().map(|device| state.info(device)).collect()
        })
    }

    /// Runs `step`, then adds the change of the adapter properties it caused.
    fn track(&mut self, step: impl FnOnce(&mut Self) -> Vec<Effect>) -> Vec<Effect> {
        let old = self.props();
        self.mark_default();
        let mut effects = step(self);
        let new = self.props();
        if old != new {
            effects.push(Effect::Changed(Change::Adapter { old, new }));
        }
        effects
    }

    /// A packet from the kernel.
    pub fn on_packet(&mut self, now: Duration, packet: Packet) -> Vec<Effect> {
        self.track(|model| {
            let mut out = Vec::new();
            let Packet { index, event } = packet;
            match event {
                Event::CommandComplete { opcode: Opcode::READ_INDEX_LIST, status, data } => model.on_index_list(status, &data, &mut out),
                Event::CommandComplete { opcode: Opcode::READ_CONTROLLER_INFO, status, data } => model.on_info(index, status, &data, &mut out),
                Event::IndexAdded => out.push(Effect::Send { index, command: Command::ReadControllerInfo }),
                Event::IndexRemoved => {
                    if let Some(mut state) = model.adapters.remove(&index) {
                        state.removed(&mut out);
                    }
                }
                event => {
                    if let Some(state) = model.adapters.get_mut(&index).filter(|state| state.ready) {
                        state.on_event(now, event, &mut out);
                    }
                }
            }
            out
        })
    }

    fn on_index_list(&mut self, status: Status, data: &[u8], out: &mut Vec<Effect>) {
        if !status.is_success() {
            return out.push(Effect::Log(format!("reading the controller list failed: {status}")));
        }
        match cupertino_bluetooth::reply::parse_index_list(data) {
            Ok(indexes) => {
                for index in indexes.into_iter().filter(|index| !self.adapters.contains_key(index)) {
                    out.push(Effect::Send { index, command: Command::ReadControllerInfo });
                }
            }
            Err(error) => out.push(Effect::Log(format!("reading the controller list: {error}"))),
        }
    }

    fn on_info(&mut self, index: ControllerIndex, status: Status, data: &[u8], out: &mut Vec<Effect>) {
        if self.adapters.contains_key(&index) {
            return;
        }
        if !status.is_success() {
            return out.push(Effect::Log(format!("reading controller {} failed: {status}", index.0)));
        }
        let info = match ControllerInfo::parse(data) {
            Ok(info) => info,
            Err(error) => return out.push(Effect::Log(format!("reading controller {}: {error}", index.0))),
        };
        let adapter = Adapter { index, address: info.address, name: info.name, supported: info.supported_settings, settings: info.current_settings, class: info.class };
        let mut state = AdapterState::new(adapter);
        if self.read_only {
            state.ready = true;
        } else {
            out.push(Effect::LoadKeys { index, address: info.address });
        }
        self.adapters.insert(index, state);
    }

    /// The stored keys of an adapter that `Effect::LoadKeys` asked for. Sets the controller up.
    pub fn keys_loaded(&mut self, now: Duration, index: ControllerIndex, keys: KeySet) -> Vec<Effect> {
        self.track(|model| {
            let mut out = Vec::new();
            if let Some(state) = model.adapters.get_mut(&index) {
                state.ready = true;
            }
            model.mark_default();
            if let Some(state) = model.adapters.get_mut(&index) {
                state.bring_up(now, keys, &mut out);
            }
            out
        })
    }

    /// A request from a caller. The answer comes as an [`Effect::Complete`] with the same ticket,
    /// at once or after the controller replied.
    pub fn request(&mut self, now: Duration, ticket: Ticket, request: Request) -> Vec<Effect> {
        self.track(|model| {
            let mut out = Vec::new();
            let result = if model.read_only {
                Err(RequestError::ReadOnly)
            } else {
                match model.default_index().and_then(|index| model.adapters.get_mut(&index)) {
                    Some(state) => state.start_request(now, ticket, request, &mut out),
                    None => Err(RequestError::NoAdapter),
                }
            };
            if let Err(error) = result {
                out.push(Effect::Complete { ticket, result: Err(error) });
            }
            out
        })
    }

    /// The baseband link for a Connect could not be opened.
    pub fn link_failed(&mut self, ticket: Ticket, message: String) -> Vec<Effect> {
        let mut out = Vec::new();
        for state in self.adapters.values_mut() {
            if let Some(pending) = state.take_pending(|p| matches!(p, Pending::Connect { ticket: t, .. } if *t == ticket)) {
                state.complete(&mut out, pending, Err(RequestError::Transport(message.clone())));
            }
        }
        out
    }

    /// The agent answered, or could not.
    pub fn agent_answered(&mut self, answer: AgentAnswer) -> Vec<Effect> {
        self.track(|model| {
            let mut out = Vec::new();
            if let Some(state) = model.adapters.values_mut().find(|state| state.pairing.as_ref().is_some_and(|pairing| pairing.id == answer.pairing)) {
                state.on_agent_answer(answer, &mut out);
            }
            out
        })
    }

    /// Time passed.
    pub fn tick(&mut self, now: Duration) -> Vec<Effect> {
        self.track(|model| {
            let mut out = Vec::new();
            for state in model.adapters.values_mut().filter(|state| state.ready) {
                state.tick(now, &mut out);
            }
            out
        })
    }

    /// When [`Model::tick`] next has something to do.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.adapters.values().filter(|state| state.ready).filter_map(AdapterState::next_deadline).min()
    }
}
