//! IPv4 and UDP framing for a client that has no address yet.

use cupertino_netlink::packet::{build_udp_packet, parse_udp_packet, UdpDatagram};
use cupertino_netlink::ParseError;
use std::net::Ipv4Addr;

fn discover() -> UdpDatagram {
    UdpDatagram {
        source: Ipv4Addr::UNSPECIFIED,
        source_port: 68,
        destination: Ipv4Addr::BROADCAST,
        destination_port: 67,
        payload: vec![1, 2, 3, 4, 5],
    }
}

/// The Internet checksum of `data`: zero over a packet whose checksum field is right.
fn ones_complement_sum(initial: u32, data: &[u8]) -> u16 {
    let mut sum = initial;
    for pair in data.chunks(2) {
        sum += u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum as u16
}

/// A built `discover()` packet changed by `change`.
fn changed(change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut packet = build_udp_packet(&discover());
    change(&mut packet);
    packet
}

#[test]
fn a_built_packet_has_the_ipv4_and_udp_headers_of_a_dhcp_client() {
    let packet = build_udp_packet(&discover());
    assert_eq!(packet.len(), 20 + 8 + 5);
    for (name, range, expected) in [
        ("version and header length", 0..1, &[0x45][..]),
        ("total length", 2..4, &33u16.to_be_bytes()),
        ("don't fragment", 6..7, &[0x40]),
        ("time to live", 8..9, &[64]),
        ("UDP", 9..10, &[17]),
        ("source", 12..16, &[0, 0, 0, 0]),
        ("destination", 16..20, &[255, 255, 255, 255]),
        ("source port", 20..22, &68u16.to_be_bytes()),
        ("destination port", 22..24, &67u16.to_be_bytes()),
        ("UDP length", 24..26, &13u16.to_be_bytes()),
        ("payload", 28..33, &[1, 2, 3, 4, 5]),
    ] {
        assert_eq!(&packet[range], expected, "{name}");
    }
}

#[test]
fn the_checksums_make_the_ip_header_and_the_udp_datagram_with_its_pseudo_header_sum_to_all_ones() {
    for (source, destination, payload) in [
        (Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST, vec![]),
        (Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST, vec![9]),
        (
            Ipv4Addr::new(192, 168, 1, 2),
            Ipv4Addr::new(192, 168, 1, 1),
            vec![1, 2, 3, 4, 5],
        ),
        (Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST, vec![0xFF; 300]),
    ] {
        let packet = build_udp_packet(&UdpDatagram {
            source,
            destination,
            payload,
            ..discover()
        });
        assert_eq!(ones_complement_sum(0, &packet[..20]), 0xffff, "{source}");
        let word = |octets: [u8; 4], at: usize| {
            u32::from(u16::from_be_bytes([octets[at], octets[at + 1]]))
        };
        let pseudo = word(source.octets(), 0)
            + word(source.octets(), 2)
            + word(destination.octets(), 0)
            + word(destination.octets(), 2)
            + 17
            + (packet.len() as u32 - 20);
        assert_eq!(
            ones_complement_sum(pseudo, &packet[20..]),
            0xffff,
            "{source}"
        );
    }
}

#[test]
fn a_packet_survives_building_and_parsing_whatever_its_options_trailer_or_checksums() {
    for payload in [vec![], vec![0xAB], (0..=255).collect::<Vec<u8>>()] {
        let datagram = UdpDatagram {
            source: Ipv4Addr::new(10, 0, 0, 5),
            source_port: 1234,
            destination: Ipv4Addr::new(10, 0, 0, 1),
            destination_port: 4321,
            payload,
        };
        let packet = build_udp_packet(&datagram);
        assert_eq!(parse_udp_packet(&packet), Ok(Some(datagram)));
    }

    let plain = build_udp_packet(&discover());
    let mut options = vec![0x46];
    options.extend(&plain[1..20]);
    options.extend([1, 1, 1, 0]);
    options.extend(&plain[20..]);
    options[2..4].copy_from_slice(&(plain.len() as u16 + 4).to_be_bytes());
    for (name, packet) in [
        ("bytes after the datagram", changed(|p| p.extend([0xEE; 6]))),
        ("a header with options", options),
        (
            "wrong checksums",
            changed(|p| {
                p[10] ^= 0xFF;
                p[26] ^= 0xFF;
            }),
        ),
        ("the don't fragment flag", plain),
    ] {
        assert_eq!(parse_udp_packet(&packet), Ok(Some(discover())), "{name}");
    }
}

#[test]
fn a_packet_that_is_not_a_whole_ipv4_udp_datagram_is_not_one() {
    for (name, packet) in [
        ("TCP", changed(|p| p[9] = 6)),
        ("IPv6", changed(|p| p[0] = 0x65)),
        ("a header shorter than 20 bytes", changed(|p| p[0] = 0x44)),
        ("more fragments", changed(|p| p[6] = 0x20)),
        ("a fragment offset", changed(|p| p[7] = 0x08)),
    ] {
        assert_eq!(parse_udp_packet(&packet), Ok(None), "{name}");
    }
}

#[test]
fn a_packet_cut_short_or_with_a_udp_length_that_disagrees_is_truncated() {
    let packet = build_udp_packet(&discover());
    assert_eq!(
        parse_udp_packet(&[]),
        Err(ParseError::Truncated {
            what: "IPv4 header",
            needed: 20,
            have: 0
        })
    );
    assert_eq!(
        parse_udp_packet(&packet[..27]),
        Err(ParseError::Truncated {
            what: "IPv4 and UDP headers",
            needed: 28,
            have: 27
        })
    );
    for (name, packet) in [
        ("a cut datagram", packet[..30].to_vec()),
        (
            "a UDP length below its header",
            changed(|p| p[24..26].copy_from_slice(&4u16.to_be_bytes())),
        ),
        (
            "a UDP length past the packet",
            changed(|p| p[24..26].copy_from_slice(&200u16.to_be_bytes())),
        ),
    ] {
        let result = parse_udp_packet(&packet);
        assert!(
            matches!(
                result,
                Err(ParseError::Truncated {
                    what: "UDP datagram",
                    ..
                })
            ),
            "{name}: {result:?}"
        );
    }
}
