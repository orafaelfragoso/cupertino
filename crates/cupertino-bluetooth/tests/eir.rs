//! Extended inquiry response and advertising data.

use cupertino_bluetooth::{Eir, EirName, MajorClass, ParseError, Uuid};

fn name(text: &str, complete: bool) -> Option<EirName> {
    Some(EirName {
        text: text.to_owned(),
        complete,
    })
}

#[test]
fn the_fields_are_read_up_to_a_zero_length_and_unknown_ones_skipped() {
    let mut full = vec![2, 0x01, 0x1A];
    full.extend([6, 0x09, b'B', b'u', b'd', b's', b'!']);
    full.extend([2, 0x0A, 0xFE]);
    full.extend([4, 0x0D, 0x04, 0x04, 0x20]);
    full.extend([3, 0x19, 0xC1, 0x03]);
    let eir = Eir::parse(&full).unwrap();
    assert_eq!(eir.flags, Some(0x1A));
    assert_eq!(eir.name, name("Buds!", true));
    assert_eq!(eir.tx_power, Some(-2));
    let class = eir.class.unwrap();
    assert_eq!(class.value(), 0x20_04_04);
    assert_eq!(class.major(), MajorClass::AudioVideo);
    assert_eq!(class.minor(), 1);
    assert_eq!(eir.appearance, Some(0x03C1));

    for (case, data, expected) in [
        ("no data", vec![], Eir::default()),
        (
            "padding after a zero length",
            vec![2, 0x01, 0x06, 0, 0xFF, 0xFF, 0xFF],
            Eir {
                flags: Some(6),
                ..Eir::default()
            },
        ),
        (
            "an unknown field",
            vec![4, 0xFF, 1, 2, 3, 2, 0x01, 0x02],
            Eir {
                flags: Some(2),
                ..Eir::default()
            },
        ),
        (
            "a shortened name",
            vec![4, 0x08, b'a', b'b', b'c'],
            Eir {
                name: name("abc", false),
                ..Eir::default()
            },
        ),
        (
            "a name that is not UTF-8 keeps its valid parts",
            vec![5, 0x09, b'a', 0xFF, 0xFE, b'z'],
            Eir {
                name: name("a\u{FFFD}\u{FFFD}z", true),
                ..Eir::default()
            },
        ),
    ] {
        assert_eq!(Eir::parse(&data).unwrap(), expected, "{case}");
    }
}

#[test]
fn uuids_of_every_size_and_both_list_kinds_are_listed_in_order_of_appearance() {
    let mut data = vec![3, 0x02, 0x0D, 0x11];
    data.extend([3, 0x03, 0x0B, 0x11]);
    data.extend([5, 0x04, 0x78, 0x56, 0x34, 0x12]);
    data.extend([17, 0x07]);
    data.extend(1u8..=16);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&(1u8..=16).collect::<Vec<_>>());
    assert_eq!(
        Eir::parse(&data).unwrap().uuids,
        [
            Uuid::from_u16(0x110D),
            Uuid::from_u16(0x110B),
            Uuid::from_u32(0x1234_5678),
            Uuid(u128::from_le_bytes(bytes)),
        ]
    );
}

#[test]
fn a_short_uuid_extends_the_bluetooth_base_uuid_and_only_such_a_uuid_has_a_short_form() {
    assert_eq!(
        Uuid::from_u16(0x110B).to_string(),
        "0000110b-0000-1000-8000-00805f9b34fb"
    );
    assert_eq!(
        Uuid::from_u32(0x1234_5678).to_string(),
        "12345678-0000-1000-8000-00805f9b34fb"
    );
    for (uuid, expected) in [
        (Uuid::from_u16(0x110B), Some(0x110B)),
        (Uuid::from_u16(0), Some(0)),
        (Uuid::from_u16(u16::MAX), Some(u16::MAX)),
        (Uuid::from_u32(0x0001_0000), None),
        (Uuid(0x1234), None),
    ] {
        assert_eq!(uuid.as_u16(), expected, "{uuid}");
    }
}

#[test]
fn data_that_overruns_or_has_fields_of_the_wrong_size_is_refused() {
    for (data, expected) in [
        (
            &[2, 0x01, 0x06, 9, 0x09, b'a'][..],
            ParseError::EirOverrun { offset: 3 },
        ),
        (
            &[4, 0x03, 1, 2, 3],
            ParseError::EirUuidLength { size: 2, length: 3 },
        ),
        (
            &[4, 0x05, 1, 2, 3],
            ParseError::EirUuidLength { size: 4, length: 3 },
        ),
        (
            &[3, 0x07, 1, 2],
            ParseError::EirUuidLength {
                size: 16,
                length: 2,
            },
        ),
        (
            &[1, 0x01],
            ParseError::EirFieldLength {
                kind: 0x01,
                length: 0,
            },
        ),
        (
            &[3, 0x0A, 1, 2],
            ParseError::EirFieldLength {
                kind: 0x0A,
                length: 2,
            },
        ),
        (
            &[3, 0x0D, 1, 2],
            ParseError::EirFieldLength {
                kind: 0x0D,
                length: 2,
            },
        ),
        (
            &[2, 0x19, 1],
            ParseError::EirFieldLength {
                kind: 0x19,
                length: 1,
            },
        ),
    ] {
        assert_eq!(Eir::parse(data), Err(expected), "{data:?}");
    }
}
