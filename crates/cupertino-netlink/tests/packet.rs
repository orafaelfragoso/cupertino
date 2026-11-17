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

#[test]
fn a_built_packet_has_the_ipv4_and_udp_headers_of_a_dhcp_client() {
    let packet = build_udp_packet(&discover());
    assert_eq!(packet.len(), 20 + 8 + 5);
    assert_eq!(packet[0], 0x45);
    assert_eq!(packet[2..4], 33u16.to_be_bytes());
    assert_eq!(packet[6], 0x40, "don't fragment");
    assert_eq!(packet[8], 64, "time to live");
    assert_eq!(packet[9], 17, "UDP");
    assert_eq!(packet[12..16], [0, 0, 0, 0]);
    assert_eq!(packet[16..20], [255, 255, 255, 255]);
    assert_eq!(packet[20..22], 68u16.to_be_bytes());
    assert_eq!(packet[22..24], 67u16.to_be_bytes());
    assert_eq!(packet[24..26], 13u16.to_be_bytes());
    assert_eq!(packet[28..], [1, 2, 3, 4, 5]);
}

#[test]
fn the_ip_header_checksum_makes_the_header_sum_to_all_ones() {
    for payload in [vec![], vec![9], vec![1, 2, 3, 4, 5], vec![0xFF; 300]] {
        let packet = build_udp_packet(&UdpDatagram {
            payload,
            ..discover()
        });
        assert_eq!(ones_complement_sum(0, &packet[..20]), 0xffff);
    }
}

#[test]
fn the_udp_checksum_covers_the_pseudo_header() {
    let datagram = UdpDatagram {
        source: Ipv4Addr::new(192, 168, 1, 2),
        destination: Ipv4Addr::new(192, 168, 1, 1),
        ..discover()
    };
    let packet = build_udp_packet(&datagram);
    let pseudo = u32::from(u16::from_be_bytes([192, 168]))
        + u32::from(u16::from_be_bytes([1, 2]))
        + u32::from(u16::from_be_bytes([192, 168]))
        + u32::from(u16::from_be_bytes([1, 1]))
        + 17
        + (packet.len() as u32 - 20);
    assert_eq!(ones_complement_sum(pseudo, &packet[20..]), 0xffff);
}

#[test]
fn a_packet_survives_building_and_parsing() {
    for payload in [vec![], vec![0xAB], (0..=255).collect::<Vec<u8>>()] {
        let datagram = UdpDatagram {
            source: Ipv4Addr::new(10, 0, 0, 5),
            source_port: 1234,
            destination: Ipv4Addr::new(10, 0, 0, 1),
            destination_port: 4321,
            payload,
        };
        let packet = build_udp_packet(&datagram);
        assert_eq!(parse_udp_packet(&packet).unwrap(), Some(datagram));
    }
}

#[test]
fn bytes_after_the_datagram_are_ignored() {
    let mut packet = build_udp_packet(&discover());
    packet.extend([0xEE; 6]);
    assert_eq!(
        parse_udp_packet(&packet).unwrap().unwrap().payload,
        [1, 2, 3, 4, 5]
    );
}

#[test]
fn an_ip_header_with_options_is_skipped() {
    let plain = build_udp_packet(&discover());
    let mut packet = vec![0x46];
    packet.extend(&plain[1..20]);
    packet.extend([1, 1, 1, 0]);
    packet.extend(&plain[20..]);
    packet[2..4].copy_from_slice(&(plain.len() as u16 + 4).to_be_bytes());
    assert_eq!(parse_udp_packet(&packet).unwrap(), Some(discover()));
}

#[test]
fn a_packet_that_is_not_ipv4_udp_is_not_a_datagram() {
    let mut tcp = build_udp_packet(&discover());
    tcp[9] = 6;
    assert_eq!(parse_udp_packet(&tcp), Ok(None));

    let mut v6 = build_udp_packet(&discover());
    v6[0] = 0x65;
    assert_eq!(parse_udp_packet(&v6), Ok(None));

    let mut short_header = build_udp_packet(&discover());
    short_header[0] = 0x44;
    assert_eq!(parse_udp_packet(&short_header), Ok(None));
}

#[test]
fn a_fragment_is_not_a_datagram_but_a_dont_fragment_flag_is_fine() {
    let mut fragment = build_udp_packet(&discover());
    fragment[6] = 0x20;
    assert_eq!(parse_udp_packet(&fragment), Ok(None));
    let mut offset = build_udp_packet(&discover());
    offset[7] = 0x08;
    assert_eq!(parse_udp_packet(&offset), Ok(None));
    assert!(parse_udp_packet(&build_udp_packet(&discover()))
        .unwrap()
        .is_some());
}

#[test]
fn a_packet_cut_short_is_truncated() {
    assert_eq!(
        parse_udp_packet(&[]),
        Err(ParseError::Truncated {
            what: "IPv4 header",
            needed: 20,
            have: 0
        })
    );
    let packet = build_udp_packet(&discover());
    assert_eq!(
        parse_udp_packet(&packet[..27]),
        Err(ParseError::Truncated {
            what: "IPv4 and UDP headers",
            needed: 28,
            have: 27
        })
    );
    assert!(matches!(
        parse_udp_packet(&packet[..30]),
        Err(ParseError::Truncated {
            what: "UDP datagram",
            ..
        })
    ));
}

#[test]
fn a_udp_length_that_disagrees_with_the_packet_is_refused() {
    let mut too_small = build_udp_packet(&discover());
    too_small[24..26].copy_from_slice(&4u16.to_be_bytes());
    assert!(matches!(
        parse_udp_packet(&too_small),
        Err(ParseError::Truncated {
            what: "UDP datagram",
            ..
        })
    ));
    let mut too_large = build_udp_packet(&discover());
    too_large[24..26].copy_from_slice(&200u16.to_be_bytes());
    assert!(matches!(
        parse_udp_packet(&too_large),
        Err(ParseError::Truncated {
            what: "UDP datagram",
            ..
        })
    ));
}

#[test]
fn the_checksums_are_not_verified() {
    let mut packet = build_udp_packet(&discover());
    packet[10] ^= 0xFF;
    packet[26] ^= 0xFF;
    assert_eq!(parse_udp_packet(&packet), Ok(Some(discover())));
}
