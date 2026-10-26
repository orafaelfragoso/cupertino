//! 802.11 information elements: the SSID and the security a network announces.
//!
//! An element is `id (1 byte) | length (1 byte) | data`. Layouts are those of IEEE 802.11-2020
//! (element formats; multi-byte fields are little-endian) and, for the pre-RSN "WPA" element, the
//! Wi-Fi Alliance WPA specification: a vendor element (id 221) with OUI 00:50:F2 and type 1.

use crate::error::ParseError;

/// Element id of the SSID (IEEE 802.11-2020, element IDs table).
const ELEMENT_SSID: u8 = 0;
/// Element id of the robust security network (RSN) element.
const ELEMENT_RSN: u8 = 48;
/// Element id of vendor-specific elements, which start with the vendor's OUI.
const ELEMENT_VENDOR: u8 = 221;
/// The vendor prefix of the WPA element: OUI 00:50:F2, type 1.
const WPA_PREFIX: [u8; 4] = [0x00, 0x50, 0xf2, 0x01];
/// OUI of the 802.11 cipher and AKM suite selectors.
const IEEE_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
/// OUI of the WPA element's suites.
const WPA_OUI: [u8; 3] = [0x00, 0x50, 0xf2];
/// The privacy bit of the capability field: the network uses encryption.
pub const CAPABILITY_PRIVACY: u16 = 0x0010;

/// A network name: 0 to 32 bytes, not necessarily UTF-8.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ssid(pub Vec<u8>);

impl Ssid {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The name for display, with bytes that are not UTF-8 replaced by U+FFFD. Compare and
    /// store SSIDs as bytes: two different names can share this text.
    pub fn to_text(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    /// A network that hides its name announces an empty SSID, or one of NUL bytes.
    pub fn is_hidden(&self) -> bool {
        self.0.iter().all(|&byte| byte == 0)
    }
}

/// The authentication a network asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Security {
    /// No encryption.
    Open,
    /// Opportunistic encryption without authentication (OWE, "enhanced open").
    Owe,
    /// Encryption without an RSN or WPA element: a WEP key.
    Wep,
    /// WPA (the pre-RSN element) with a pre-shared key.
    Wpa,
    /// WPA2: RSN with a pre-shared key.
    Wpa2,
    /// WPA3: RSN with SAE.
    Wpa3,
    /// Transition mode: RSN offering both a pre-shared key and SAE.
    Wpa2Wpa3,
    /// 802.1X: a user name and credentials, not a passphrase.
    Enterprise,
    /// An RSN element with no key management suite this crate maps.
    Unsupported,
}

/// What the key management suites of an element allow.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Akms {
    psk: bool,
    sae: bool,
    owe: bool,
    eap: bool,
}

impl Akms {
    /// AKM suite types of the 00:0F:AC OUI (IEEE 802.11-2020, AKM suite selectors table; 24 and
    /// 25 are from IEEE 802.11-2024).
    fn add_ieee(&mut self, suite_type: u8) {
        match suite_type {
            2 | 4 | 6 | 19 | 20 => self.psk = true,
            8 | 9 | 24 | 25 => self.sae = true,
            18 => self.owe = true,
            1 | 3 | 5 | 11 | 12 | 13 | 14 | 15 | 16 | 17 => self.eap = true,
            _ => {}
        }
    }

    /// AKM suite types of the WPA element: 1 is 802.1X, 2 is a pre-shared key.
    fn add_wpa(&mut self, suite_type: u8) {
        match suite_type {
            1 => self.eap = true,
            2 => self.psk = true,
            _ => {}
        }
    }

    fn rsn_security(self) -> Security {
        if self.sae && self.psk {
            Security::Wpa2Wpa3
        } else if self.sae {
            Security::Wpa3
        } else if self.psk {
            Security::Wpa2
        } else if self.owe {
            Security::Owe
        } else if self.eap {
            Security::Enterprise
        } else {
            Security::Unsupported
        }
    }

    fn wpa_security(self) -> Security {
        if self.psk {
            Security::Wpa
        } else if self.eap {
            Security::Enterprise
        } else {
            Security::Unsupported
        }
    }
}

/// A forward-only reader over an element's data.
struct Cursor<'a> {
    rest: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize, what: &'static str) -> Result<&'a [u8], ParseError> {
        if self.rest.len() < count {
            return Err(ParseError::SecurityElement { what });
        }
        let (taken, rest) = self.rest.split_at(count);
        self.rest = rest;
        Ok(taken)
    }

    fn u16(&mut self, what: &'static str) -> Result<u16, ParseError> {
        let bytes = self.take(2, what)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}

