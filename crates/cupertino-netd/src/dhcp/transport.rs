//! How DHCP messages travel: a trait the engine drives, and the packet socket behind it.

use std::net::Ipv4Addr;
use std::os::fd::{AsFd, BorrowedFd};

use cupertino_netlink::packet::{build_udp_packet, parse_udp_packet, PacketSocket, UdpDatagram};

use super::message::{CLIENT_PORT, SERVER_PORT};
use crate::error::Error;

/// Carries DHCP messages for one interface. Tests replace it with a recording double.
pub trait Transport {
    /// Sends `message` (an encoded DHCP message) from `source` to `destination`, port 68 to port 67.
    fn send(
        &mut self,
        source: Ipv4Addr,
        destination: Ipv4Addr,
        message: &[u8],
    ) -> Result<(), Error>;

    /// The next DHCP message a server sent (port 67 to port 68), or `None` when none is queued.
    fn receive(&mut self) -> Result<Option<Vec<u8>>, Error>;

    /// The descriptor to poll for readability, if the transport has one.
    fn as_fd(&self) -> Option<BorrowedFd<'_>>;
}

/// A packet socket on one interface (`CAP_NET_RAW`).
pub struct PacketTransport {
    socket: PacketSocket,
}

impl PacketTransport {
    pub fn open(interface_index: u32) -> Result<Self, Error> {
        Ok(Self {
            socket: PacketSocket::open(interface_index).map_err(Error::Netlink)?,
        })
    }
}

impl Transport for PacketTransport {
    fn send(
        &mut self,
        source: Ipv4Addr,
        destination: Ipv4Addr,
        message: &[u8],
    ) -> Result<(), Error> {
        let packet = build_udp_packet(&UdpDatagram {
            source,
            source_port: CLIENT_PORT,
            destination,
            destination_port: SERVER_PORT,
            payload: message.to_vec(),
        });
        self.socket.send_broadcast(&packet).map_err(Error::Netlink)
    }

    fn receive(&mut self) -> Result<Option<Vec<u8>>, Error> {
        // The socket also sees this client's own packets and everything else on IPv4: skip until a
        // datagram from a server to a client arrives.
        while let Some(packet) = self.socket.receive().map_err(Error::Netlink)? {
            // a packet that is not UDP is not for us; one that does not frame is not DHCP
            if let Ok(Some(datagram)) = parse_udp_packet(&packet) {
                if datagram.source_port == SERVER_PORT && datagram.destination_port == CLIENT_PORT {
                    return Ok(Some(datagram.payload));
                }
            }
        }
        Ok(None)
    }

    fn as_fd(&self) -> Option<BorrowedFd<'_>> {
        Some(self.socket.as_fd())
    }
}
