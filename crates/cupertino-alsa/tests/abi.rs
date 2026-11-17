//! Decoding of the kernel's control structures and encoding of an element id.

use cupertino_alsa::abi::{CARD_INFO_SIZE, ELEM_ID_SIZE, ELEM_INFO_SIZE, EVENT_SIZE};
use cupertino_alsa::{
    Access, CardInfo, ElementId, ElementInfo, ElementType, Error, Event, Interface,
};

fn put_u32(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_ne_bytes());
}

fn put_i64(buf: &mut [u8], at: usize, value: i64) {
    buf[at..at + 8].copy_from_slice(&value.to_ne_bytes());
}

fn put_text(buf: &mut [u8], at: usize, text: &str) {
    buf[at..at + text.len()].copy_from_slice(text.as_bytes());
}

fn master_id() -> ElementId {
    ElementId {
        numid: 7,
        interface: Interface::Mixer,
        device: 1,
        subdevice: 2,
        name: "Master Playback Volume".to_owned(),
        index: 3,
    }
}

#[test]
fn an_element_id_survives_encoding_and_parsing_with_the_rest_of_its_name_cleared() {
    for name in ["Master Playback Volume", "Mic", &"n".repeat(43)] {
        let id = ElementId {
            name: name.to_owned(),
            ..master_id()
        };
        let mut buf = [0xffu8; ELEM_ID_SIZE];
        id.encode(&mut buf).unwrap();
        assert_eq!(&buf[16..16 + name.len()], name.as_bytes());
        assert!(buf[16 + name.len()..60].iter().all(|&b| b == 0), "{name}");
        assert_eq!(ElementId::parse(&buf).unwrap(), id, "{name}");
    }
}

#[test]
fn a_name_too_long_for_its_nul_or_a_field_that_is_not_utf8_is_refused() {
    let mut buf = [0u8; ELEM_ID_SIZE];
    let too_long = ElementId {
        name: "n".repeat(44),
        ..master_id()
    };
    assert!(matches!(
        too_long.encode(&mut buf),
        Err(Error::NameTooLong { len: 44 })
    ));

    let mut id = [0u8; ELEM_ID_SIZE];
    id[16..18].copy_from_slice(&[0xff, 0xfe]);
    let mut card = [0u8; CARD_INFO_SIZE];
    card[40] = 0xc0;
    for (name, result) in [
        ("element id", ElementId::parse(&id).map(drop)),
        ("card info", CardInfo::parse(&card).map(drop)),
    ] {
        assert!(
            matches!(result, Err(Error::NotUtf8 { field: "name" })),
            "{name}: {result:?}"
        );
    }
}

#[test]
fn interfaces_and_types_map_to_the_kernel_numbers_and_keep_unknown_ones() {
    for raw in 0..=6 {
        assert_eq!(Interface::from_raw(raw).raw(), raw);
    }
    for (raw, expected) in [(2, Interface::Mixer), (99, Interface::Other(99))] {
        assert_eq!(Interface::from_raw(raw), expected, "{raw}");
    }
    assert_eq!(Interface::Other(99).raw(), 99);
    for (raw, expected) in [
        (1, ElementType::Boolean),
        (2, ElementType::Integer),
        (6, ElementType::Integer64),
        (42, ElementType::Other(42)),
    ] {
        assert_eq!(ElementType::from_raw(raw), expected, "{raw}");
    }
}

#[test]
fn access_flags_are_read_independently() {
    for (bits, readable, writable, inactive, tlv_readable) in [
        (Access::READ | Access::TLV_READ, true, false, false, true),
        (Access::WRITE | Access::INACTIVE, false, true, true, false),
    ] {
        let access = Access(bits);
        assert_eq!(
            (
                access.readable(),
                access.writable(),
                access.inactive(),
                access.tlv_readable()
            ),
            (readable, writable, inactive, tlv_readable),
            "{bits:#x}"
        );
    }
}

