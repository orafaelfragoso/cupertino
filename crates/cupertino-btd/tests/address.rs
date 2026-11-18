//! Device addresses and address types.

use cupertino_btd::bluetooth::{Address, AddressType, ParseError};

#[test]
fn an_address_is_written_most_significant_byte_first_in_upper_case_and_kept_least_significant_first(
) {
    for (text, wire, printed) in [
        (
            "00:1A:7D:DA:71:13",
            [0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00],
            "00:1A:7D:DA:71:13",
        ),
        (
            "aa:bb:cc:dd:ee:ff",
            [0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA],
            "AA:BB:CC:DD:EE:FF",
        ),
        ("00:00:00:00:00:00", [0; 6], "00:00:00:00:00:00"),
    ] {
        let address: Address = text.parse().unwrap();
        assert_eq!(address.to_wire(), wire, "{text}");
        assert_eq!(address.to_string(), printed, "{text}");
        assert_eq!(format!("{address:?}"), printed, "{text}");
        assert_eq!(Address::from_wire(wire), address, "{text}");
    }
    assert_eq!(Address::ZERO.to_wire(), [0; 6]);
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
fn address_types_round_trip_through_the_management_numbers_and_their_names() {
    for (kind, wire, le) in [
        (AddressType::BrEdr, 0, false),
        (AddressType::LePublic, 1, true),
        (AddressType::LeRandom, 2, true),
    ] {
        assert_eq!(kind.to_wire(), wire, "{kind:?}");
        assert_eq!(AddressType::from_wire(wire), Ok(kind), "{kind:?}");
        assert_eq!(kind.is_le(), le, "{kind:?}");
        assert_eq!(
            kind.to_string().parse::<AddressType>(),
            Ok(kind),
            "{kind:?}"
        );
    }
    assert_eq!(
        AddressType::from_wire(3),
        Err(ParseError::UnknownAddressType(3))
    );
    assert_eq!(
        "BREDR".parse::<AddressType>(),
        Err(ParseError::InvalidAddressTypeName("BREDR".to_owned()))
    );
}
