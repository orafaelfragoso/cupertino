//! Shared helpers: a model driven by hand with a clock, and builders for kernel replies.
// Each test file uses a different part.

pub mod controller;

use std::time::Duration;

use cupertino_bluetooth::{
    Address, AddressType, Command, ControllerIndex, DeviceId, Eir, Event, FoundFlags, Opcode,
    Packet, Status,
};
use cupertino_btd::keys::KeySet;
use cupertino_btd::model::Model;
use cupertino_btd::request::{
    AgentAnswer, AgentCall, Change, Effect, Request, RequestError, Ticket,
};

pub const C0: ControllerIndex = ControllerIndex(0);

/// The adapter's own address, AA:BB:CC:DD:EE:01.
pub const ADAPTER: Address = Address::from_wire([0x01, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA]);

/// Bit 0 (powered), bit 7 (BR/EDR) and bit 9 (low energy) of the settings.
pub const POWERED: u32 = 0x0281;
pub const OFF: u32 = 0x0280;

/// 11:22:33:44:55:<last>, as a device of the given type.
pub fn device(last: u8, kind: AddressType) -> DeviceId {
    DeviceId {
        address: Address::from_wire([last, 0x55, 0x44, 0x33, 0x22, 0x11]),
        kind,
    }
}

/// A BR/EDR device 11:22:33:44:55:01.
pub fn phone() -> DeviceId {
    device(0x01, AddressType::BrEdr)
}

/// A BR/EDR device 11:22:33:44:55:02.
pub fn keyboard() -> DeviceId {
    device(0x02, AddressType::BrEdr)
}

/// The reply to Read Controller Information for the test adapter.
pub fn info_reply(settings: u32) -> Vec<u8> {
    let mut reply = ADAPTER.to_wire().to_vec();
    reply.push(0x0A);
    reply.extend_from_slice(&2u16.to_le_bytes());
    reply.extend_from_slice(&0x03FFu32.to_le_bytes());
    reply.extend_from_slice(&settings.to_le_bytes());
    reply.extend_from_slice(&[0x0C, 0x01, 0x7A]);
    let mut name = b"test-adapter".to_vec();
    name.resize(249, 0);
    reply.extend_from_slice(&name);
    reply.extend_from_slice(&[0u8; 11]);
    reply
}

/// The return parameters of an addressed command: address, address type.
pub fn device_reply(device: DeviceId) -> Vec<u8> {
    let mut reply = device.address.to_wire().to_vec();
    reply.push(device.kind.to_wire());
    reply
}

pub fn complete(opcode: Opcode, status: Status, data: Vec<u8>) -> Event {
    Event::CommandComplete {
        opcode,
        status,
        data,
    }
}

/// A model, the clock it is driven by, and shorthands that feed it.
pub struct Harness {
    pub model: Model,
    pub now: Duration,
    next_ticket: u64,
}

impl Harness {
    /// A model whose adapter has the given settings and no stored keys, set up and ready.
    pub fn with(settings: u32, keys: KeySet) -> (Self, Vec<Effect>) {
        let mut harness = Self {
            model: Model::new(false),
            now: Duration::from_secs(1000),
            next_ticket: 0,
        };
        assert_eq!(
            harness.model.start(),
            [Effect::Send {
                index: ControllerIndex::NONE,
                command: Command::ReadIndexList
            }]
        );
        let list = harness.model.on_packet(
            harness.now,
            Packet {
                index: ControllerIndex::NONE,
                event: complete(Opcode::READ_INDEX_LIST, Status::Success, vec![1, 0, 0, 0]),
            },
        );
        assert_eq!(
            list,
            [Effect::Send {
                index: C0,
                command: Command::ReadControllerInfo
            }]
        );
        let info = harness.packet(complete(
            Opcode::READ_CONTROLLER_INFO,
            Status::Success,
            info_reply(settings),
        ));
        assert_eq!(
            info,
            [Effect::LoadKeys {
                index: C0,
                address: ADAPTER
            }]
        );
        let effects = harness.model.keys_loaded(harness.now, C0, keys);
        (harness, effects)
    }

    /// A powered adapter with no bonds.
    pub fn powered() -> Self {
        Self::with(POWERED, KeySet::default()).0
    }

    pub fn packet(&mut self, event: Event) -> Vec<Effect> {
        self.model.on_packet(self.now, Packet { index: C0, event })
    }

    pub fn reply(&mut self, opcode: Opcode, status: Status, data: Vec<u8>) -> Vec<Effect> {
        self.packet(complete(opcode, status, data))
    }

    /// Issues a request and returns its ticket with the effects.
    pub fn request(&mut self, request: Request) -> (Ticket, Vec<Effect>) {
        self.next_ticket += 1;
        let ticket = Ticket(self.next_ticket);
        (ticket, self.model.request(self.now, ticket, request))
    }

    pub fn advance(&mut self, seconds: u64) -> Vec<Effect> {
        self.now += Duration::from_secs(seconds);
        self.model.tick(self.now)
    }

    /// A Device Found for `device` with only the given data.
    pub fn found(
        &mut self,
        device: DeviceId,
        rssi: Option<i8>,
        flags: u32,
        eir: Eir,
    ) -> Vec<Effect> {
        self.packet(Event::DeviceFound {
            device,
            rssi,
            flags: FoundFlags(flags),
            eir,
        })
    }

    pub fn answer(&mut self, answer: AgentAnswer) -> Vec<Effect> {
        self.model.agent_answered(answer)
    }

    /// Makes `device` known, as discovery would.
    pub fn discover(&mut self, device: DeviceId) {
        self.found(device, Some(-50), 0, Eir::default());
    }
}

/// The commands among the effects, in order.
pub fn sent(effects: &[Effect]) -> Vec<Command> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send { command, .. } => Some(command.clone()),
            _ => None,
        })
        .collect()
}

/// The answers to requests among the effects.
pub fn completions(effects: &[Effect]) -> Vec<(Ticket, Result<(), RequestError>)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Complete { ticket, result } => Some((*ticket, result.clone())),
            _ => None,
        })
        .collect()
}

/// The calls to the agent among the effects.
pub fn agent_calls(effects: &[Effect]) -> Vec<AgentCall> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Agent(call) => Some(call.clone()),
            _ => None,
        })
        .collect()
}

/// The changes among the effects.
pub fn changes(effects: &[Effect]) -> Vec<Change> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Changed(change) => Some(change.clone()),
            _ => None,
        })
        .collect()
}

/// The one change among the effects.
pub fn only_change(effects: &[Effect]) -> Change {
    let mut all = changes(effects);
    assert_eq!(all.len(), 1, "expected exactly one change, got {all:?}");
    all.remove(0)
}
