//! Netlink messages and attributes: framing, the body builder, and the typed readers.

use cupertino_netlink::attr::{attrs, NLA_F_NET_BYTEORDER};
use cupertino_netlink::message::{align4, frame, messages, Body, HEADER_LEN, NLMSG_DONE, NLM_F_REQUEST};
use cupertino_netlink::{MacAddr, ParseError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[test]
fn lengths_round_up_to_four_bytes() {
    for (length, expected) in [(0, 0), (1, 4), (4, 4), (5, 8), (17, 20)] {
        assert_eq!(align4(length), expected, "{length}");
    }
}

#[test]
fn a_frame_is_a_header_in_front_of_its_body() {
    let message = frame(18, NLM_F_REQUEST, 7, &[1, 2, 3, 4]);
    assert_eq!(message.len(), HEADER_LEN + 4);
    let parsed = messages(&message).next().unwrap().unwrap();
    assert_eq!(
        (
            parsed.header.length,
            parsed.header.ty,
            parsed.header.flags,
            parsed.header.sequence,
            parsed.header.pid
        ),
        (20, 18, NLM_F_REQUEST, 7, 0)
    );
    assert_eq!(parsed.payload, [1, 2, 3, 4]);
}

#[test]
fn a_buffer_gives_its_messages_in_order_skipping_padding_that_the_last_may_omit() {
    let three = [
        frame(1, 0, 1, &[0xAA; 4]),
        frame(2, 0, 2, &[]),
        frame(NLMSG_DONE, 0, 3, &[0; 4]),
    ]
    .concat();
    // a 17-byte last message: header and one byte, the three padding bytes missing
    let unpadded_last = [frame(1, 0, 1, &[1, 2, 3, 4]), frame(2, 0, 2, &[9])].concat();
    let padded_between = [frame(1, 0, 1, &[7]), vec![0, 0, 0], frame(2, 0, 2, &[])].concat();
    for (name, buffer, expected) in [
        (
            "three",
            three,
            vec![(1, vec![0xAA; 4]), (2, vec![]), (NLMSG_DONE, vec![0; 4])],
        ),
        (
            "the last unpadded",
            unpadded_last,
            vec![(1, vec![1, 2, 3, 4]), (2, vec![9])],
        ),
        (
            "padding between",
            padded_between,
            vec![(1, vec![7]), (2, vec![])],
        ),
        ("empty", vec![], vec![]),
    ] {
        let all: Vec<_> = messages(&buffer)
            .map(|m| {
                let m = m.unwrap();
                (m.header.ty, m.payload.to_vec())
            })
            .collect();
        assert_eq!(all, expected, "{name}");
    }
}

#[test]
fn a_bad_message_is_refused_and_ends_the_messages() {
    let mut below = frame(1, 0, 1, &[0; 4]);
    below[0..4].copy_from_slice(&8u32.to_ne_bytes());
    let mut above = frame(1, 0, 1, &[0; 4]);
    above[0..4].copy_from_slice(&64u32.to_ne_bytes());
    let mut bad = frame(3, 0, 3, &[]);
    bad[0..4].copy_from_slice(&999u32.to_ne_bytes());
    let after_good = [frame(1, 0, 1, &[0; 4]), bad, frame(2, 0, 2, &[])].concat();
    for (name, buffer, good, error) in [
        (
            "shorter than a header",
            vec![1, 2, 3],
            0,
            ParseError::Truncated {
                what: "netlink header",
                needed: 16,
                have: 3,
            },
        ),
        (
            "a length below the header",
            below,
            0,
            ParseError::BadMessageLength {
                length: 8,
                available: 20,
            },
        ),
        (
            "a length above the buffer",
            above,
            0,
            ParseError::BadMessageLength {
                length: 64,
                available: 20,
            },
        ),
        (
            "after a good one",
            after_good,
            1,
            ParseError::BadMessageLength {
                length: 999,
                available: 32,
            },
        ),
    ] {
        let results: Vec<_> = messages(&buffer).collect();
        assert_eq!(results.len(), good + 1, "{name}");
        assert!(results[..good].iter().all(Result::is_ok), "{name}");
        assert_eq!(results[good], Err(error), "{name}");
    }
}

#[test]
fn the_body_builder_pads_the_family_header_and_each_attribute_and_ends_strings_in_a_nul() {
    let mut body = Body::new();
    body.header(&[1, 2, 3, 4, 5]);
    assert_eq!(body.as_bytes(), [1, 2, 3, 4, 5, 0, 0, 0]);

    let mut body = Body::new();
    body.attr(3, &[1, 2, 3, 4, 5]).attr_cstr(4, "wlan0");
    let expected = [
        &9u16.to_ne_bytes()[..],
        &3u16.to_ne_bytes(),
        &[1, 2, 3, 4, 5, 0, 0, 0],
        &10u16.to_ne_bytes(),
        &4u16.to_ne_bytes(),
        b"wlan0\0",
        &[0, 0],
    ]
    .concat();
    assert_eq!(body.as_bytes(), expected);
}

#[test]
fn attributes_are_read_as_their_type() {
    let mut body = Body::new();
    body.attr(1, &[0xFE])
        .attr(2, &0xBEEFu16.to_ne_bytes())
        .attr_u32(3, 0xDEAD_BEEF)
        .attr(4, &(-70i32).to_ne_bytes())
        .attr(5, &[0x02, 0x00, 0x5E, 0x10, 0x20, 0x30])
        .attr(6, &[192, 168, 1, 7])
        .attr(7, &Ipv6Addr::LOCALHOST.octets())
        .attr_cstr(8, "wlan0")
        .attr(9, b"abc")
        .attr(71, &[]);
    let all: Vec<_> = attrs(body.as_bytes()).map(Result::unwrap).collect();
    assert_eq!(
        all.iter().map(|a| a.ty).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 71]
    );
    assert_eq!(all[0].u8(), Ok(0xFE));
    assert_eq!(all[0].i8(), Ok(-2));
    assert_eq!(all[1].u16(), Ok(0xBEEF));
    assert_eq!(all[2].u32(), Ok(0xDEAD_BEEF));
    assert_eq!(all[3].i32(), Ok(-70));
    assert_eq!(all[4].mac(), Ok([0x02, 0x00, 0x5E, 0x10, 0x20, 0x30]));
    assert_eq!(all[5].ip(), Ok(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 7))));
    assert_eq!(all[6].ip(), Ok(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    assert_eq!(all[7].data, b"wlan0\0");
    assert_eq!(all[7].string().unwrap(), "wlan0", "the reader removes the nul");
    assert_eq!(all[8].string().unwrap(), "abc", "a string without a nul");
    assert!(all[9].data.is_empty(), "an empty flag");
    assert_eq!(
        MacAddr([0x00, 0x1A, 0x7D, 0xDA, 0x71, 0x0B]).to_string(),
        "00:1a:7d:da:71:0b"
    );
}

#[test]
fn an_attribute_of_the_wrong_size_or_text_is_refused_naming_its_type() {
    let mut body = Body::new();
    body.attr(7, &[1, 2, 3]).attr(2, &[0xff, 0xfe]);
    let all: Vec<_> = attrs(body.as_bytes()).map(Result::unwrap).collect();
    let size = |expected| ParseError::AttributeSize {
        attribute: 7,
        expected,
        actual: 3,
    };
    assert_eq!(all[0].u8().map(drop), Err(size(1)));
    assert_eq!(all[0].u32().map(drop), Err(size(4)));
    assert_eq!(all[0].mac().map(drop), Err(size(6)));
    assert_eq!(
        all[0].ip(),
        Err(ParseError::AddressSize {
            attribute: 7,
            actual: 3
        })
    );
    assert_eq!(
        all[1].string(),
        Err(ParseError::AttributeNotUtf8 { attribute: 2 })
    );
}

#[test]
fn nested_attributes_are_read_level_by_level_with_the_flags_removed_from_the_type() {
    let mut body = Body::new();
    body.attr(5 | NLA_F_NET_BYTEORDER, &[0, 0, 0, 1]);
    body.nested(6, |inner| {
        inner.attr_u32(1, 10).attr_u32(2, 20);
        inner.nested(3, |deeper| {
            deeper.attr_cstr(4, "deep");
        });
    });
    let all: Vec<_> = attrs(body.as_bytes()).map(Result::unwrap).collect();
    assert_eq!((all[0].ty, all[1].ty), (5, 6));
    let children: Vec<_> = all[1].nested().map(Result::unwrap).collect();
    assert_eq!(children.len(), 3);
    assert_eq!((children[0].ty, children[0].u32()), (1, Ok(10)));
    assert_eq!((children[1].ty, children[1].u32()), (2, Ok(20)));
    assert_eq!(children[2].ty, 3);
    let deep = children[2].nested().next().unwrap().unwrap();
    assert_eq!(deep.string().unwrap(), "deep");
}

#[test]
fn an_attribute_list_reads_an_unpadded_last_attribute_and_refuses_a_cut_one() {
    let unpadded = [&5u16.to_ne_bytes()[..], &1u16.to_ne_bytes(), &[0x7F]].concat();
    let all: Vec<_> = attrs(&unpadded).map(Result::unwrap).collect();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].data, [0x7F]);

    let long = [&40u16.to_ne_bytes()[..], &9u16.to_ne_bytes(), &[0; 4]].concat();
    let small = [&2u16.to_ne_bytes()[..], &9u16.to_ne_bytes()].concat();
    for (name, bytes, error) in [
        (
            "a cut header",
            vec![1, 0],
            ParseError::Truncated {
                what: "attribute header",
                needed: 4,
                have: 2,
            },
        ),
        (
            "a length past the list",
            long,
            ParseError::BadAttributeLength {
                attribute: 9,
                length: 40,
                available: 8,
            },
        ),
        (
            "a length below the header",
            small,
            ParseError::BadAttributeLength {
                attribute: 9,
                length: 2,
                available: 4,
            },
        ),
    ] {
        let mut iterator = attrs(&bytes);
        assert_eq!(iterator.next().map(|r| r.map(drop)), Some(Err(error)), "{name}");
        assert!(iterator.next().is_none(), "{name}");
    }
}
