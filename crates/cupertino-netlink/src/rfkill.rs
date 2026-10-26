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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_an_added_wlan_switch_that_is_soft_blocked() {
        // index 3, type WLAN (1), op ADD (0), soft 1, hard 0
        let event = parse_event(&[3, 0, 0, 0, 1, 0, 1, 0]).unwrap();
        assert_eq!(
            event,
            Event {
                index: 3,
                kind: Kind::Wlan,
                operation: Operation::Added,
                soft_blocked: true,
                hard_blocked: false
            }
        );
    }

    #[test]
    fn a_newer_longer_event_decodes_by_its_first_eight_bytes() {
        let event = parse_event(&[0, 0, 0, 0, 2, 2, 0, 1, 7]).unwrap();
        assert_eq!(
            (event.kind, event.operation, event.hard_blocked),
            (Kind::Bluetooth, Operation::Changed, true)
        );
    }

    #[test]
    fn a_short_event_is_an_error() {
        assert_eq!(
            parse_event(&[1, 2, 3]),
            Err(ParseError::RfkillSize { actual: 3 })
        );
    }

    #[test]
    fn the_block_request_names_the_kind_and_the_op() {
        // idx 0, type WLAN, op CHANGE_ALL (3), soft 1, hard 0
        assert_eq!(
            encode_soft_block_all(Kind::Wlan, true),
            [0, 0, 0, 0, 1, 3, 1, 0]
        );
        assert_eq!(
            encode_soft_block_all(Kind::Wlan, false),
            [0, 0, 0, 0, 1, 3, 0, 0]
        );
    }

    #[test]
    fn the_wlan_state_follows_the_event_stream() {
        let wlan = |index, operation, soft_blocked, hard_blocked| Event {
            index,
            kind: Kind::Wlan,
            operation,
            soft_blocked,
            hard_blocked,
        };
        let mut switches = WlanSwitches::new();
        assert!(!switches.is_present());
        switches.apply(&wlan(0, Operation::Added, false, false));
        switches.apply(&Event {
            index: 1,
            kind: Kind::Bluetooth,
            operation: Operation::Added,
            soft_blocked: true,
            hard_blocked: true,
        });
        assert!(
            switches.is_present() && !switches.is_soft_blocked() && !switches.is_hard_blocked()
        );
        switches.apply(&wlan(0, Operation::Changed, true, false));
        assert!(switches.is_soft_blocked() && !switches.is_hard_blocked());
        switches.apply(&wlan(0, Operation::Changed, true, true));
        assert!(switches.is_hard_blocked());
        switches.apply(&wlan(0, Operation::Deleted, false, false));
        assert!(!switches.is_present());
    }

    #[test]
    fn reads_events_from_a_file_and_writes_the_block_request() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rfkill");
        std::fs::write(&path, [0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 2, 1, 0]).unwrap();
        let rfkill = Rfkill::open(&path).unwrap();
        let events = rfkill.read_events().unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.index, event.operation, event.soft_blocked))
                .collect::<Vec<_>>(),
            [(0, Operation::Added, false), (1, Operation::Changed, true)]
        );
        // the file offset is at the end, so the request is appended
        rfkill.set_soft_block(Kind::Wlan, true).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap()[16..],
            [0, 0, 0, 0, 1, 3, 1, 0]
        );
    }
}
