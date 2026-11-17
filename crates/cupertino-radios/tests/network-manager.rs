//! NetworkManager's words turned into the daemons': security, strength and SSID keys.

#![cfg(feature = "dbus")]

use cupertino_radios::nm::{dbm, security, ssid_key};

const PRIVACY: u32 = 0x1;
const PSK: u32 = 0x100;
const ENTERPRISE: u32 = 0x200;
const SAE: u32 = 0x400;
const OWE: u32 = 0x800;

#[test]
fn an_access_point_without_flags_or_keys_is_open() {
    assert_eq!(security(0, 0, 0), "open");
}

#[test]
fn the_privacy_flag_alone_is_wep() {
    assert_eq!(security(PRIVACY, 0, 0), "wep");
}

#[test]
fn a_psk_key_in_the_rsn_set_is_wpa2_and_in_the_wpa_set_is_wpa() {
    assert_eq!(security(PRIVACY, 0, PSK), "wpa2");
    assert_eq!(security(PRIVACY, PSK, 0), "wpa");
}

#[test]
fn sae_with_psk_is_the_transition_mode_and_sae_alone_is_wpa3() {
    assert_eq!(security(PRIVACY, 0, PSK | SAE), "wpa2-wpa3");
    assert_eq!(security(PRIVACY, 0, SAE), "wpa3");
}

#[test]
fn enterprise_wins_over_every_other_key() {
    assert_eq!(security(PRIVACY, 0, ENTERPRISE), "enterprise");
    assert_eq!(security(PRIVACY, PSK | ENTERPRISE, PSK | SAE), "enterprise");
}

#[test]
fn owe_is_an_encrypted_open_network() {
    assert_eq!(security(0, 0, OWE), "owe");
}

#[test]
fn keys_beat_the_privacy_flag() {
    assert_eq!(security(PRIVACY, 0, OWE), "owe");
    assert_eq!(security(PRIVACY, PSK, PSK), "wpa2");
}

#[test]
fn sae_in_the_old_wpa_set_does_not_make_wpa3() {
    assert_eq!(security(0, SAE, PSK), "wpa2");
    assert_eq!(security(0, SAE, 0), "open");
}

#[test]
fn strength_maps_linearly_to_dbm() {
    assert_eq!(dbm(0), -100);
    assert_eq!(dbm(50), -75);
    assert_eq!(dbm(100), -50);
    assert_eq!(dbm(1), -100);
    assert_eq!(dbm(99), -51);
}

#[test]
fn a_strength_above_100_is_taken_as_100() {
    assert_eq!(dbm(101), -50);
    assert_eq!(dbm(255), -50);
}

#[test]
fn an_ssid_that_is_text_is_its_own_key() {
    assert_eq!(ssid_key(b"Home Wi-Fi"), "Home Wi-Fi");
    assert_eq!(ssid_key("Café".as_bytes()), "Café");
    assert_eq!(ssid_key(b""), "");
}

#[test]
fn an_ssid_that_is_not_text_is_a_nul_and_its_bytes_in_hex() {
    assert_eq!(ssid_key(&[0xff, 0x41]), "\0ff41");
    assert_eq!(ssid_key(&[0x00, 0xab]), "\0".to_owned() + "00ab");
}

#[test]
fn an_ssid_that_starts_with_a_nul_is_hex_too_so_it_cannot_clash_with_a_key() {
    assert_eq!(ssid_key(b"\0a"), "\0".to_owned() + "0061");
}

#[test]
fn ssids_that_differ_in_bytes_that_are_not_text_have_different_keys() {
    assert_ne!(ssid_key(&[0xff, 0x41]), ssid_key(&[0xfe, 0x41]));
    assert_ne!(ssid_key(&[0xff]), ssid_key(&[0xff, 0xff]));
}
