//! A packet socket (`AF_PACKET`) for IPv4 on one interface, and the IPv4/UDP framing it needs.
//!
//! A client that has no address yet (DHCP) cannot use a UDP socket: it sends from 0.0.0.0 and
//! must receive replies addressed to an address it does not hold. The socket is `SOCK_DGRAM`, so
//! the kernel writes and removes the Ethernet header and the caller handles IP and UDP.
//!
//! Layouts: RFC 791 (IPv4), RFC 768 (UDP), `linux/if_packet.h` (`struct sockaddr_ll`).

use std::net::Ipv4Addr;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::io::Errno;
use rustix::net::addr::{SocketAddrArg, SocketAddrLen, SocketAddrOpaque};
use rustix::net::eth;
use rustix::net::{
    bind, recvfrom, sendto, socket_with, AddressFamily, RecvFlags, SendFlags, SocketFlags,
    SocketType,
};

use crate::error::{Error, ParseError, Result};

const IPV4_HEADER_LEN: usize = 20;
const UDP_HEADER_LEN: usize = 8;
const PROTOCOL_UDP: u8 = 17;
/// `AF_PACKET`.
const AF_PACKET: u16 = 17;
/// Largest frame received: an Ethernet MTU with room for jumbo-ish links. approximate.
const RECV_BUFFER_SIZE: usize = 4096;
/// Time to live of sent packets: 64, the default RFC 1122 recommends.
const TTL: u8 = 64;

/// A UDP datagram inside an IPv4 packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpDatagram {
    pub source: Ipv4Addr,
    pub source_port: u16,
    pub destination: Ipv4Addr,
    pub destination_port: u16,
    pub payload: Vec<u8>,
}

/// The Internet checksum (RFC 1071) of `data`, starting from `initial`.
fn checksum(initial: u32, data: &[u8]) -> u16 {
    let mut sum = initial;
    let (pairs, remainder) = data.as_chunks::<2>();
    for pair in pairs {
        sum += u32::from(u16::from_be_bytes(*pair));
    }
    if let [last] = remainder {
        sum += u32::from(*last) << 8;
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// An IPv4 packet carrying `datagram`, with both checksums filled in. The packet is not
/// fragmented (`DF` set); the caller keeps the payload below the interface MTU.
pub fn build_udp_packet(datagram: &UdpDatagram) -> Vec<u8> {
    let udp_length = UDP_HEADER_LEN + datagram.payload.len();
    let total =
        u16::try_from(IPV4_HEADER_LEN + udp_length).expect("a DHCP message is far below 64 KiB");
    let mut packet = Vec::with_capacity(usize::from(total));
    // version 4, header length 5 words; DSCP 0
    packet.extend_from_slice(&[0x45, 0]);
    packet.extend_from_slice(&total.to_be_bytes());
    // identification 0 (the packet is never fragmented); flags: don't fragment
    packet.extend_from_slice(&[0, 0, 0x40, 0]);
    packet.extend_from_slice(&[TTL, PROTOCOL_UDP, 0, 0]);
    packet.extend_from_slice(&datagram.source.octets());
    packet.extend_from_slice(&datagram.destination.octets());
    let header_checksum = checksum(0, &packet[..IPV4_HEADER_LEN]);
    packet[10..12].copy_from_slice(&header_checksum.to_be_bytes());

    let udp_length_field = u16::try_from(udp_length).expect("fits: the total fits");
    packet.extend_from_slice(&datagram.source_port.to_be_bytes());
    packet.extend_from_slice(&datagram.destination_port.to_be_bytes());
    packet.extend_from_slice(&udp_length_field.to_be_bytes());
    packet.extend_from_slice(&[0, 0]);
    packet.extend_from_slice(&datagram.payload);
    // The UDP checksum covers a pseudo header: addresses, protocol and UDP length (RFC 768).
    let mut pseudo = 0u32;
    for address in [datagram.source, datagram.destination] {
        let octets = address.octets();
        pseudo += u32::from(u16::from_be_bytes([octets[0], octets[1]]))
            + u32::from(u16::from_be_bytes([octets[2], octets[3]]));
    }
    pseudo += u32::from(PROTOCOL_UDP) + u32::from(udp_length_field);
    // a computed 0 is sent as all ones (RFC 768)
    let udp_checksum = match checksum(pseudo, &packet[IPV4_HEADER_LEN..]) {
        0 => 0xffff,
        sum => sum,
    };
    packet[IPV4_HEADER_LEN + 6..IPV4_HEADER_LEN + 8].copy_from_slice(&udp_checksum.to_be_bytes());
    packet
}

/// Reads an IPv4 packet. `Ok(None)` for a packet that is not an unfragmented UDP datagram.
///
/// Checksums are not verified: the kernel hands packets of the local host and of offloading
/// network cards over with the checksum not yet computed.
pub fn parse_udp_packet(packet: &[u8]) -> std::result::Result<Option<UdpDatagram>, ParseError> {
    let Some(&first) = packet.first() else {
        return Err(ParseError::Truncated {
            what: "IPv4 header",
            needed: IPV4_HEADER_LEN,
            have: 0,
        });
    };
    let header_len = usize::from(first & 0x0f) * 4;
    if first >> 4 != 4 || header_len < IPV4_HEADER_LEN {
        return Ok(None);
    }
    if packet.len() < header_len + UDP_HEADER_LEN {
        return Err(ParseError::Truncated {
            what: "IPv4 and UDP headers",
            needed: header_len + UDP_HEADER_LEN,
            have: packet.len(),
        });
    }
    let total = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    let fragment = u16::from_be_bytes([packet[6], packet[7]]) & 0x3fff;
    if packet[9] != PROTOCOL_UDP || fragment != 0 {
        return Ok(None);
    }
    let udp = &packet[header_len..];
    let udp_length = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
    if total > packet.len() || udp_length < UDP_HEADER_LEN || header_len + udp_length > total {
        return Err(ParseError::Truncated {
            what: "UDP datagram",
            needed: header_len + udp_length.max(UDP_HEADER_LEN),
            have: total.min(packet.len()),
        });
    }
    Ok(Some(UdpDatagram {
        source: Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]),
        source_port: u16::from_be_bytes([udp[0], udp[1]]),
        destination: Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]),
        destination_port: u16::from_be_bytes([udp[2], udp[3]]),
        payload: udp[UDP_HEADER_LEN..udp_length].to_vec(),
    }))
}

