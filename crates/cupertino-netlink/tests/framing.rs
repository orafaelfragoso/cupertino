//! Netlink messages and attributes: framing, the body builder, and the typed readers.

use cupertino_netlink::attr::{attrs, NLA_F_NESTED, NLA_F_NET_BYTEORDER};
use cupertino_netlink::message::{
    align4, frame, messages, Body, HEADER_LEN, NLMSG_DONE, NLM_F_ACK, NLM_F_DUMP, NLM_F_REQUEST,
};
use cupertino_netlink::{MacAddr, ParseError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[test]
fn lengths_round_up_to_four_bytes() {
    assert_eq!(align4(0), 0);
    assert_eq!(align4(1), 4);
    assert_eq!(align4(4), 4);
    assert_eq!(align4(5), 8);
    assert_eq!(align4(17), 20);
}

#[test]
fn a_dump_request_asks_for_every_object() {
    assert_eq!(NLM_F_DUMP, 0x300);
    assert_eq!(NLM_F_REQUEST | NLM_F_ACK, 0x5);
}

#[test]
fn a_frame_has_the_header_in_front_of_the_body() {
    let message = frame(18, NLM_F_REQUEST, 7, &[1, 2, 3, 4]);
    assert_eq!(message.len(), HEADER_LEN + 4);
    let parsed = messages(&message).next().unwrap().unwrap();
    assert_eq!(parsed.header.length, 20);
    assert_eq!(parsed.header.ty, 18);
    assert_eq!(parsed.header.flags, NLM_F_REQUEST);
    assert_eq!(parsed.header.sequence, 7);
    assert_eq!(parsed.header.pid, 0);
    assert_eq!(parsed.payload, [1, 2, 3, 4]);
}

#[test]
fn several_messages_in_one_buffer_come_out_in_order() {
    let mut buffer = frame(1, 0, 1, &[0xAA; 4]);
    buffer.extend(frame(2, 0, 2, &[]));
    buffer.extend(frame(NLMSG_DONE, 0, 3, &[0; 4]));
    let all: Vec<_> = messages(&buffer).map(Result::unwrap).collect();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter().map(|m| m.header.ty).collect::<Vec<_>>(),
        [1, 2, NLMSG_DONE]
    );
    assert_eq!(all[1].payload, []);
}

#[test]
fn the_last_message_may_omit_its_padding() {
    let mut buffer = frame(1, 0, 1, &[1, 2, 3, 4]);
    // a 17-byte message: header and one byte, the three padding bytes missing
    let mut short = frame(2, 0, 2, &[9]);
    assert_eq!(short.len(), 17);
    buffer.append(&mut short);
    let all: Vec<_> = messages(&buffer).map(Result::unwrap).collect();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].payload, [9]);
}

#[test]
fn padding_between_messages_is_skipped() {
    let mut first = frame(1, 0, 1, &[7]);
    first.extend([0, 0, 0]);
    first.extend(frame(2, 0, 2, &[]));
    let all: Vec<_> = messages(&first).map(Result::unwrap).collect();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].payload, [7]);
    assert_eq!(all[1].header.ty, 2);
}

#[test]
fn an_empty_buffer_has_no_messages() {
    assert_eq!(messages(&[]).count(), 0);
}

#[test]
fn a_buffer_shorter_than_a_header_is_truncated_and_ends_the_iteration() {
    let mut iterator = messages(&[1, 2, 3]);
    assert_eq!(
        iterator.next(),
        Some(Err(ParseError::Truncated {
            what: "netlink header",
            needed: 16,
            have: 3
        }))
    );
    assert_eq!(iterator.next(), None);
}

#[test]
fn a_length_field_below_the_header_or_above_the_buffer_is_refused() {
    let mut message = frame(1, 0, 1, &[0; 4]);
    message[0..4].copy_from_slice(&8u32.to_ne_bytes());
    assert_eq!(
        messages(&message).next(),
        Some(Err(ParseError::BadMessageLength {
            length: 8,
            available: 20
        }))
    );
    message[0..4].copy_from_slice(&64u32.to_ne_bytes());
    assert_eq!(
        messages(&message).next(),
        Some(Err(ParseError::BadMessageLength {
            length: 64,
            available: 20
        }))
    );
}

