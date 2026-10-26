//! The management control channel: an `AF_BLUETOOTH` raw socket of protocol `BTPROTO_HCI` bound
//! to `HCI_CHANNEL_CONTROL`.
//!
//! Each `send` carries one command and each `recv` returns one event, so no framing is needed.
//! The socket is non-blocking and [`AsFd`], so a caller waits for readability with `poll` and then
//! calls [`ControlSocket::drain`].
//!
//! Privilege: any process may open and bind the channel, but the kernel marks a socket without
//! `CAP_NET_ADMIN` untrusted. An untrusted socket may send only the read commands (version,
//! supported commands, index lists, controller information) and receives only the events about
//! controllers and their settings; every other command is answered with Permission Denied.

use std::num::NonZeroU32;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::io::Errno;
use rustix::net::{bind, recv, send, socket_with, AddressFamily, Protocol, RecvFlags, SendFlags, SocketFlags, SocketType};

use crate::command::{Command, ControllerIndex};
use crate::error::{Error, ParseError, Result};
use crate::event::{decode_packet, Packet};
use crate::sockaddr::SocketAddrControl;

/// `BTPROTO_HCI` (`include/net/bluetooth/bluetooth.h`).
const BTPROTO_HCI: u32 = 1;

/// The largest packet: a 6-byte header and 65535 parameter bytes.
const MAX_PACKET: usize = 6 + 65535;

/// An open management control channel.
#[derive(Debug)]
pub struct ControlSocket {
    fd: OwnedFd,
}

/// What one [`ControlSocket::drain`] read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// The packets that decoded, in arrival order.
    pub packets: Vec<Packet>,
    /// One entry per packet that did not decode, in arrival order.
    pub errors: Vec<ParseError>,
}

impl ControlSocket {
    /// Opens a non-blocking, close-on-exec socket and binds it to the control channel.
    pub fn open() -> Result<Self> {
        let syscall = |call| move |source| Error::Syscall { call, source };
        // BTPROTO_HCI is 1, so the protocol is never zero.
        let protocol = Protocol::from_raw(NonZeroU32::new(BTPROTO_HCI).expect("BTPROTO_HCI is 1"));
        let fd = socket_with(AddressFamily::BLUETOOTH, SocketType::RAW, SocketFlags::CLOEXEC | SocketFlags::NONBLOCK, Some(protocol)).map_err(syscall("socket(BTPROTO_HCI)"))?;
        bind(&fd, &SocketAddrControl).map_err(syscall("bind(HCI_CHANNEL_CONTROL)"))?;
        Ok(Self { fd })
    }

    /// Sends one command. A full socket queue is an error (`EAGAIN`), not a wait.
    pub fn send(&self, index: ControllerIndex, command: &Command) -> Result<()> {
        let packet = command.encode(index)?;
        send(&self.fd, &packet, SendFlags::empty()).map_err(|source| Error::Syscall { call: "send", source })?;
        Ok(())
    }

    /// Reads every queued packet, undecoded. The result is empty when the socket has nothing.
    pub fn read_raw(&self) -> Result<Vec<Vec<u8>>> {
        let mut packets = Vec::new();
        let mut buffer = vec![0u8; MAX_PACKET];
        loop {
            match recv(&self.fd, &mut buffer[..], RecvFlags::empty()) {
                Ok((received, _)) => packets.push(buffer[..received].to_vec()),
                Err(Errno::AGAIN) => return Ok(packets),
                Err(source) => return Err(Error::Syscall { call: "recv", source }),
            }
        }
    }

    /// Reads and decodes every queued packet. `Err` only when the socket itself fails; a packet
    /// that does not decode is reported in [`Drained::errors`] and costs none of the others.
    pub fn drain(&self) -> Result<Drained> {
        let mut drained = Drained::default();
        for raw in self.read_raw()? {
            match decode_packet(&raw) {
                Ok(packet) => drained.packets.push(packet),
                Err(error) => drained.errors.push(error),
            }
        }
        Ok(drained)
    }
}

impl AsFd for ControlSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
