//! Decoding of the kernel's control structures and encoding of an element id.

use cupertino_alsa::abi::{CARD_INFO_SIZE, ELEM_ID_SIZE, ELEM_INFO_SIZE, EVENT_SIZE, MAX_VALUES};
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
fn an_element_id_survives_encoding_and_parsing() {
    let id = master_id();
    let mut buf = [0xffu8; ELEM_ID_SIZE];
    id.encode(&mut buf).unwrap();
    assert_eq!(ElementId::parse(&buf).unwrap(), id);
}

#[test]
fn encoding_clears_the_rest_of_the_name_field() {
    let mut buf = [0xffu8; ELEM_ID_SIZE];
    ElementId {
        name: "Mic".to_owned(),
        ..master_id()
    }
    .encode(&mut buf)
    .unwrap();
    assert_eq!(&buf[16..19], b"Mic");
    assert!(buf[19..60].iter().all(|&b| b == 0));
}

#[test]
fn a_name_that_fills_the_field_has_no_room_for_its_nul() {
    let mut buf = [0u8; ELEM_ID_SIZE];
    let longest = ElementId {
        name: "n".repeat(43),
        ..master_id()
    };
    longest.encode(&mut buf).unwrap();
    assert_eq!(ElementId::parse(&buf).unwrap(), longest);

    let too_long = ElementId {
        name: "n".repeat(44),
        ..master_id()
    };
    assert!(matches!(
        too_long.encode(&mut buf),
        Err(Error::NameTooLong { len: 44 })
    ));
}

#[test]
fn a_name_that_is_not_utf8_is_refused() {
    let mut buf = [0u8; ELEM_ID_SIZE];
    buf[16] = 0xff;
    buf[17] = 0xfe;
    assert!(matches!(
        ElementId::parse(&buf),
        Err(Error::NotUtf8 { field: "name" })
    ));
}

#[test]
fn interfaces_and_types_map_to_the_kernel_numbers() {
    for raw in 0..=6 {
        assert_eq!(Interface::from_raw(raw).raw(), raw);
    }
    assert_eq!(Interface::from_raw(2), Interface::Mixer);
    assert_eq!(Interface::from_raw(99), Interface::Other(99));
    assert_eq!(Interface::Other(99).raw(), 99);
    assert_eq!(ElementType::from_raw(1), ElementType::Boolean);
    assert_eq!(ElementType::from_raw(2), ElementType::Integer);
    assert_eq!(ElementType::from_raw(6), ElementType::Integer64);
    assert_eq!(ElementType::from_raw(42), ElementType::Other(42));
}

#[test]
fn access_flags_are_read_independently() {
    let access = Access(Access::READ | Access::TLV_READ);
    assert!(access.readable());
    assert!(!access.writable());
    assert!(!access.inactive());
    assert!(access.tlv_readable());
    let access = Access(Access::WRITE | Access::INACTIVE);
    assert!(!access.readable());
    assert!(access.writable());
    assert!(access.inactive());
    assert!(!access.tlv_readable());
}

#[test]
fn card_info_fields_stop_at_their_nul() {
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
}

#[test]
fn a_card_info_field_may_fill_its_whole_width() {
    let mut buf = [0u8; CARD_INFO_SIZE];
    buf[8..24].fill(b'i');
    let info = CardInfo::parse(&buf).unwrap();
    assert_eq!(info.id, "i".repeat(16));
    assert_eq!(info.driver, "");
}

#[test]
fn a_card_info_field_that_is_not_utf8_names_the_field() {
    let mut buf = [0u8; CARD_INFO_SIZE];
    buf[40] = 0xc0;
    assert!(matches!(
        CardInfo::parse(&buf),
        Err(Error::NotUtf8 { field: "name" })
    ));
}

fn element_info_buf(kind: u32) -> [u8; ELEM_INFO_SIZE] {
    let mut buf = [0u8; ELEM_INFO_SIZE];
    master_id().encode(&mut buf).unwrap();
    put_u32(&mut buf, 64, kind);
    put_u32(&mut buf, 68, Access::READ | Access::WRITE);
    put_u32(&mut buf, 72, 2);
    put_i64(&mut buf, 80, -5);
    put_i64(&mut buf, 88, 87);
    put_i64(&mut buf, 96, 1);
    buf
}

#[test]
fn an_integer_element_carries_its_range() {
    let info = ElementInfo::parse(&element_info_buf(2)).unwrap();
    assert_eq!(info.id, master_id());
    assert_eq!(info.kind, ElementType::Integer);
    assert!(info.access.readable() && info.access.writable());
    assert_eq!(info.count, 2);
    let range = info.range.unwrap();
    assert_eq!((range.min, range.max, range.step), (-5, 87, 1));
}

#[test]
fn a_64_bit_integer_element_carries_its_range() {
    let info = ElementInfo::parse(&element_info_buf(6)).unwrap();
    assert_eq!(info.kind, ElementType::Integer64);
    assert_eq!(info.range.unwrap().max, 87);
}

#[test]
fn other_element_types_have_no_range() {
    for kind in [0, 1, 3, 4, 5, 9] {
        assert_eq!(
            ElementInfo::parse(&element_info_buf(kind)).unwrap().range,
            None
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
fn events_are_parsed_in_order() {
    let first = event_record(0, 1, &master_id());
    let second = event_record(
        0,
        !0,
        &ElementId {
            numid: 9,
            ..master_id()
        },
    );
    let events = Event::parse_all(&[first.as_slice(), second.as_slice()].concat()).unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[0].value_changed());
    assert!(!events[0].removed());
    assert_eq!(events[0].id, master_id());
    assert!(events[1].removed());
    assert!(!events[1].value_changed());
    assert_eq!(events[1].id.numid, 9);
}

#[test]
fn an_event_without_the_value_bit_is_not_a_value_change() {
    let events = Event::parse_all(&event_record(0, 1 << 1, &master_id())).unwrap();
    assert!(!events[0].value_changed());
    assert!(!events[0].removed());
}

#[test]
fn no_bytes_are_no_events() {
    assert!(Event::parse_all(&[]).unwrap().is_empty());
}

#[test]
fn a_read_that_is_not_a_whole_number_of_events_is_truncated() {
    let bytes = event_record(0, 1, &master_id());
    assert!(matches!(
        Event::parse_all(&bytes[..EVENT_SIZE - 1]),
        Err(Error::TruncatedEvent { len }) if len == EVENT_SIZE - 1
    ));
}

#[test]
fn an_event_of_another_type_is_refused() {
    assert!(matches!(
        Event::parse_all(&event_record(3, 1, &master_id())),
        Err(Error::UnknownEventType { kind: 3 })
    ));
}

#[test]
fn the_value_limit_is_the_kernels() {
    assert_eq!(MAX_VALUES, 128);
}
