//! rfkill (`linux/rfkill.h`): the radio kill switches, read and written through `/dev/rfkill`.
//!
//! Opening the device queues one `Added` event per switch; later events report changes. Every
//! read and write here uses the 8-byte `struct rfkill_event` (`RFKILL_EVENT_SIZE_V1`), which every
//! kernel accepts.

use std::collections::BTreeMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::{open, Mode, OFlags};
use rustix::io::{read, write, Errno};

use crate::error::{Error, ParseError, Result};

/// The size of `struct rfkill_event` (`RFKILL_EVENT_SIZE_V1`).
const EVENT_SIZE: usize = 8;

/// The device node.
pub const DEVICE: &str = "/dev/rfkill";

/// `RFKILL_TYPE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Every kind of switch (only for writes).
    All,
    Wlan,
    Bluetooth,
    Other(u8),
}

impl Kind {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::All,
            1 => Self::Wlan,
            2 => Self::Bluetooth,
            other => Self::Other(other),
        }
    }

    fn to_raw(self) -> u8 {
        match self {
            Self::All => 0,
            Self::Wlan => 1,
            Self::Bluetooth => 2,
            Self::Other(raw) => raw,
        }
    }
}

/// `RFKILL_OP_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Added,
    Deleted,
    Changed,
    /// Write only: change every switch of a kind.
    ChangedAll,
    Other(u8),
}

impl Operation {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Added,
            1 => Self::Deleted,
            2 => Self::Changed,
            3 => Self::ChangedAll,
            other => Self::Other(other),
        }
    }
}

/// One `struct rfkill_event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub index: u32,
    pub kind: Kind,
    pub operation: Operation,
    /// Blocked by software: the user, or this crate.
    pub soft_blocked: bool,
    /// Blocked by a hardware switch.
    pub hard_blocked: bool,
}

/// Decodes one event. A read can return more than 8 bytes on a kernel with a newer event layout;
/// the first 8 bytes are the same on all.
pub fn parse_event(bytes: &[u8]) -> std::result::Result<Event, ParseError> {
    let Some(bytes) = bytes.get(..EVENT_SIZE) else {
        return Err(ParseError::RfkillSize {
            actual: bytes.len(),
        });
    };
    Ok(Event {
        index: u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        kind: Kind::from_raw(bytes[4]),
        operation: Operation::from_raw(bytes[5]),
        soft_blocked: bytes[6] != 0,
        hard_blocked: bytes[7] != 0,
    })
}

/// The event that blocks or unblocks, by software, every switch of `kind`
/// (`RFKILL_OP_CHANGE_ALL`).
pub fn encode_soft_block_all(kind: Kind, blocked: bool) -> [u8; EVENT_SIZE] {
    let mut bytes = [0u8; EVENT_SIZE];
    bytes[4] = kind.to_raw();
    bytes[5] = 3;
    bytes[6] = u8::from(blocked);
    bytes
}

/// The Wi-Fi switches, folded from the event stream.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WlanSwitches {
    /// Switch index to `(soft blocked, hard blocked)`.
    switches: BTreeMap<u32, (bool, bool)>,
}

impl WlanSwitches {
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds in one event; events about other kinds of switch are ignored.
    pub fn apply(&mut self, event: &Event) {
        if event.kind != Kind::Wlan {
            return;
        }
        match event.operation {
            Operation::Added | Operation::Changed => {
                self.switches
                    .insert(event.index, (event.soft_blocked, event.hard_blocked));
            }
            Operation::Deleted => {
                self.switches.remove(&event.index);
            }
            Operation::ChangedAll | Operation::Other(_) => {}
        }
    }

    /// There is a Wi-Fi switch at all.
    pub fn is_present(&self) -> bool {
        !self.switches.is_empty()
    }

    /// Software has blocked at least one Wi-Fi switch.
    pub fn is_soft_blocked(&self) -> bool {
        self.switches.values().any(|(soft, _)| *soft)
    }

    /// A hardware switch has blocked at least one Wi-Fi switch.
    pub fn is_hard_blocked(&self) -> bool {
        self.switches.values().any(|(_, hard)| *hard)
    }
}

/// An open `/dev/rfkill`.
#[derive(Debug)]
pub struct Rfkill {
    fd: OwnedFd,
    path: PathBuf,
}

impl Rfkill {
    /// Opens `path` (normally [`DEVICE`]) for reading and writing, non-blocking.
    pub fn open(path: &Path) -> Result<Self> {
        let fd = open(
            path,
            OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|source| Error::Device {
            path: path.to_owned(),
            call: "open",
            source,
        })?;
        Ok(Self {
            fd,
            path: path.to_owned(),
        })
    }

    /// Reads every queued event. Returns at once when there is none.
    pub fn read_events(&self) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        let mut buffer = [0u8; EVENT_SIZE];
        loop {
            match read(&self.fd, &mut buffer) {
                // End of file: only a regular file (a test double) ends; the device returns EAGAIN.
                Ok(0) | Err(Errno::AGAIN) => return Ok(events),
                Ok(length) => {
                    events.push(parse_event(&buffer[..length]).map_err(Error::parse("rfkill"))?)
                }
                Err(Errno::INTR) => continue,
                Err(source) => {
                    return Err(Error::Device {
                        path: self.path.clone(),
                        call: "read",
                        source,
                    })
                }
            }
        }
    }

    /// Blocks or unblocks, by software, every switch of `kind`. Needs write access to the device.
    pub fn set_soft_block(&self, kind: Kind, blocked: bool) -> Result<()> {
        let event = encode_soft_block_all(kind, blocked);
        match write(&self.fd, &event) {
            Ok(EVENT_SIZE) => Ok(()),
            Ok(_) => Err(Error::Device {
                path: self.path.clone(),
                call: "write",
                source: Errno::IO,
            }),
            Err(source) => Err(Error::Device {
                path: self.path.clone(),
                call: "write",
                source,
            }),
        }
    }
}

impl AsFd for Rfkill {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