/// `struct sockaddr_ll`.
#[repr(C)]
struct LinkAddress {
    family: u16,
    /// Big endian.
    protocol: u16,
    interface_index: i32,
    hardware_type: u16,
    packet_type: u8,
    address_length: u8,
    address: [u8; 8],
}

impl LinkAddress {
    /// The IPv4 ethertype on one interface, with `mac` as the link-layer destination.
    fn ipv4(interface_index: u32, mac: [u8; 6]) -> Self {
        let mut address = [0u8; 8];
        address[..6].copy_from_slice(&mac);
        Self {
            family: AF_PACKET,
            protocol: 0x0800u16.to_be(),
            interface_index: i32::try_from(interface_index)
                .expect("interface indexes are positive 31-bit numbers"),
            hardware_type: 0,
            packet_type: 0,
            address_length: 6,
            address,
        }
    }
}

// SAFETY: `with_sockaddr` calls `f` with a pointer to a fully initialised `sockaddr_ll` that
// outlives the call, and its size.
unsafe impl SocketAddrArg for LinkAddress {
    unsafe fn with_sockaddr<R>(
        &self,
        f: impl FnOnce(*const SocketAddrOpaque, SocketAddrLen) -> R,
    ) -> R {
        f(
            (self as *const Self).cast(),
            size_of::<Self>() as SocketAddrLen,
        )
    }
}

/// A non-blocking packet socket bound to IPv4 on one interface.
#[derive(Debug)]
pub struct PacketSocket {
    fd: OwnedFd,
    interface_index: u32,
}

impl PacketSocket {
    /// Opens the socket. Needs `CAP_NET_RAW`.
    pub fn open(interface_index: u32) -> Result<Self> {
        let fd = socket_with(
            AddressFamily::PACKET,
            SocketType::DGRAM,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            Some(eth::IP),
        )
        .map_err(Error::syscall("socket(AF_PACKET)"))?;
        bind(&fd, &LinkAddress::ipv4(interface_index, [0; 6]))
            .map_err(Error::syscall("bind(AF_PACKET)"))?;
        Ok(Self {
            fd,
            interface_index,
        })
    }

