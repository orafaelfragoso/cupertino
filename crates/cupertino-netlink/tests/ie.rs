//! The SSID and security of a network from its information elements.

use cupertino_netlink::ie::{ssid_and_security, CAPABILITY_PRIVACY};
use cupertino_netlink::{ParseError, Security, Ssid};

fn element(id: u8, data: &[u8]) -> Vec<u8> {
    [&[id, data.len() as u8][..], data].concat()
}

fn ssid(name: &str) -> Vec<u8> {
    element(0, name.as_bytes())
}

const IEEE: [u8; 3] = [0x00, 0x0F, 0xAC];
const WPA: [u8; 3] = [0x00, 0x50, 0xF2];

/// An RSN-layout body: version, group cipher, one pairwise cipher and the AKM suites.
fn body(oui: [u8; 3], akms: &[u8]) -> Vec<u8> {
    let mut data = vec![1, 0];
    data.extend([oui[0], oui[1], oui[2], 4]);
    data.extend([1, 0, oui[0], oui[1], oui[2], 4]);
    data.extend((akms.len() as u16).to_le_bytes());
    for &suite in akms {
        data.extend([oui[0], oui[1], oui[2], suite]);
    }
    data
}

fn rsn(akms: &[u8]) -> Vec<u8> {
    element(48, &body(IEEE, akms))
}

fn wpa(akms: &[u8]) -> Vec<u8> {
    element(
        221,
        &[&[0x00, 0x50, 0xF2, 0x01][..], &body(WPA, akms)].concat(),
    )
}

fn security(elements: &[u8], capability: u16) -> Security {
    ssid_and_security(elements, capability).unwrap().1
}

#[test]
fn no_elements_are_an_open_network_with_an_empty_name() {
    let (name, security) = ssid_and_security(&[], 0).unwrap();
    assert_eq!(name, Ssid(Vec::new()));
    assert!(name.is_hidden());
    assert_eq!(security, Security::Open);
}

#[test]
fn the_ssid_element_gives_the_name_as_bytes() {
    let (name, _) = ssid_and_security(&ssid("Cafe Wifi"), 0).unwrap();
    assert_eq!(name.as_bytes(), b"Cafe Wifi");
    assert_eq!(name.to_text(), "Cafe Wifi");
    assert!(!name.is_hidden());
}

#[test]
fn a_name_that_is_not_utf8_keeps_its_bytes_and_shows_replacement_characters() {
    let elements = element(0, &[b'a', 0xFF, b'b']);
    let (name, _) = ssid_and_security(&elements, 0).unwrap();
    assert_eq!(name.as_bytes(), [b'a', 0xFF, b'b']);
    assert_eq!(name.to_text(), "a\u{FFFD}b");
}

#[test]
fn a_network_that_hides_its_name_announces_nothing_or_nul_bytes() {
    assert!(Ssid(Vec::new()).is_hidden());
    assert!(Ssid(vec![0, 0, 0]).is_hidden());
    assert!(!Ssid(vec![0, b'x']).is_hidden());
}

#[test]
fn the_privacy_bit_without_a_security_element_is_wep() {
    assert_eq!(security(&ssid("old"), CAPABILITY_PRIVACY), Security::Wep);
    assert_eq!(security(&ssid("old"), 0x0001), Security::Open);
}

#[test]
fn key_management_suites_map_to_the_security_they_allow() {
    for (akms, expected) in [
        (&[2u8][..], Security::Wpa2),
        (&[4], Security::Wpa2),
        (&[6], Security::Wpa2),
        (&[19], Security::Wpa2),
        (&[20], Security::Wpa2),
        (&[8], Security::Wpa3),
        (&[9], Security::Wpa3),
        (&[24], Security::Wpa3),
        (&[25], Security::Wpa3),
        (&[2, 8], Security::Wpa2Wpa3),
        (&[8, 2], Security::Wpa2Wpa3),
        (&[18], Security::Owe),
        (&[1], Security::Enterprise),
        (&[5], Security::Enterprise),
        (&[11], Security::Enterprise),
        (&[17], Security::Enterprise),
        (&[2, 1], Security::Wpa2),
        (&[99], Security::Unsupported),
        (&[], Security::Unsupported),
    ] {
        assert_eq!(
            security(&rsn(akms), CAPABILITY_PRIVACY),
            expected,
            "{akms:?}"
        );
    }
}