#[test]
fn card_info_fields_stop_at_their_nul_or_fill_their_whole_width() {
    let mut buf = [0u8; CARD_INFO_SIZE];
    buf[0..4].copy_from_slice(&2i32.to_ne_bytes());
    put_text(&mut buf, 8, "PCH");
    put_text(&mut buf, 24, "HDA-Intel");
    put_text(&mut buf, 40, "HDA Intel PCH");
    put_text(&mut buf, 72, "HDA Intel PCH at 0xf7e10000");
    put_text(&mut buf, 168, "Realtek ALC892");
    let info = CardInfo::parse(&buf).unwrap();
    assert_eq!(info.card, 2);
    assert_eq!(info.id, "PCH");
    assert_eq!(info.driver, "HDA-Intel");
    assert_eq!(info.name, "HDA Intel PCH");
    assert_eq!(info.long_name, "HDA Intel PCH at 0xf7e10000");
    assert_eq!(info.mixer_name, "Realtek ALC892");

    let mut buf = [0u8; CARD_INFO_SIZE];
    buf[8..24].fill(b'i');
    let info = CardInfo::parse(&buf).unwrap();
    assert_eq!(info.id, "i".repeat(16));
    assert_eq!(info.driver, "");
}

#[test]
fn only_an_integer_element_carries_its_range() {
    for (kind, expected_kind, has_range) in [
        (2, ElementType::Integer, true),
        (6, ElementType::Integer64, true),
        (0, ElementType::from_raw(0), false),
        (1, ElementType::Boolean, false),
        (3, ElementType::from_raw(3), false),
        (4, ElementType::from_raw(4), false),
        (5, ElementType::from_raw(5), false),
        (9, ElementType::Other(9), false),
    ] {
        let mut buf = [0u8; ELEM_INFO_SIZE];
        master_id().encode(&mut buf).unwrap();
        put_u32(&mut buf, 64, kind);
        put_u32(&mut buf, 68, Access::READ | Access::WRITE);
        put_u32(&mut buf, 72, 2);
        put_i64(&mut buf, 80, -5);
        put_i64(&mut buf, 88, 87);
        put_i64(&mut buf, 96, 1);
        let info = ElementInfo::parse(&buf).unwrap();
        assert_eq!(info.id, master_id(), "{kind}");
        assert_eq!(info.kind, expected_kind, "{kind}");
        assert!(info.access.readable() && info.access.writable(), "{kind}");
        assert_eq!(info.count, 2, "{kind}");
        assert_eq!(
            info.range.map(|r| (r.min, r.max, r.step)),
            has_range.then_some((-5, 87, 1)),
            "{kind}"
        );
    }
}

fn event_record(kind: u32, mask: u32, id: &ElementId) -> [u8; EVENT_SIZE] {
    let mut buf = [0u8; EVENT_SIZE];
    put_u32(&mut buf, 0, kind);
    put_u32(&mut buf, 4, mask);
    id.encode(&mut buf[8..]).unwrap();
    buf
}

#[test]
fn events_are_parsed_in_order_with_their_value_and_removal_bits() {
    let other = ElementId {
        numid: 9,
        ..master_id()
    };
    let bytes = [
        event_record(0, 1, &master_id()),
        event_record(0, !0, &other),
        event_record(0, 1 << 1, &master_id()),
    ]
    .concat();
    let events = Event::parse_all(&bytes).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| (e.id.numid, e.value_changed(), e.removed()))
            .collect::<Vec<_>>(),
        [(7, true, false), (9, false, true), (7, false, false)]
    );
    assert_eq!(events[0].id, master_id());
    assert!(Event::parse_all(&[]).unwrap().is_empty());
}

#[test]
fn a_read_that_is_not_whole_events_of_the_element_type_is_refused() {
    let record = event_record(0, 1, &master_id());
    let truncated = Event::parse_all(&record[..EVENT_SIZE - 1]);
    assert!(
        matches!(truncated, Err(Error::TruncatedEvent { len }) if len == EVENT_SIZE - 1),
        "{truncated:?}"
    );
    let other_type = Event::parse_all(&event_record(3, 1, &master_id()));
    assert!(
        matches!(other_type, Err(Error::UnknownEventType { kind: 3 })),
        "{other_type:?}"
    );
}
