//! A stand-in for the kernel's management interface: one powered or unpowered controller that
//! answers the commands the daemon sends, finds a fixed set of nearby devices and pairs with them
//! by numeric comparison. Synthetic: it follows `doc/mgmt-protocol.rst`, not a recording.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use cupertino_bluetooth::{
    Command, ControllerIndex, DeviceId, Drained, Eir, Event, FoundFlags, KeyValue, LinkKey,
    LinkKeyType, Opcode, Packet, Result, Status,
};
use cupertino_btd::transport::Transport;
use rustix::io::Errno;

use super::{complete, device_reply, info_reply, C0};

/// The number the controller shows for the comparison.
pub const PASSKEY: u32 = 123_456;

struct Inner {
    queue: VecDeque<Packet>,
    settings: u32,
    sent: Vec<Command>,
    nearby: Vec<(DeviceId, Eir)>,
    closed: bool,
}

pub struct FakeController {
    inner: Mutex<Inner>,
    wake: Condvar,
}

impl FakeController {
    /// A controller with the given settings that finds `nearby` when it scans.
    pub fn new(settings: u32, nearby: Vec<(DeviceId, Eir)>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                queue: VecDeque::new(),
                settings,
                sent: Vec::new(),
                nearby,
                closed: false,
            }),
            wake: Condvar::new(),
        }
    }

    /// Every command received so far.
    pub fn sent(&self) -> Vec<Command> {
        self.inner.lock().unwrap().sent.clone()
    }

    /// Makes `receive` fail, which ends the daemon's loop.
    pub fn close(&self) {
        self.inner.lock().unwrap().closed = true;
        self.wake.notify_all();
    }
}

impl Inner {
    fn push(&mut self, index: ControllerIndex, event: Event) {
        self.queue.push_back(Packet { index, event });
    }

    fn reply(&mut self, command: &Command, data: Vec<u8>) {
        self.push(C0, complete(command.opcode(), Status::Success, data));
    }

    /// What the controller does with a command.
    fn react(&mut self, index: ControllerIndex, command: &Command) {
        match command {
            Command::ReadIndexList => self.push(
                ControllerIndex::NONE,
                complete(Opcode::READ_INDEX_LIST, Status::Success, vec![1, 0, 0, 0]),
            ),
            Command::ReadControllerInfo => {
                let info = info_reply(self.settings);
                self.reply(command, info);
            }
            Command::SetPowered(on) => {
                self.settings = if *on {
                    self.settings | 1
                } else {
                    self.settings & !1
                };
                let settings = self.settings.to_le_bytes().to_vec();
                self.reply(command, settings);
            }
            Command::SetBondable(_) | Command::SetConnectable(_) => {
                let settings = self.settings.to_le_bytes().to_vec();
                self.reply(command, settings);
            }
            Command::LoadLinkKeys(_)
            | Command::LoadLongTermKeys(_)
            | Command::SetIoCapability(_) => self.reply(command, Vec::new()),
            Command::GetConnections => self.reply(command, vec![0, 0]),
            Command::StartDiscovery(types) => {
                self.reply(command, vec![types.to_wire()]);
                self.push(
                    index,
                    Event::Discovering {
                        types: *types,
                        active: true,
                    },
                );
                for (device, eir) in self.nearby.clone() {
                    self.push(
                        index,
                        Event::DeviceFound {
                            device,
                            rssi: Some(-52),
                            flags: FoundFlags(0),
                            eir,
                        },
                    );
                }
            }
            Command::StopDiscovery(types) => {
                self.reply(command, vec![types.to_wire()]);
                self.push(
                    index,
                    Event::Discovering {
                        types: *types,
                        active: false,
                    },
                );
            }
            Command::PairDevice { device, .. } => self.push(
                index,
                Event::UserConfirmationRequest {
                    device: *device,
                    just_works: false,
                    value: PASSKEY,
                },
            ),
            Command::UserConfirmationReply {
                device,
                accept: true,
            } => {
                let key = LinkKey {
                    device: *device,
                    key_type: LinkKeyType::AuthenticatedP256,
                    value: KeyValue([0x42; 16]),
                    pin_length: 0,
                };
                self.push(index, Event::NewLinkKey { store: true, key });
                self.push(
                    index,
                    complete(Opcode::PAIR_DEVICE, Status::Success, device_reply(*device)),
                );
            }
            Command::UserConfirmationReply {
                device,
                accept: false,
            } => {
                self.push(
                    index,
                    Event::AuthenticationFailed {
                        device: *device,
                        status: Status::AuthenticationFailed,
                    },
                );
                self.push(
                    index,
                    complete(
                        Opcode::PAIR_DEVICE,
                        Status::AuthenticationFailed,
                        device_reply(*device),
                    ),
                );
            }
            other => panic!("the fake controller does not know {other:?}"),
        }
    }
}

impl Transport for FakeController {
    fn send(&self, index: ControllerIndex, command: &Command) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.sent.push(command.clone());
        inner.react(index, command);
        self.wake.notify_all();
        Ok(())
    }

    fn receive(&self, timeout: Option<Duration>) -> Result<Drained> {
        let timeout = timeout.expect("the daemon always sets a timeout");
        let mut inner = self.inner.lock().unwrap();
        if inner.queue.is_empty() && !inner.closed {
            inner = self.wake.wait_timeout(inner, timeout).unwrap().0;
        }
        if inner.closed {
            return Err(cupertino_bluetooth::Error::Syscall {
                call: "recv",
                source: Errno::PIPE,
            });
        }
        Ok(Drained {
            packets: inner.queue.drain(..).collect(),
            errors: Vec::new(),
        })
    }

    fn open_link(&self, _adapter: cupertino_bluetooth::Address, _device: DeviceId) -> Result<()> {
        panic!("the tests do not connect");
    }

    fn close_link(&self, _adapter: cupertino_bluetooth::Address, _device: DeviceId) {}
}
