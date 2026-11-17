//! Hotplug monitor: kernel uevents over a `NETLINK_KOBJECT_UEVENT` socket.
//!
//! This listens to the kernel's multicast group 1, not to udevd (group 2). The kernel broadcasts
//! an event before udevd has applied permissions and tags to the node, so a node named in an
//! `Added` event may not be accessible to the user yet. That is fine for the compositor: it opens
//! device nodes through its session (logind), which grants access itself.
//!
//! The socket is non-blocking and [`Monitor`] implements [`AsFd`], so it can be registered with an
//! event loop (calloop's `Generic` over the monitor, readable interest) and [`Monitor::drain`]
//! called when it fires.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::PathBuf;

use rustix::io::Errno;
use rustix::net::netlink::{SocketAddrNetlink, KOBJECT_UEVENT};
use rustix::net::{bind, recvfrom, socket_with, AddressFamily, RecvFlags, SocketFlags, SocketType};

use crate::dev_t::DevT;
use crate::error::{Error, ParseError, Result};
use crate::uevent::{self, Properties};

/// The kernel's uevent multicast group (udevd's own is group 2).
const KERNEL_GROUP: u32 = 1;

/// Datagram prefix of udevd's own messages on its group.
const LIBUDEV_PREFIX: &[u8] = b"libudev\0";

/// Receive buffer size. Kernel uevents are far smaller (the kernel caps the environment at 2 KiB).
const RECV_BUFFER_SIZE: usize = 16 * 1024;

/// What happened to a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `add`: the device appeared.
    Added,
    /// `remove`: the device went away.
    Removed,
    /// `change`: the device's state changed (a connector hotplug is a `change` of the card).
    Changed,
    /// Any other kernel action (`bind`, `unbind`, `move`, `online`, `offline`), verbatim.
    Other(String),
}

/// One kernel uevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEvent {
    /// What happened.
    pub action: Action,
    /// The device path below `/sys`, e.g. `/devices/pci0000:00/.../drm/card1`.
    pub devpath: String,
    /// The subsystem (`input`, `drm`, ...), from `SUBSYSTEM`.
    pub subsystem: String,
    /// The node under `/dev` (from `DEVNAME`), if the device has one.
    pub devnode: Option<PathBuf>,
    /// The device number (from `MAJOR` and `MINOR`), if the device has a node.
    pub devnum: Option<DevT>,
    /// All `KEY=VALUE` properties of the message.
    pub properties: Properties,
}

/// Parses one kernel datagram: `action@devpath`, a NUL, then NUL-terminated `KEY=VALUE` entries.
///
/// Returns `Ok(None)` for udevd's own `libudev` messages, which are not kernel events. Anything
/// else that does not parse is an error.
pub fn parse_datagram(datagram: &[u8]) -> std::result::Result<Option<DeviceEvent>, ParseError> {
    if datagram.starts_with(LIBUDEV_PREFIX) {
        return Ok(None);
    }
    let text = std::str::from_utf8(datagram).map_err(|_| ParseError::NotUtf8)?;
    // Every segment, including the last, ends in a NUL.
    let text = text.strip_suffix('\0').unwrap_or(text);
    let mut segments = text.split('\0');

    let header = segments.next().ok_or(ParseError::MissingHeader)?;
    let (action, devpath) = header.split_once('@').ok_or(ParseError::MissingHeader)?;
    let properties = uevent::collect(segments)?;

    let subsystem = properties
        .get("SUBSYSTEM")
        .ok_or(ParseError::MissingProperty("SUBSYSTEM"))?
        .clone();
    let action = match action {
        "add" => Action::Added,
        "remove" => Action::Removed,
        "change" => Action::Changed,
        other => Action::Other(other.to_owned()),
    };
    Ok(Some(DeviceEvent {
        action,
        devpath: devpath.to_owned(),
        subsystem,
        devnode: uevent::devnode(&properties),
        devnum: uevent::devnum(&properties)?,
        properties,
    }))
}

/// A bound `NETLINK_KOBJECT_UEVENT` socket.
#[derive(Debug)]
pub struct Monitor {
    fd: OwnedFd,
}

impl Monitor {
    /// Opens a non-blocking, close-on-exec socket and joins the kernel multicast group.
    pub fn new() -> Result<Self> {
        let syscall = |call| move |source| Error::Syscall { call, source };
        let fd = socket_with(
            AddressFamily::NETLINK,
            SocketType::DGRAM,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            Some(KOBJECT_UEVENT),
        )
        .map_err(syscall("socket(NETLINK_KOBJECT_UEVENT)"))?;
        // pid 0: let the kernel assign the port id.
        bind(&fd, &SocketAddrNetlink::new(0, KERNEL_GROUP)).map_err(syscall("bind"))?;
        Ok(Self { fd })
    }

    /// Reads every queued datagram and returns the events in arrival order, with what went wrong
    /// on the way (see [`Drained`]). A bad datagram never costs the good ones around it. The result
    /// is empty when the socket has nothing; `Err` only when the socket itself fails.
    ///
    /// Datagrams not sent by the kernel (port id other than 0) are dropped: any process may unicast
    /// to this socket, but only the kernel may be trusted as a source of hotplug events. udevd's
    /// `libudev` messages are dropped too.
    pub fn drain(&self) -> Result<Drained> {
        let mut drained = Drained::default();
        let mut buffer = [0u8; RECV_BUFFER_SIZE];
        loop {
            let (received, length, sender) =
                match recvfrom(&self.fd, &mut buffer[..], RecvFlags::TRUNC) {
                    Ok(datagram) => datagram,
                    Err(Errno::AGAIN) => return Ok(drained),
                    // The receive queue overflowed and the kernel dropped events. It reports this
                    // once and keeps delivering afterwards.
                    Err(Errno::NOBUFS) => {
                        drained.overflowed = true;
                        continue;
                    }
                    Err(source) => {
                        return Err(Error::Syscall {
                            call: "recvfrom",
                            source,
                        })
                    }
                };
            let from_kernel = sender
                .and_then(|address| SocketAddrNetlink::try_from(address).ok())
                .is_some_and(|address| address.pid() == 0);
            if from_kernel {
                drained.record(&buffer[..received], length > buffer.len());
            }
        }
    }
}

/// Everything one [`Monitor::drain`] read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// The kernel events that parsed, in arrival order.
    pub events: Vec<DeviceEvent>,
    /// One entry per kernel datagram that did not parse, in arrival order.
    pub errors: Vec<ParseError>,
    /// The kernel dropped events because the receive queue was full (`ENOBUFS`). The device set
    /// may have changed without a matching event: enumerate again.
    pub overflowed: bool,
}

impl Drained {
    /// Folds in one kernel datagram. `truncated` is true when it was larger than the receive buffer.
    fn record(&mut self, datagram: &[u8], truncated: bool) {
        if truncated {
            self.errors.push(ParseError::Truncated);
            return;
        }
        match parse_datagram(datagram) {
            Ok(Some(event)) => self.events.push(event),
            Ok(None) => {}
            Err(error) => self.errors.push(error),
        }
    }
}

impl AsFd for Monitor {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