    /// Sends an IPv4 packet in an Ethernet broadcast frame. The frame is a broadcast even when
    /// the IP destination is a unicast address: a client that has no ARP entry for its server
    /// still reaches it.
    pub fn send_broadcast(&self, ip_packet: &[u8]) -> Result<()> {
        sendto(
            &self.fd,
            ip_packet,
            SendFlags::empty(),
            &LinkAddress::ipv4(self.interface_index, [0xff; 6]),
        )
        .map(drop)
        .map_err(Error::syscall("sendto(AF_PACKET)"))
    }

    /// The next IPv4 packet, without its Ethernet header; `None` when there is none queued.
    /// Packets this socket sent are looped back to it too: filter by port.
    pub fn receive(&self) -> Result<Option<Vec<u8>>> {
        let mut buffer = vec![0u8; RECV_BUFFER_SIZE];
        loop {
            match recvfrom(&self.fd, &mut buffer[..], RecvFlags::TRUNC) {
                Ok((received, length, _)) if length <= buffer.len() => {
                    buffer.truncate(received);
                    return Ok(Some(buffer));
                }
                // a frame larger than the buffer is no DHCP message
                Ok(_) => continue,
                Err(Errno::AGAIN) => return Ok(None),
                Err(Errno::INTR) => continue,
                Err(source) => {
                    return Err(Error::Syscall {
                        call: "recvfrom(AF_PACKET)",
                        source,
                    })
                }
            }
        }
    }
}

impl AsFd for PacketSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discover() -> UdpDatagram {
        UdpDatagram {
            source: Ipv4Addr::UNSPECIFIED,
            source_port: 68,
            destination: Ipv4Addr::BROADCAST,
            destination_port: 67,
            payload: vec![1, 2, 3],
        }
    }

    #[test]
    fn builds_the_headers_of_a_broadcast() {
        let packet = build_udp_packet(&discover());
        assert_eq!(
            packet[..28],
            [
                // version/IHL, DSCP, total length 31, identification, DF, TTL 64, UDP, checksum
                0x45, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0x3a, 0xcf,
                // 0.0.0.0 -> 255.255.255.255
                0, 0, 0, 0, 255, 255, 255, 255,
                // UDP 68 -> 67, length 11, checksum (both checksums cross-checked with an independent RFC 1071 implementation)
                0x00, 0x44, 0x00, 0x43, 0x00, 0x0b, 0xfb, 0x4f,
            ]
        );
        assert_eq!(packet[28..], [1, 2, 3][..]);
    }

    #[test]
    fn a_packet_that_was_built_parses_back() {
        let datagram = discover();
        assert_eq!(
            parse_udp_packet(&build_udp_packet(&datagram)),
            Ok(Some(datagram))
        );
    }

    #[test]
    fn the_header_checksum_of_a_built_packet_verifies_to_zero() {
        let packet = build_udp_packet(&discover());
        assert_eq!(checksum(0, &packet[..IPV4_HEADER_LEN]), 0);
    }

    #[test]
    fn tcp_and_fragments_are_not_datagrams() {
        let mut packet = build_udp_packet(&discover());
        packet[9] = 6;
        assert_eq!(parse_udp_packet(&packet), Ok(None));
        let mut fragment = build_udp_packet(&discover());
        fragment[6] = 0x20;
        assert_eq!(parse_udp_packet(&fragment), Ok(None));
    }

    #[test]
    fn a_length_past_the_packet_is_an_error() {
        let mut packet = build_udp_packet(&discover());
        packet[24..26].copy_from_slice(&200u16.to_be_bytes());
        assert!(matches!(
            parse_udp_packet(&packet),
            Err(ParseError::Truncated {
                what: "UDP datagram",
                ..
            })
        ));
    }

    #[test]
    fn a_link_address_is_twenty_bytes() {
        assert_eq!(size_of::<LinkAddress>(), 20);
    }
}
