//! Records this machine's rtnetlink and nl80211 replies as binary fixtures.
//!
//! ```text
//! cargo run -p cupertino-netlink --example record-netlink -- crates/cupertino-netlink/tests/fixtures
//! ```
//!
//! Only dumps are requested (read-only, no capability needed); no scan is triggered. Before a
//! file is written, everything that identifies a network or a machine is replaced by a fixed
//! fake value of the same size, so the framing stays valid:
//!
//! - every hardware address (also inside IPv6 interface identifiers and bridge identifiers),
//! - every SSID, and the contents of vendor elements other than the WPA element (WPS names),
//! - the first 6 bytes of every IPv6 address that is not link-local, loopback or multicast, and
//!   the interface identifier of link-local addresses not derived from a hardware address.
//!
//! The example refuses to write when a public IPv4 address is in a dump, or when a replaced
//! value is still found in the output.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use cupertino_netlink::attr::attrs;
use cupertino_netlink::message::{messages, Message};
use cupertino_netlink::nl80211::{self, InterfaceType, Nl80211};
use cupertino_netlink::rtnetlink::{parse_addresses, parse_links, parse_routes, RouteSocket};
use cupertino_netlink::socket::{Mode, Socket};
use rustix::net::netlink::GENERIC;

const ATTR_SSID: u16 = 52;
const ATTR_BSS: u16 = 47;
const BSS_INFORMATION_ELEMENTS: u16 = 6;
const BSS_BEACON_IES: u16 = 11;
const ELEMENT_SSID: u8 = 0;
const ELEMENT_VENDOR: u8 = 221;
const WPA_PREFIX: [u8; 4] = [0x00, 0x50, 0xf2, 0x01];

struct Capture {
    name: &'static str,
    bytes: Vec<u8>,
}

/// The bytes of `part` start at this offset of `whole`.
fn offset(whole: &[u8], part: &[u8]) -> usize {
    part.as_ptr() as usize - whole.as_ptr() as usize
}

/// The whole message (header included) that `message` was read from `raw`.
fn message_bytes<'a>(raw: &'a [u8], message: &Message<'a>) -> &'a [u8] {
    let start = offset(raw, message.payload) - 16;
    &raw[start..start + message.header.length as usize]
}

fn replace_all(buffer: &mut [u8], from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let mut at = 0;
    while at + from.len() <= buffer.len() {
        if &buffer[at..at + from.len()] == from {
            buffer[at..at + from.len()].copy_from_slice(to);
            at += from.len();
        } else {
            at += 1;
        }
    }
}

fn contains(buffer: &[u8], needle: &[u8]) -> bool {
    buffer.windows(needle.len()).any(|window| window == needle)
}

/// The 8-byte IPv6 interface identifier derived from a MAC (modified EUI-64).
fn eui64(mac: [u8; 6]) -> [u8; 8] {
    [
        mac[0] ^ 2,
        mac[1],
        mac[2],
        0xff,
        0xfe,
        mac[3],
        mac[4],
        mac[5],
    ]
}

