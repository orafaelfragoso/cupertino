//! Extended inquiry response and advertising data.

use cupertino_bluetooth::{Eir, EirName, MajorClass, ParseError, Uuid};

#[test]
fn no_data_is_no_fields() {
    assert_eq!(Eir::parse(&[]).unwrap(), Eir::default());
}

#[test]
fn a_zero_length_ends_the_data_and_the_padding_after_it_is_ignored() {
    let eir = Eir::parse(&[2, 0x01, 0x06, 0, 0xFF, 0xFF, 0xFF]).unwrap();
    assert_eq!(eir.flags, Some(6));
}

#[test]
fn flags_name_power_class_and_appearance_are_read() {
    let mut data = vec![2, 0x01, 0x1A];
    data.extend([6, 0x09, b'B', b'u', b'd', b's', b'!']);
    data.extend([2, 0x0A, 0xFE]);
    data.extend([4, 0x0D, 0x04, 0x04, 0x20]);
    data.extend([3, 0x19, 0xC1, 0x03]);
    let eir = Eir::parse(&data).unwrap();
    assert_eq!(eir.flags, Some(0x1A));
    assert_eq!(
        eir.name,
        Some(EirName {
            text: "Buds!".to_owned(),
            complete: true
        })
    );
    assert_eq!(eir.tx_power, Some(-2));
    let class = eir.class.unwrap();
    assert_eq!(class.value(), 0x20_04_04);
    assert_eq!(class.major(), MajorClass::AudioVideo);
    assert_eq!(class.minor(), 1);
    assert_eq!(eir.appearance, Some(0x03C1));
}

#[test]
fn a_shortened_name_is_not_complete() {
    let eir = Eir::parse(&[4, 0x08, b'a', b'b', b'c']).unwrap();
    assert_eq!(
        eir.name,
        Some(EirName {
            text: "abc".to_owned(),
            complete: false
        })
    );
}

#[test]
fn a_name_that_is_not_utf8_keeps_its_valid_parts() {
    let eir = Eir::parse(&[5, 0x09, b'a', 0xFF, 0xFE, b'z']).unwrap();
    assert_eq!(eir.name.unwrap().text, "a\u{FFFD}\u{FFFD}z");
}

#[test]
fn uuids_of_every_size_are_listed_in_order_of_appearance() {
    let mut data = vec![5, 0x03, 0x0D, 0x11, 0x0B, 0x11];
    data.extend([5, 0x04, 0x78, 0x56, 0x34, 0x12]);
    data.extend([17, 0x07]);
    data.extend((1u8..=16).collect::<Vec<_>>());
    let eir = Eir::parse(&data).unwrap();
    assert_eq!(eir.uuids.len(), 4);
    assert_eq!(eir.uuids[0], Uuid::from_u16(0x110D));
    assert_eq!(eir.uuids[1], Uuid::from_u16(0x110B));
    assert_eq!(eir.uuids[2], Uuid::from_u32(0x1234_5678));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&(1u8..=16).collect::<Vec<_>>());
    assert_eq!(eir.uuids[3], Uuid(u128::from_le_bytes(bytes)));
}

#[test]
fn incomplete_and_complete_lists_both_count() {
    let eir = Eir::parse(&[3, 0x02, 0x0D, 0x11, 3, 0x03, 0x0B, 0x11]).unwrap();
    assert_eq!(
        eir.uuids,
        vec![Uuid::from_u16(0x110D), Uuid::from_u16(0x110B)]
    );
}

#[test]
fn a_short_uuid_extends_the_bluetooth_base_uuid() {
    assert_eq!(
        Uuid::from_u16(0x110B).to_string(),
        "0000110b-0000-1000-8000-00805f9b34fb"
    );
    assert_eq!(
        Uuid::from_u32(0x1234_5678).to_string(),
        "12345678-0000-1000-8000-00805f9b34fb"
    );
    assert_eq!(Uuid::from_u16(0x110B).as_u16(), Some(0x110B));
}

#[test]
fn a_uuid_that_is_not_a_16_bit_extension_has_no_short_form() {
    assert_eq!(Uuid::from_u32(0x0001_0000).as_u16(), None);
    assert_eq!(Uuid(0x1234).as_u16(), None);
    assert_eq!(Uuid::from_u16(0).as_u16(), Some(0));
    assert_eq!(Uuid::from_u16(u16::MAX).as_u16(), Some(u16::MAX));
}

#[test]
fn unknown_field_types_are_skipped() {
    let eir = Eir::parse(&[4, 0xFF, 1, 2, 3, 2, 0x01, 0x02]).unwrap();
    assert_eq!(eir.flags, Some(2));
}

#[test]
fn a_structure_that_runs_past_the_data_is_an_overrun() {
    assert_eq!(
        Eir::parse(&[2, 0x01, 0x06, 9, 0x09, b'a']),
        Err(ParseError::EirOverrun { offset: 3 })
    );
}

#[test]
fn a_uuid_list_that_is_not_a_whole_number_of_uuids_is_refused() {
    assert_eq!(
        Eir::parse(&[4, 0x03, 1, 2, 3]),
        Err(ParseError::EirUuidLength { size: 2, length: 3 })
    );
    assert_eq!(
        Eir::parse(&[4, 0x05, 1, 2, 3]),
        Err(ParseError::EirUuidLength { size: 4, length: 3 })
    );
    assert_eq!(
        Eir::parse(&[3, 0x07, 1, 2]),
        Err(ParseError::EirUuidLength {
            size: 16,
            length: 2
        })
    );
}

#[test]
fn fields_of_the_wrong_size_are_refused() {
    assert_eq!(
        Eir::parse(&[1, 0x01]),
        Err(ParseError::EirFieldLength {
            kind: 0x01,
            length: 0
        })
    );
    assert_eq!(
        Eir::parse(&[3, 0x0A, 1, 2]),
        Err(ParseError::EirFieldLength {
            kind: 0x0A,
            length: 2
        })
    );
    assert_eq!(
        Eir::parse(&[3, 0x0D, 1, 2]),
        Err(ParseError::EirFieldLength {
            kind: 0x0D,
            length: 2
        })
    );
    assert_eq!(
        Eir::parse(&[2, 0x19, 1]),
        Err(ParseError::EirFieldLength {
            kind: 0x19,
            length: 1
        })
    );
}
