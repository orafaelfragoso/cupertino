//! Device addresses, address types and the discovery mask.

use cupertino_bluetooth::{Address, AddressType, AddressTypes, ParseError};

#[test]
fn an_address_is_written_most_significant_byte_first_and_kept_least_significant_first() {
    let address: Address = "00:1A:7D:DA:71:13".parse().unwrap();
    assert_eq!(address.to_wire(), [0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00]);
    assert_eq!(address.to_string(), "00:1A:7D:DA:71:13");
    assert_eq!(Address::from_wire(address.to_wire()), address);
}

#[test]
fn an_address_parses_in_either_case_and_prints_in_upper_case() {
    let address: Address = "aa:bb:cc:dd:ee:ff".parse().unwrap();
    assert_eq!(address.to_string(), "AA:BB:CC:DD:EE:FF");
    assert_eq!(format!("{address:?}"), "AA:BB:CC:DD:EE:FF");
}

#[test]
fn the_zero_address_is_all_zero() {
    assert_eq!(Address::ZERO.to_wire(), [0; 6]);
    assert_eq!(Address::ZERO.to_string(), "00:00:00:00:00:00");
}

#[test]
fn text_that_is_not_an_address_is_refused_with_the_text() {
    for text in [
        "",
        "00:1A:7D:DA:71",
        "00:1A:7D:DA:71:13:99",
        "0:1A:7D:DA:71:13",
        "001:1A:7D:DA:71:13",
        "00-1A-7D-DA-71-13",
        "GG:1A:7D:DA:71:13",
        "+0:1A:7D:DA:71:13",
        " 0:1A:7D:DA:71:13",
        "00:1A:7D:DA:71:13:",
    ] {
        assert_eq!(
            text.parse::<Address>(),
            Err(ParseError::InvalidAddress(text.to_owned())),
            "{text:?}"
        );
    }
}

#[test]
fn address_types_use_the_management_numbers() {
    for (kind, wire) in [
        (AddressType::BrEdr, 0),
        (AddressType::LePublic, 1),
        (AddressType::LeRandom, 2),
    ] {
        assert_eq!(kind.to_wire(), wire);
        assert_eq!(AddressType::from_wire(wire), Ok(kind));
    }
    assert_eq!(
        AddressType::from_wire(3),
        Err(ParseError::UnknownAddressType(3))
    );
}

#[test]
fn only_the_two_low_energy_types_are_le() {
    assert!(!AddressType::BrEdr.is_le());
    assert!(AddressType::LePublic.is_le());
    assert!(AddressType::LeRandom.is_le());
}

#[test]
fn address_type_names_round_trip() {
    for kind in [
        AddressType::BrEdr,
        AddressType::LePublic,
        AddressType::LeRandom,
    ] {
        assert_eq!(kind.to_string().parse::<AddressType>(), Ok(kind));
    }
    assert_eq!(AddressType::LePublic.to_string(), "le-public");
    assert_eq!(
        "BREDR".parse::<AddressType>(),
        Err(ParseError::InvalidAddressTypeName("BREDR".to_owned()))
    );
}

#[test]
fn the_discovery_masks_are_the_documents() {
    assert_eq!(AddressTypes::BR_EDR.to_wire(), 0x01);
    assert_eq!(AddressTypes::LE.to_wire(), 0x06);
    assert_eq!(AddressTypes::ALL.to_wire(), 0x07);
    assert_eq!(AddressTypes::from_wire(0x05).to_wire(), 0x05);
}