#[test]
fn a_suite_of_another_oui_is_ignored() {
    let mut data = body(IEEE, &[]);
    let count_at = 2 + 4 + 2 + 4;
    data[count_at] = 1;
    data.extend([0x00, 0x50, 0xF2, 2]);
    assert_eq!(
        security(&element(48, &data), CAPABILITY_PRIVACY),
        Security::Unsupported
    );
}

#[test]
fn the_wpa_element_gives_wpa_or_enterprise() {
    assert_eq!(security(&wpa(&[2]), CAPABILITY_PRIVACY), Security::Wpa);
    assert_eq!(
        security(&wpa(&[1]), CAPABILITY_PRIVACY),
        Security::Enterprise
    );
    assert_eq!(
        security(&wpa(&[9]), CAPABILITY_PRIVACY),
        Security::Unsupported
    );
}

#[test]
fn an_rsn_element_wins_over_a_wpa_element_in_either_order() {
    let both = [wpa(&[2]), rsn(&[2])].concat();
    assert_eq!(security(&both, CAPABILITY_PRIVACY), Security::Wpa2);
    let both = [rsn(&[8]), wpa(&[2])].concat();
    assert_eq!(security(&both, CAPABILITY_PRIVACY), Security::Wpa3);
}

#[test]
fn a_vendor_element_that_is_not_wpa_does_not_count() {
    let other = element(221, &[0x00, 0x10, 0x18, 0x02, 0, 0]);
    assert_eq!(security(&other, 0), Security::Open);
    assert_eq!(security(&other, CAPABILITY_PRIVACY), Security::Wep);
}

#[test]
fn elements_around_the_interesting_ones_are_skipped() {
    let all = [
        element(1, &[0x82, 0x84]),
        ssid("home"),
        element(3, &[6]),
        rsn(&[2]),
    ]
    .concat();
    let (name, security) = ssid_and_security(&all, CAPABILITY_PRIVACY).unwrap();
    assert_eq!(name.to_text(), "home");
    assert_eq!(security, Security::Wpa2);
}

#[test]
fn a_cut_element_header_is_truncated() {
    assert_eq!(
        ssid_and_security(&[0], 0),
        Err(ParseError::Truncated {
            what: "information element header",
            needed: 2,
            have: 1
        })
    );
}

#[test]
fn an_element_longer_than_the_list_is_refused() {
    assert_eq!(
        ssid_and_security(&[0, 9, b'a'], 0),
        Err(ParseError::InformationElement {
            id: 0,
            length: 9,
            available: 1
        })
    );
}

#[test]
fn an_rsn_element_that_ends_early_names_the_missing_field() {
    for (data, what) in [
        (vec![1], "version"),
        (vec![1, 0, 0, 0xF, 0xAC], "group cipher suite"),
        (vec![1, 0, 0, 0xF, 0xAC, 4, 1], "pairwise suite count"),
        (
            vec![1, 0, 0, 0xF, 0xAC, 4, 2, 0, 0, 0xF, 0xAC, 4],
            "pairwise cipher suites",
        ),
        (vec![1, 0, 0, 0xF, 0xAC, 4, 0, 0], "AKM suite count"),
        (
            vec![1, 0, 0, 0xF, 0xAC, 4, 0, 0, 2, 0, 0, 0xF, 0xAC, 2],
            "AKM suites",
        ),
    ] {
        assert_eq!(
            ssid_and_security(&element(48, &data), 0),
            Err(ParseError::SecurityElement { what }),
            "{what}"
        );
    }
}
