//! NetworkManager's words turned into the daemons': security, strength and SSID keys.

#![cfg(feature = "dbus")]

use cupertino_radios::nm::{dbm, security, ssid_key};

const PRIVACY: u32 = 0x1;
const PSK: u32 = 0x100;
const ENTERPRISE: u32 = 0x200;
const SAE: u32 = 0x400;
const OWE: u32 = 0x800;

#[test]
fn the_security_of_an_access_point_comes_from_its_keys_then_its_privacy_flag() {
    for (flags, wpa, rsn, expected) in [
        (0, 0, 0, "open"),
        (PRIVACY, 0, 0, "wep"),
        (PRIVACY, 0, PSK, "wpa2"),
        (PRIVACY, PSK, 0, "wpa"),
        (PRIVACY, 0, PSK | SAE, "wpa2-wpa3"),
        (PRIVACY, 0, SAE, "wpa3"),
        (PRIVACY, 0, ENTERPRISE, "enterprise"),
        (PRIVACY, PSK | ENTERPRISE, PSK | SAE, "enterprise"),
        (0, 0, OWE, "owe"),
        (PRIVACY, 0, OWE, "owe"),
        (PRIVACY, PSK, PSK, "wpa2"),
        // SAE in the old WPA set does not make WPA3
        (0, SAE, PSK, "wpa2"),
        (0, SAE, 0, "open"),
    ] {
        assert_eq!(
            security(flags, wpa, rsn),
            expected,
            "{flags:#x} {wpa:#x} {rsn:#x}"
        );
    }
}

#[test]
fn strength_maps_linearly_to_dbm_up_to_100() {
    for (strength, expected) in [
        (0, -100),
        (1, -100),
        (50, -75),
        (99, -51),
        (100, -50),
        (101, -50),
        (255, -50),
    ] {
        assert_eq!(dbm(strength), expected, "{strength}");
    }
}

#[test]
fn an_ssid_is_its_text_or_a_nul_and_its_bytes_in_hex() {
    for (ssid, expected) in [
        (&b"Home Wi-Fi"[..], "Home Wi-Fi"),
        ("Café".as_bytes(), "Café"),
        (b"", ""),
        (&[0xff, 0x41], "\0ff41"),
        (&[0x00, 0xab], "\u{0}00ab"),
        // a leading NUL is hex too, so text cannot clash with a hex key
        (b"\0a", "\u{0}0061"),
    ] {
        assert_eq!(ssid_key(ssid), expected, "{ssid:?}");
    }
    assert_ne!(ssid_key(&[0xff, 0x41]), ssid_key(&[0xfe, 0x41]));
    assert_ne!(ssid_key(&[0xff]), ssid_key(&[0xff, 0xff]));
}
