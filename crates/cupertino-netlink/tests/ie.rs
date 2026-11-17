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

#[test]
fn the_ssid_is_the_names_bytes_and_a_name_of_nothing_or_nul_bytes_is_hidden() {
    for (name, elements, bytes, text, hidden) in [
        ("no elements", vec![], &[][..], "", true),
        ("a name", ssid("Cafe Wifi"), b"Cafe Wifi", "Cafe Wifi", false),
        (
            "not UTF-8",
            element(0, &[b'a', 0xFF, b'b']),
            &[b'a', 0xFF, b'b'],
            "a\u{FFFD}b",
            false,
        ),
        (
            "among other elements",
            [element(1, &[0x82, 0x84]), ssid("home"), element(3, &[6])].concat(),
            b"home",
            "home",
            false,
        ),
        ("nul bytes", element(0, &[0, 0, 0]), &[0, 0, 0], "\0\0\0", true),
        ("a nul and a letter", element(0, &[0, b'x']), &[0, b'x'], "\0x", false),
    ] {
        let (ssid, _) = ssid_and_security(&elements, 0).unwrap();
        assert_eq!(ssid, Ssid(bytes.to_vec()), "{name}");
        assert_eq!(ssid.as_bytes(), bytes, "{name}");
        assert_eq!(ssid.to_text(), text, "{name}");
        assert_eq!(ssid.is_hidden(), hidden, "{name}");
    }
}

#[test]
fn the_security_follows_the_key_management_of_the_rsn_then_the_wpa_element_then_the_privacy_bit() {
    let mut other_oui = body(IEEE, &[]);
    other_oui[2 + 4 + 2 + 4] = 1;
    other_oui.extend([0x00, 0x50, 0xF2, 2]);
    let other_vendor = element(221, &[0x00, 0x10, 0x18, 0x02, 0, 0]);
    let mut rows: Vec<(String, Vec<u8>, u16, Security)> = vec![
        ("no elements".into(), vec![], 0, Security::Open),
        ("privacy alone is WEP".into(), ssid("old"), CAPABILITY_PRIVACY, Security::Wep),
        ("another capability bit".into(), ssid("old"), 0x0001, Security::Open),
        (
            "a suite of another OUI".into(),
            element(48, &other_oui),
            CAPABILITY_PRIVACY,
            Security::Unsupported,
        ),
        ("WPA".into(), wpa(&[2]), CAPABILITY_PRIVACY, Security::Wpa),
        ("WPA enterprise".into(), wpa(&[1]), CAPABILITY_PRIVACY, Security::Enterprise),
        ("WPA unknown".into(), wpa(&[9]), CAPABILITY_PRIVACY, Security::Unsupported),
        (
            "RSN after WPA wins".into(),
            [wpa(&[2]), rsn(&[2])].concat(),
            CAPABILITY_PRIVACY,
            Security::Wpa2,
        ),
        (
            "RSN before WPA wins".into(),
            [rsn(&[8]), wpa(&[2])].concat(),
            CAPABILITY_PRIVACY,
            Security::Wpa3,
        ),
        ("another vendor element".into(), other_vendor.clone(), 0, Security::Open),
        (
            "another vendor element with privacy".into(),
            other_vendor,
            CAPABILITY_PRIVACY,
            Security::Wep,
        ),
        (
            "RSN among other elements".into(),
            [element(1, &[0x82, 0x84]), ssid("home"), element(3, &[6]), rsn(&[2])].concat(),
            CAPABILITY_PRIVACY,
            Security::Wpa2,
        ),
    ];
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
        rows.push((format!("RSN {akms:?}"), rsn(akms), CAPABILITY_PRIVACY, expected));
    }
    for (name, elements, capability, expected) in rows {
        assert_eq!(
            ssid_and_security(&elements, capability).map(|(_, s)| s),
            Ok(expected),
            "{name}"
        );
    }
}

#[test]
fn a_cut_or_overlong_element_or_an_rsn_element_that_ends_early_is_refused() {
    let mut rows = vec![
        (
            vec![0],
            ParseError::Truncated {
                what: "information element header",
                needed: 2,
                have: 1,
            },
        ),
        (
            vec![0, 9, b'a'],
            ParseError::InformationElement {
                id: 0,
                length: 9,
                available: 1,
            },
        ),
    ];
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
        rows.push((element(48, &data), ParseError::SecurityElement { what }));
    }
    for (elements, expected) in rows {
        assert_eq!(
            ssid_and_security(&elements, 0).map(drop),
            Err(expected.clone()),
            "{expected:?}"
        );
    }
}
