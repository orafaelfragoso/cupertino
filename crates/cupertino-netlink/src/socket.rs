//! A netlink socket: requests with their replies, and multicast events.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::time::Duration;

use rustix::io::Errno;
use rustix::net::netlink::SocketAddrNetlink;
use rustix::net::sockopt::{set_socket_timeout, Timeout};
use rustix::net::{bind, recvfrom, sendto, socket_with, AddressFamily, Protocol, RecvFlags, SendFlags, SocketFlags, SocketType};

use crate::error::{Error, ParseError, Result};
use crate::message::{frame, messages, NLMSG_DONE, NLMSG_ERROR, NLM_F_ACK, NLM_F_DUMP, NLM_F_REQUEST};

/// Receive buffer size. A dump message is limited to a page by the kernel; a generic netlink
/// reply can be larger (a wiphy description), so this is generous. approximate.
const RECV_BUFFER_SIZE: usize = 64 * 1024;

/// How long a request waits for the kernel before failing. The kernel answers a request
/// immediately; this only bounds a wedged kernel. approximate.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether reads wait for data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// For request sockets: reads wait up to [`REQUEST_TIMEOUT`].
    Blocking,
    /// For event sockets that are registered with an event loop.
    NonBlocking,
}

/// Everything one [`Socket::drain`] read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// The datagrams in arrival order; each holds one or more netlink messages.
    pub datagrams: Vec<Vec<u8>>,
    /// The kernel dropped messages because the receive queue was full (`ENOBUFS`). The state may
    /// have changed without a matching event: dump it again.
    pub overflowed: bool,
    /// Datagrams larger than the receive buffer, which are dropped.
    pub truncated: usize,
}

/// A bound netlink socket.
#[derive(Debug)]
pub struct Socket {
    fd: OwnedFd,
    sequence: u32,
}

impl Socket {
    /// Opens a close-on-exec socket of `protocol` (`None` is `NETLINK_ROUTE`) and joins the
    /// multicast groups in the `groups` bit mask (bit `n - 1` is group `n`).
    pub fn open(protocol: Option<Protocol>, groups: u32, mode: Mode) -> Result<Self> {
        let flags = match mode {
            Mode::Blocking => SocketFlags::CLOEXEC,
            Mode::NonBlocking => SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        };
        let fd = socket_with(AddressFamily::NETLINK, SocketType::RAW, flags, protocol).map_err(Error::syscall("socket(AF_NETLINK)"))?;
        // port id 0: the kernel assigns one.
        bind(&fd, &SocketAddrNetlink::new(0, groups)).map_err(Error::syscall("bind"))?;
        if mode == Mode::Blocking {
            set_socket_timeout(&fd, Timeout::Recv, Some(REQUEST_TIMEOUT)).map_err(Error::syscall("setsockopt(SO_RCVTIMEO)"))?;
        }
        Ok(Self { fd, sequence: 0 })
    }