/// Reads an RSN element's body (`version | group cipher | pairwise count | pairwise suites | AKM
/// count | AKM suites | ...`) or a WPA element's body after the vendor prefix (same layout).
fn read_akms(body: &[u8], oui: [u8; 3], add: fn(&mut Akms, u8)) -> Result<Akms, ParseError> {
    let mut cursor = Cursor { rest: body };
    cursor.take(2, "version")?;
    cursor.take(4, "group cipher suite")?;
    let pairwise = cursor.u16("pairwise suite count")?;
    cursor.take(usize::from(pairwise) * 4, "pairwise cipher suites")?;
    let count = cursor.u16("AKM suite count")?;
    let suites = cursor.take(usize::from(count) * 4, "AKM suites")?;
    let mut akms = Akms::default();
    for suite in suites.as_chunks::<4>().0 {
        if suite[..3] == oui {
            add(&mut akms, suite[3]);
        }
    }
    Ok(akms)
}

/// The SSID and the security of a network, from its information elements and capability field.
/// An RSN element wins over a WPA element (a network may carry both); with neither, the privacy
/// bit tells WEP from open.
pub fn ssid_and_security(elements: &[u8], capability: u16) -> Result<(Ssid, Security), ParseError> {
    let (mut ssid, mut rsn, mut wpa) = (Ssid(Vec::new()), None, None);
    let mut rest = elements;
    while !rest.is_empty() {
        if rest.len() < 2 {
            return Err(ParseError::Truncated {
                what: "information element header",
                needed: 2,
                have: rest.len(),
            });
        }
        let (id, length) = (rest[0], usize::from(rest[1]));
        let data = rest[2..]
            .get(..length)
            .ok_or(ParseError::InformationElement {
                id,
                length,
                available: rest.len() - 2,
            })?;
        match id {
            ELEMENT_SSID => ssid = Ssid(data.to_vec()),
            ELEMENT_RSN => rsn = Some(read_akms(data, IEEE_OUI, Akms::add_ieee)?.rsn_security()),
            ELEMENT_VENDOR if data.starts_with(&WPA_PREFIX) => {
                wpa = Some(
                    read_akms(&data[WPA_PREFIX.len()..], WPA_OUI, Akms::add_wpa)?.wpa_security(),
                )
            }
            _ => {}
        }
        rest = &rest[2 + length..];
    }
    let security = match (rsn, wpa) {
        (Some(security), _) | (None, Some(security)) => security,
        (None, None) if capability & CAPABILITY_PRIVACY != 0 => Security::Wep,
        (None, None) => Security::Open,
    };
    Ok((ssid, security))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `id | length | data`.
    fn element(id: u8, data: &[u8]) -> Vec<u8> {
        let mut bytes = vec![id, u8::try_from(data.len()).unwrap()];
        bytes.extend_from_slice(data);
        bytes
    }

    /// An RSN element body (IEEE 802.11-2020, RSNE format): version 1, group cipher CCMP (00-0F-AC:4),
    /// one pairwise cipher CCMP, then the AKM suites given as 00-0F-AC types, then the RSN
    /// capabilities (0).
    fn rsn(akm_types: &[u8]) -> Vec<u8> {
        let mut body = vec![1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4];
        body.extend_from_slice(&u16::try_from(akm_types.len()).unwrap().to_le_bytes());
        for suite_type in akm_types {
            body.extend_from_slice(&[0x00, 0x0f, 0xac, *suite_type]);
        }
        body.extend_from_slice(&[0, 0]);
        element(ELEMENT_RSN, &body)
    }

    /// The WPA vendor element: OUI 00:50:F2 type 1, version 1, group TKIP (00-50-F2:2), one
    /// pairwise TKIP, then the AKM suites given as 00-50-F2 types.
    fn wpa(akm_types: &[u8]) -> Vec<u8> {
        let mut body = vec![
            0x00, 0x50, 0xf2, 0x01, 1, 0, 0x00, 0x50, 0xf2, 2, 1, 0, 0x00, 0x50, 0xf2, 2,
        ];
        body.extend_from_slice(&u16::try_from(akm_types.len()).unwrap().to_le_bytes());
        for suite_type in akm_types {
            body.extend_from_slice(&[0x00, 0x50, 0xf2, *suite_type]);
        }
        element(ELEMENT_VENDOR, &body)
    }

    fn network(ssid: &[u8], security_elements: &[Vec<u8>], capability: u16) -> (Ssid, Security) {
        let mut elements = element(ELEMENT_SSID, ssid);
        // supported rates, which every network carries and which are no concern of ours
        elements.extend(element(1, &[0x82, 0x84, 0x8b, 0x96]));
        for security in security_elements {
            elements.extend_from_slice(security);
        }
        ssid_and_security(&elements, capability).unwrap()
    }

    #[test]
    fn an_open_network_has_no_security_element_and_no_privacy_bit() {
        assert_eq!(
            network(b"cafe", &[], 0x0401),
            (Ssid(b"cafe".to_vec()), Security::Open)
        );
    }

    #[test]
    fn the_privacy_bit_without_a_security_element_is_wep() {
        assert_eq!(network(b"old", &[], 0x0411).1, Security::Wep);
    }

    #[test]
    fn the_wpa_vendor_element_with_a_pre_shared_key_is_wpa() {
        assert_eq!(network(b"legacy", &[wpa(&[2])], 0x0411).1, Security::Wpa);
    }

    #[test]
    fn rsn_with_a_pre_shared_key_is_wpa2() {
        assert_eq!(network(b"home", &[rsn(&[2])], 0x0411).1, Security::Wpa2);
        // FT-PSK (4) and PSK-SHA256 (6) are pre-shared keys too
        assert_eq!(network(b"home", &[rsn(&[4, 6])], 0x0411).1, Security::Wpa2);
    }

    #[test]
    fn rsn_with_sae_is_wpa3() {
        assert_eq!(network(b"modern", &[rsn(&[8])], 0x0411).1, Security::Wpa3);
    }

    #[test]
    fn rsn_offering_psk_and_sae_is_the_transition_mode() {
        assert_eq!(
            network(b"both", &[rsn(&[2, 8])], 0x0411).1,
            Security::Wpa2Wpa3
        );
    }

    #[test]
    fn rsn_with_802_1x_is_enterprise() {
        assert_eq!(
            network(b"corp", &[rsn(&[1])], 0x0411).1,
            Security::Enterprise
        );
        assert_eq!(
            network(b"corp", &[wpa(&[1])], 0x0411).1,
            Security::Enterprise
        );
    }

    #[test]
    fn rsn_with_owe_is_enhanced_open() {
        assert_eq!(network(b"guest", &[rsn(&[18])], 0x0401).1, Security::Owe);
    }

    #[test]
    fn rsn_wins_over_the_wpa_element() {
        assert_eq!(
            network(b"mixed", &[wpa(&[2]), rsn(&[2])], 0x0411).1,
            Security::Wpa2
        );
    }

    #[test]
    fn an_unknown_suite_is_unsupported() {
        assert_eq!(
            network(b"odd", &[rsn(&[7])], 0x0411).1,
            Security::Unsupported
        );
    }

    #[test]
    fn a_suite_of_another_oui_is_ignored() {
        let mut body = vec![1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0];
        body.extend_from_slice(&[0x00, 0x50, 0xf2, 2, 0, 0]);
        assert_eq!(
            network(b"x", &[element(ELEMENT_RSN, &body)], 0x0411).1,
            Security::Unsupported
        );
    }

    #[test]
    fn a_hidden_network_has_an_empty_or_zeroed_name() {
        let (ssid, _) = network(b"", &[rsn(&[2])], 0x0411);
        assert!(ssid.is_hidden());
        assert!(Ssid(vec![0, 0, 0]).is_hidden());
        assert!(!Ssid(b"a".to_vec()).is_hidden());
    }

    #[test]
    fn an_ssid_that_is_not_utf8_displays_with_replacement_characters() {
        assert_eq!(Ssid(vec![b'a', 0xff]).to_text(), "a\u{fffd}");
    }

    #[test]
    fn an_element_longer_than_the_list_is_an_error() {
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
    fn an_rsn_element_cut_inside_the_akm_list_is_an_error() {
        let mut body = rsn(&[2]);
        // keep the element framing consistent: drop the last 6 bytes (the AKM suite and capabilities)
        body.truncate(body.len() - 6);
        body[1] -= 6;
        assert_eq!(
            ssid_and_security(&body, 0),
            Err(ParseError::SecurityElement { what: "AKM suites" })
        );
    }
}
