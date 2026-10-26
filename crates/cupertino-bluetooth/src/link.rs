//! Opening a baseband link to a device with an L2CAP socket.
//!
//! The management interface has no "connect" command: the kernel connects when a socket asks for
//! a channel. This opens the smallest channel that every device accepts without pairing and holds
//! it, so the link stays up until [`Link`] is dropped. It is not a profile: nothing is spoken on
//! the channel. For BR/EDR it is the service discovery channel, PSM 0x0001; for low energy it is
//! the attribute protocol's fixed channel, CID 0x0004 (Bluetooth Assigned Numbers, "Logical Link
//! Control and Adaptation Protocol Fixed Channels" and "Protocol/Service Multiplexer").

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::io::Errno;
use rustix::net::{bind, connect, socket_with, AddressFamily, SocketFlags, SocketType};

use crate::address::{Address, DeviceId};
use crate::error::{Error, Result};
use crate::sockaddr::SocketAddrL2;

/// The service discovery PSM.
const PSM_SDP: u16 = 0x0001;

/// The attribute protocol's fixed channel.
const CID_ATT: u16 = 0x0004;

/// An L2CAP socket that holds a link to one device open.
#[derive(Debug)]
pub struct Link {
    fd: OwnedFd,
}

impl Link {
    /// Starts a connection from the controller with address `adapter` to `device` and returns at
    /// once; the connection completes later and the kernel reports it as Device Connected or
    /// Connect Failed on the management channel.
    pub fn open(adapter: Address, device: DeviceId) -> Result<Self> {
        let syscall = |call| move |source| Error::Syscall { call, source };
        // Protocol 0 is BTPROTO_L2CAP.
        let fd = socket_with(
            AddressFamily::BLUETOOTH,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            None,
        )
        .map_err(syscall("socket(BTPROTO_L2CAP)"))?;
        // Binding the source pins the link to this controller on a host with several.
        let source = SocketAddrL2 {
            psm: 0,
            address: adapter,
            cid: 0,
            kind: device.kind,
        };
        bind(&fd, &source).map_err(syscall("bind(l2cap)"))?;
        let (psm, cid) = if device.kind.is_le() {
            (0, CID_ATT)
        } else {
            (PSM_SDP, 0)
        };
        match connect(
            &fd,
            &SocketAddrL2 {
                psm,
                address: device.address,
                cid,
                kind: device.kind,
            },
        ) {
            Ok(()) | Err(Errno::INPROGRESS) => Ok(Self { fd }),
            Err(source) => Err(Error::Syscall {
                call: "connect(l2cap)",
                source,
            }),
        }
    }
}

impl AsFd for Link {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