    /// Sends a request and returns the raw reply: every datagram up to and including the
    /// acknowledgement (a request) or `NLMSG_DONE` (a dump), concatenated. A refusal by the
    /// kernel is an [`Error::Kernel`] named `request`.
    ///
    /// The socket must be [`Mode::Blocking`].
    pub fn call(&mut self, request: &'static str, ty: u16, flags: u16, body: &[u8]) -> Result<Vec<u8>> {
        self.sequence = self.sequence.wrapping_add(1);
        let sequence = self.sequence;
        let is_dump = flags & NLM_F_DUMP == NLM_F_DUMP;
        // A dump ends with DONE; anything else asks for an acknowledgement so the end is known.
        let flags = flags | NLM_F_REQUEST | if is_dump { 0 } else { NLM_F_ACK };
        let message = frame(ty, flags, sequence, body);
        sendto(&self.fd, &message, SendFlags::empty(), &SocketAddrNetlink::new(0, 0)).map_err(Error::syscall("sendto"))?;

        let mut reply = Vec::new();
        let mut buffer = vec![0u8; RECV_BUFFER_SIZE];
        loop {
            let length = loop {
                match recv_datagram(&self.fd, &mut buffer) {
                    Ok(Some(length)) => break length,
                    Ok(None) => continue,
                    Err(Errno::INTR) => continue,
                    Err(source) => return Err(Error::Syscall { call: "recvfrom", source }),
                }
            };
            let datagram = &buffer[..length];
            let mut finished = false;
            for message in messages(datagram) {
                let message = message.map_err(Error::parse(request))?;
                if message.header.sequence != sequence {
                    return Err(Error::UnexpectedSequence { expected: sequence, actual: message.header.sequence });
                }
                match message.header.ty {
                    NLMSG_ERROR => {
                        let errno = error_code(message.payload).map_err(Error::parse(request))?;
                        match errno {
                            0 => finished = true,
                            code => return Err(Error::Kernel { request, errno: Errno::from_raw_os_error(-code) }),
                        }
                    }
                    NLMSG_DONE => {
                        // A dump that failed half way says so in the DONE payload.
                        match error_code(message.payload).map_err(Error::parse(request))? {
                            0 => finished = true,
                            code => return Err(Error::Kernel { request, errno: Errno::from_raw_os_error(-code) }),
                        }
                    }
                    _ => {}
                }
            }
            reply.extend_from_slice(datagram);
            if finished {
                return Ok(reply);
            }
        }
    }

    /// Reads every queued datagram sent by the kernel. Returns at once when there is none.
    ///
    /// Datagrams not sent by the kernel (port id other than 0) are dropped: any process may
    /// unicast to this socket, but only the kernel may be trusted for network state.
    pub fn drain(&self) -> Result<Drained> {
        let mut drained = Drained::default();
        let mut buffer = vec![0u8; RECV_BUFFER_SIZE];
        loop {
            match recvfrom(&self.fd, &mut buffer[..], RecvFlags::TRUNC) {
                Ok((received, length, sender)) => {
                    let from_kernel = sender.and_then(|address| SocketAddrNetlink::try_from(address).ok()).is_some_and(|address| address.pid() == 0);
                    if !from_kernel {
                        continue;
                    }
                    if length > buffer.len() {
                        drained.truncated += 1;
                    } else {
                        drained.datagrams.push(buffer[..received].to_vec());
                    }
                }
                Err(Errno::AGAIN) => return Ok(drained),
                Err(Errno::INTR) => continue,
                // The receive queue overflowed; the kernel reports it once and keeps delivering.
                Err(Errno::NOBUFS) => drained.overflowed = true,
                Err(source) => return Err(Error::Syscall { call: "recvfrom", source }),
            }
        }
    }
}

impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// Reads one datagram into `buffer` and returns its length. `Ok(None)` is a datagram that a
/// process sent (not the kernel) and was dropped. A datagram larger than the buffer is `EMSGSIZE`.
fn recv_datagram(fd: &OwnedFd, buffer: &mut [u8]) -> std::result::Result<Option<usize>, Errno> {
    let (received, length, sender) = recvfrom(fd, &mut buffer[..], RecvFlags::TRUNC)?;
    if length > buffer.len() {
        return Err(Errno::MSGSIZE);
    }
    let from_kernel = sender.and_then(|address| SocketAddrNetlink::try_from(address).ok()).is_some_and(|address| address.pid() == 0);
    Ok(from_kernel.then_some(received))
}

/// The status code of an `NLMSG_ERROR` or `NLMSG_DONE` payload: 0, or a negative errno.
fn error_code(payload: &[u8]) -> std::result::Result<i32, ParseError> {
    let code = payload.get(..4).ok_or(ParseError::Truncated { what: "error code", needed: 4, have: payload.len() })?;
    Ok(i32::from_ne_bytes([code[0], code[1], code[2], code[3]]))
}