#[test]
fn a_bad_message_stops_the_messages_after_it() {
    let mut buffer = frame(1, 0, 1, &[0; 4]);
    let good = frame(2, 0, 2, &[]);
    let mut bad = frame(3, 0, 3, &[]);
    bad[0..4].copy_from_slice(&999u32.to_ne_bytes());
    buffer.extend(bad);
    buffer.extend(good);
    let results: Vec<_> = messages(&buffer).collect();
    assert_eq!(results.len(), 2);
    assert!(results[0].is_ok());
    assert!(results[1].is_err());
}

#[test]
fn a_family_header_is_padded_to_four_bytes() {
    let mut body = Body::new();
    body.header(&[1, 2, 3, 4, 5]);
    assert_eq!(body.as_bytes(), [1, 2, 3, 4, 5, 0, 0, 0]);
}

#[test]
fn an_attribute_has_a_length_a_type_and_padded_data() {
    let mut body = Body::new();
    body.attr(3, &[1, 2, 3, 4, 5]);
    let mut expected = Vec::new();
    expected.extend(9u16.to_ne_bytes());
    expected.extend(3u16.to_ne_bytes());
    expected.extend([1, 2, 3, 4, 5, 0, 0, 0]);
    assert_eq!(body.as_bytes(), expected);
}

#[test]
fn string_attributes_end_in_a_nul_that_the_reader_removes() {
    let mut body = Body::new();
    body.attr_cstr(3, "wlan0");
    let bytes = body.as_bytes();
    assert_eq!(&bytes[4..10], b"wlan0\0");
    let attr = attrs(bytes).next().unwrap().unwrap();
    assert_eq!(attr.ty, 3);
    assert_eq!(attr.data, b"wlan0\0");
    assert_eq!(attr.string().unwrap(), "wlan0");
}

#[test]
fn a_string_without_a_nul_reads_whole_and_invalid_utf8_is_refused() {
    let mut body = Body::new();
    body.attr(1, b"abc").attr(2, &[0xff, 0xfe]);
    let all: Vec<_> = attrs(body.as_bytes()).map(Result::unwrap).collect();
    assert_eq!(all[0].string().unwrap(), "abc");
    assert_eq!(
        all[1].string(),
        Err(ParseError::AttributeNotUtf8 { attribute: 2 })
    );
}

#[test]
fn attributes_of_every_size_are_read_as_their_type() {
    let mut body = Body::new();
    body.attr(1, &[0xFE])
        .attr(2, &0xBEEFu16.to_ne_bytes())
        .attr_u32(3, 0xDEAD_BEEF)
        .attr(4, &(-70i32).to_ne_bytes())
        .attr(5, &[0x02, 0x00, 0x5E, 0x10, 0x20, 0x30]);
    let all: Vec<_> = attrs(body.as_bytes()).map(Result::unwrap).collect();
    assert_eq!(all.len(), 5);
    assert_eq!(all[0].u8().unwrap(), 0xFE);
    assert_eq!(all[0].i8().unwrap(), -2);
    assert_eq!(all[1].u16().unwrap(), 0xBEEF);
    assert_eq!(all[2].u32().unwrap(), 0xDEAD_BEEF);
    assert_eq!(all[3].i32().unwrap(), -70);
    assert_eq!(all[4].mac().unwrap(), [0x02, 0x00, 0x5E, 0x10, 0x20, 0x30]);
}

#[test]
fn an_attribute_of_the_wrong_size_names_its_type_and_sizes() {
    let mut body = Body::new();
    body.attr(7, &[1, 2, 3]);
    let attr = attrs(body.as_bytes()).next().unwrap().unwrap();
    let wrong = ParseError::AttributeSize {
        attribute: 7,
        expected: 4,
        actual: 3,
    };
    assert_eq!(attr.u32(), Err(wrong));
    assert_eq!(
        attr.u8(),
        Err(ParseError::AttributeSize {
            attribute: 7,
            expected: 1,
            actual: 3
        })
    );
    assert_eq!(
        attr.mac(),
        Err(ParseError::AttributeSize {
            attribute: 7,
            expected: 6,
            actual: 3
        })
    );
}