/// The 12 lower-case hex digits of a MAC, without separators.
fn hex(mac: &[u8; 6]) -> String {
    mac.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_private(address: Ipv4Addr) -> bool {
    address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_multicast()
        || address.is_broadcast()
}

#[derive(Default)]
struct Scrub {
    macs: BTreeMap<[u8; 6], [u8; 6]>,
    ssids: BTreeMap<Vec<u8>, Vec<u8>>,
    prefixes: BTreeMap<[u8; 6], [u8; 6]>,
    identifiers: BTreeMap<[u8; 8], [u8; 8]>,
}

impl Scrub {
    fn mac(&mut self, real: [u8; 6]) {
        if real == [0; 6] || real == [0xff; 6] {
            return;
        }
        let next = self.macs.len() as u16 + 1;
        let [high, low] = next.to_be_bytes();
        self.macs.entry(real).or_insert([0x02, 0, 0, 0, high, low]);
    }

    fn ssid(&mut self, real: &[u8]) {
        if real.iter().all(|&byte| byte == 0) || self.ssids.contains_key(real) {
            return;
        }
        let number = self.ssids.len() + 1;
        let mut fake = format!("net{number}-").into_bytes();
        fake.resize(real.len(), b'x');
        self.ssids.insert(real.to_vec(), fake);
    }

    fn prefix(&mut self, octets: [u8; 16]) {
        let first = octets[0];
        let link_local = first == 0xfe && octets[1] & 0xc0 == 0x80;
        if link_local {
            // An interface identifier that is not derived from a hardware address (stable-privacy
            // addresses) still identifies the machine.
            let identifier: [u8; 8] = octets[8..].try_into().unwrap();
            let derived_from_mac = identifier[3] == 0xff && identifier[4] == 0xfe;
            // all zero: the fe80::/64 route, which names no host
            if !derived_from_mac && identifier != [0; 8] {
                let number = self.identifiers.len() as u8 + 1;
                self.identifiers
                    .entry(identifier)
                    .or_insert([0, 0, 0, 0, 0, 0, 0, number]);
            }
            return;
        }
        if first == 0xff || octets[..15] == [0; 15] {
            return;
        }
        let number = self.prefixes.len() as u8 + 1;
        self.prefixes
            .entry(octets[..6].try_into().unwrap())
            .or_insert([0x20, 0x01, 0x0d, 0xb8, 0, number]);
    }
}

/// Offsets and fake contents of the SSID bytes in the file, and of vendor elements to blank.
fn element_patches(
    scrub: &Scrub,
    raw: &[u8],
    elements: &[u8],
    patches: &mut Vec<(usize, Vec<u8>)>,
) {
    let mut at = 0;
    while at + 2 <= elements.len() {
        let (id, length) = (elements[at], usize::from(elements[at + 1]));
        let data = &elements[at + 2..at + 2 + length];
        let start = offset(raw, data);
        if id == ELEMENT_SSID {
            if let Some(fake) = scrub.ssids.get(data) {
                patches.push((start, fake.clone()));
            }
        } else if id == ELEMENT_VENDOR && !data.starts_with(&WPA_PREFIX) {
            patches.push((start, vec![0; length]));
        }
        at += 2 + length;
    }
}

/// A byte range to overwrite: the offset and the replacement.
type Patch = (usize, Vec<u8>);

fn patches(scrub: &Scrub, raw: &[u8], family: u16) -> Result<Vec<Patch>, Box<dyn Error>> {
    let mut patches = Vec::new();
    for message in messages(raw) {
        let message = message?;
        if message.header.ty != family {
            continue;
        }
        for attr in attrs(&message.payload[4..]) {
            let attr = attr?;
            match attr.ty {
                ATTR_SSID => {
                    if let Some(fake) = scrub.ssids.get(attr.data) {
                        patches.push((offset(raw, attr.data), fake.clone()));
                    }
                }
                ATTR_BSS => {
                    for child in attr.nested() {
                        let child = child?;
                        if matches!(child.ty, BSS_INFORMATION_ELEMENTS | BSS_BEACON_IES) {
                            element_patches(scrub, raw, child.data, &mut patches);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(patches)
}

fn main() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: record-netlink OUT_DIR")?,
    );
    let mut files = Vec::new();
    let mut scrub = Scrub::default();

    let mut route = RouteSocket::open()?;
    let links_raw = route.dump_links_raw()?;
    let addresses_raw = route.dump_addresses_raw()?;
    let routes_raw = route.dump_routes_raw()?;
    for link in parse_links(&links_raw)? {
        link.mac.into_iter().for_each(|mac| scrub.mac(mac.0));
    }
    for address in parse_addresses(&addresses_raw)? {
        match address.address {
            IpAddr::V4(v4) if !is_private(v4) => {
                return Err(format!("public IPv4 address {v4} in the address dump").into())
            }
            IpAddr::V4(_) => {}
            IpAddr::V6(v6) => scrub.prefix(v6.octets()),
        }
    }
    for route in parse_routes(&routes_raw)? {
        for address in [route.destination, route.gateway, route.preferred_source]
            .into_iter()
            .flatten()
        {
            match address {
                IpAddr::V4(v4) if !is_private(v4) => {
                    return Err(format!("public IPv4 address {v4} in the route dump").into())
                }
                IpAddr::V4(_) => {}
                IpAddr::V6(v6) => scrub.prefix(v6.octets()),
            }
        }
    }
    files.push(Capture {
        name: "rtm-getlink",
        bytes: links_raw,
    });
    files.push(Capture {
        name: "rtm-getaddr",
        bytes: addresses_raw,
    });
    files.push(Capture {
        name: "rtm-getroute",
        bytes: routes_raw,
    });

    let mut family_id = None;
    match Nl80211::open() {
        Ok(mut nl) => {
            let family = nl.family().id;
            family_id = Some(family);
            let mut resolver = Socket::open(Some(GENERIC), 0, Mode::Blocking)?;
            files.push(Capture {
                name: "genl-getfamily-nl80211",
                bytes: nl80211::family_raw(&mut resolver)?,
            });

            let interfaces_raw = nl.interfaces_raw()?;
            let interfaces = nl80211::parse_interfaces(&interfaces_raw, family)?;
            for interface in &interfaces {
                interface.mac.into_iter().for_each(|mac| scrub.mac(mac.0));
                interface
                    .ssid
                    .iter()
                    .for_each(|ssid| scrub.ssid(ssid.as_bytes()));
            }
            files.push(Capture {
                name: "nl80211-get-interface",
                bytes: interfaces_raw,
            });

            // A split dump repeats the PHY's name in every part; the first part of each PHY is enough.
            let wiphys_raw = nl.wiphys_raw()?;
            let mut kept = Vec::new();
            let mut seen = Vec::new();
            for message in messages(&wiphys_raw) {
                let message = message?;
                if message.header.ty != family {
                    continue;
                }
                if let Some(wiphy) = nl80211::parse_wiphy(message.payload)? {
                    if !seen.contains(&wiphy.index) {
                        seen.push(wiphy.index);
                        kept.extend_from_slice(message_bytes(&wiphys_raw, &message));
                    }
                }
            }
            files.push(Capture {
                name: "nl80211-get-wiphy",
                bytes: kept,
            });

            if let Some(station) = interfaces
                .iter()
                .find(|interface| interface.interface_type == InterfaceType::Station)
            {
                let stations_raw = nl.stations_raw(station.ifindex)?;
                for found in nl80211::parse_stations(&stations_raw, family)? {
                    scrub.mac(found.bssid.0);
                }
                files.push(Capture {
                    name: "nl80211-get-station",
                    bytes: stations_raw,
                });

                let scan_raw = nl.scan_results_raw(station.ifindex)?;
                let dump = nl80211::parse_scan_dump(&scan_raw, family)?;
                if !dump.errors.is_empty() {
                    return Err(format!(
                        "{} access points in the scan dump do not decode: {:?}",
                        dump.errors.len(),
                        dump.errors
                    )
                    .into());
                }
                for result in &dump.results {
                    scrub.mac(result.bssid.0);
                    scrub.ssid(result.ssid.as_bytes());
                }
                files.push(Capture {
                    name: "nl80211-get-scan",
                    bytes: scan_raw,
                });
            } else {
                eprintln!("no station interface: station and scan dumps are not recorded");
            }
        }
        Err(error) if nl80211::is_unsupported(&error) => {
            eprintln!("no nl80211 in this kernel: wireless dumps are not recorded")
        }
        Err(error) => return Err(error.into()),
    }

    // Distinct real values must stay distinct.
    let distinct = |values: Vec<&Vec<u8>>| {
        values
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == values.len()
    };
    assert!(
        distinct(scrub.ssids.values().collect()),
        "two SSIDs got the same fake name"
    );

    for file in &mut files {
        if let Some(family) = family_id {
            if file.name.starts_with("nl80211-get-interface") || file.name == "nl80211-get-scan" {
                let found = patches(&scrub, &file.bytes, family)?;
                for (at, bytes) in found {
                    file.bytes[at..at + bytes.len()].copy_from_slice(&bytes);
                }
            }
        }
        for (real, fake) in &scrub.macs {
            replace_all(&mut file.bytes, real, fake);
            replace_all(&mut file.bytes, &eui64(*real), &eui64(*fake));
            // alternative interface names are built from the address (`enx74563c...`)
            replace_all(&mut file.bytes, hex(real).as_bytes(), hex(fake).as_bytes());
        }
        for (real, fake) in &scrub.prefixes {
            replace_all(&mut file.bytes, real, fake);
        }
        for (real, fake) in &scrub.identifiers {
            replace_all(&mut file.bytes, real, fake);
        }
    }

    for file in &files {
        for real in scrub.macs.keys() {
            assert!(
                !contains(&file.bytes, real),
                "{} still holds a real hardware address",
                file.name
            );
            assert!(
                !contains(&file.bytes, hex(real).as_bytes()),
                "{} still holds a real hardware address as text",
                file.name
            );
        }
        for real in scrub.ssids.keys().filter(|ssid| ssid.len() >= 3) {
            assert!(
                !contains(&file.bytes, real),
                "{} still holds a real SSID",
                file.name
            );
        }
        for real in scrub.prefixes.keys() {
            assert!(
                !contains(&file.bytes, real),
                "{} still holds a real IPv6 prefix",
                file.name
            );
        }
        for real in scrub.identifiers.keys() {
            assert!(
                !contains(&file.bytes, real),
                "{} still holds a real IPv6 interface identifier {real:02x?}",
                file.name
            );
        }
    }

    fs::create_dir_all(&out)?;
    for file in &files {
        fs::write(out.join(format!("{}.bin", file.name)), &file.bytes)?;
        println!("{}.bin: {} bytes", file.name, file.bytes.len());
    }
    println!(
        "replaced {} hardware addresses, {} SSIDs, {} IPv6 prefixes, {} IPv6 interface identifiers",
        scrub.macs.len(),
        scrub.ssids.len(),
        scrub.prefixes.len(),
        scrub.identifiers.len()
    );
    Ok(())
}