#[test]
fn an_address_attribute_is_four_or_sixteen_bytes() {
    let mut body = Body::new();
    body.attr(1, &[192, 168, 1, 7])
        .attr(2, &Ipv6Addr::LOCALHOST.octets())
        .attr(3, &[1, 2, 3]);
    let all: Vec<_> = attrs(body.as_bytes()).map(Result::unwrap).collect();
    assert_eq!(
        all[0].ip().unwrap(),
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 7))
    );
    assert_eq!(all[1].ip().unwrap(), IpAddr::V6(Ipv6Addr::LOCALHOST));
    assert_eq!(
        all[2].ip(),
        Err(ParseError::AddressSize {
            attribute: 3,
            actual: 3
        })
    );
}

#[test]
fn the_nested_and_byte_order_flags_are_removed_from_the_type() {
    let mut body = Body::new();
    body.attr(5 | NLA_F_NET_BYTEORDER, &[0, 0, 0, 1]);
    body.nested(6, |inner| {
        inner.attr_u32(1, 10).attr_u32(2, 20);
    });
    let all: Vec<_> = attrs(body.as_bytes()).map(Result::unwrap).collect();
    assert_eq!(all[0].ty, 5);
    assert_eq!(all[1].ty, 6);
    assert_eq!(NLA_F_NESTED, 1 << 15);
    let children: Vec<_> = all[1].nested().map(Result::unwrap).collect();
    assert_eq!(children.len(), 2);
    assert_eq!((children[0].ty, children[0].u32().unwrap()), (1, 10));
    assert_eq!((children[1].ty, children[1].u32().unwrap()), (2, 20));
}

#[test]
fn nests_inside_nests_are_read_level_by_level() {
    let mut body = Body::new();
    body.nested(1, |outer| {
        outer.nested(2, |inner| {
            inner.attr_cstr(3, "deep");
        });
    });
    let outer = attrs(body.as_bytes()).next().unwrap().unwrap();
    let middle = outer.nested().next().unwrap().unwrap();
    assert_eq!(middle.ty, 2);
    let inner = middle.nested().next().unwrap().unwrap();
    assert_eq!(inner.string().unwrap(), "deep");
}

#[test]
fn an_attribute_without_data_reads_as_an_empty_flag() {
    let mut body = Body::new();
    body.attr(71, &[]);
    let attr = attrs(body.as_bytes()).next().unwrap().unwrap();
    assert_eq!(attr.ty, 71);
    assert!(attr.data.is_empty());
}

#[test]
fn a_cut_attribute_header_or_length_is_refused_and_ends_the_list() {
    assert_eq!(
        attrs(&[1, 0]).next(),
        Some(Err(ParseError::Truncated {
            what: "attribute header",
            needed: 4,
            have: 2
        }))
    );
    let mut bytes = Vec::new();
    bytes.extend(40u16.to_ne_bytes());
    bytes.extend(9u16.to_ne_bytes());
    bytes.extend([0; 4]);
    let mut iterator = attrs(&bytes);
    assert_eq!(
        iterator.next(),
        Some(Err(ParseError::BadAttributeLength {
            attribute: 9,
            length: 40,
            available: 8
        }))
    );
    assert_eq!(iterator.next(), None);
    let mut small = Vec::new();
    small.extend(2u16.to_ne_bytes());
    small.extend(9u16.to_ne_bytes());
    assert!(matches!(
        attrs(&small).next(),
        Some(Err(ParseError::BadAttributeLength { length: 2, .. }))
    ));
}

#[test]
fn the_last_attribute_may_omit_its_padding() {
    let mut bytes = Vec::new();
    bytes.extend(5u16.to_ne_bytes());
    bytes.extend(1u16.to_ne_bytes());
    bytes.push(0x7F);
    let all: Vec<_> = attrs(&bytes).map(Result::unwrap).collect();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].data, [0x7F]);
}

#[test]
fn a_mac_address_prints_in_lower_case_pairs() {
    assert_eq!(
        MacAddr([0x00, 0x1A, 0x7D, 0xDA, 0x71, 0x0B]).to_string(),
        "00:1a:7d:da:71:0b"
    );
}
