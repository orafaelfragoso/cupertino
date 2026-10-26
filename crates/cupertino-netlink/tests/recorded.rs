//! The decoders on replies recorded from a real kernel (`tests/fixtures`, see its README). The
//! recording is scrubbed: hardware addresses are `02:00:00:00:xx:xx`, SSIDs are `netN-xxx`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use cupertino_netlink::nl80211::{self, connection, Connection, InterfaceType, ScanResult};
use cupertino_netlink::rtnetlink::{parse_addresses, parse_links, parse_routes, Family, LinkClass, OperState, Scope, PROTOCOL_DHCP};
use cupertino_netlink::{MacAddr, Security, Ssid};

const LINKS: &[u8] = include_bytes!("fixtures/rtm-getlink.bin");
const ADDRESSES: &[u8] = include_bytes!("fixtures/rtm-getaddr.bin");
const ROUTES: &[u8] = include_bytes!("fixtures/rtm-getroute.bin");
const FAMILY: &[u8] = include_bytes!("fixtures/genl-getfamily-nl80211.bin");
const INTERFACES: &[u8] = include_bytes!("fixtures/nl80211-get-interface.bin");
const WIPHY: &[u8] = include_bytes!("fixtures/nl80211-get-wiphy.bin");
const STATION: &[u8] = include_bytes!("fixtures/nl80211-get-station.bin");
const SCAN: &[u8] = include_bytes!("fixtures/nl80211-get-scan.bin");

fn family_id() -> u16 {
    nl80211::parse_family_reply(FAMILY).unwrap().id
}

#[test]
fn the_link_dump_lists_the_devices_in_index_order() {
    let links = parse_links(LINKS).unwrap();
    let names: Vec<_> = links.iter().map(|link| (link.index, link.name.as_str())).collect();
    assert_eq!(names, [(1, "lo"), (2, "enp4s0"), (3, "wlp7s0f3u4i2"), (4, "br-4244b59735c3"), (5, "br-b0dbe1b0e772"), (6, "docker0"), (7, "vethbddae96")]);
}

#[test]
fn a_cable_without_carrier_is_up_but_down() {
    let links = parse_links(LINKS).unwrap();
    let ethernet = &links[1];
    assert_eq!(ethernet.mac, Some(MacAddr([2, 0, 0, 0, 0, 1])));
    assert_eq!(ethernet.mtu, Some(1500));
    assert_eq!(ethernet.oper_state, OperState::Down);
    assert!(ethernet.flags.is_up());
    assert!(!ethernet.flags.has_carrier());
}

#[test]
fn the_wifi_link_has_a_carrier_and_is_wireless_by_the_nl80211_list() {
    let links = parse_links(LINKS).unwrap();
    let interfaces = nl80211::parse_interfaces(INTERFACES, family_id()).unwrap();
    let wireless: Vec<u32> = interfaces.iter().map(|interface| interface.ifindex).collect();
    assert_eq!(wireless, [3]);
    let wifi = &links[2];
    assert_eq!(wifi.oper_state, OperState::Up);
    assert!(wifi.flags.has_carrier());
    let classes: Vec<_> = links.iter().map(|link| link.class(&wireless)).collect();
    assert_eq!(
        classes,
        [LinkClass::Loopback, LinkClass::Wired, LinkClass::Wireless, LinkClass::Virtual, LinkClass::Virtual, LinkClass::Virtual, LinkClass::Virtual]
    );
}

#[test]
fn a_veth_names_its_driver_and_its_bridge() {
    let links = parse_links(LINKS).unwrap();
    let veth = &links[6];
    assert_eq!(veth.info_kind.as_deref(), Some("veth"));
    assert_eq!(veth.master, Some(4));
    assert_eq!(links[3].info_kind.as_deref(), Some("bridge"));
}

#[test]
fn the_address_dump_has_the_lan_address_with_its_label() {
    let addresses = parse_addresses(ADDRESSES).unwrap();
    let lan = addresses.iter().find(|address| address.address == IpAddr::V4(Ipv4Addr::new(192, 168, 68, 111))).unwrap();
    assert_eq!((lan.index, lan.family, lan.prefix_len, lan.scope), (3, Family::V4, 24, Scope::Universe));
    assert_eq!(lan.label.as_deref(), Some("wlp7s0f3u4i2"));
    // IFA_F_NOPREFIXROUTE (0x200) from the DHCP client, in the 32-bit IFA_FLAGS
    assert_eq!(lan.flags, 0x200);
}

#[test]
fn the_ipv6_addresses_are_link_local_and_loopback() {
    let addresses = parse_addresses(ADDRESSES).unwrap();
    let v6: Vec<_> = addresses.iter().filter(|address| address.family == Family::V6).map(|address| (address.index, address.address, address.scope, address.prefix_len)).collect();
    assert_eq!(
        v6,
        [
            (1, IpAddr::V6(Ipv6Addr::LOCALHOST), Scope::Host, 128),
            (3, IpAddr::V6("fe80::1".parse().unwrap()), Scope::Link, 64),
            (4, IpAddr::V6("fe80::ff:fe00:3".parse().unwrap()), Scope::Link, 64),
            (7, IpAddr::V6("fe80::ff:fe00:6".parse().unwrap()), Scope::Link, 64),
        ]
    );
}

#[test]
fn one_default_route_goes_through_the_gateway_on_the_wifi_link() {
    let routes = parse_routes(ROUTES).unwrap();
    let defaults: Vec<_> = routes.iter().filter(|route| route.is_default()).collect();
    assert_eq!(defaults.len(), 1);
    let route = defaults[0];
    assert_eq!(route.family, Family::V4);
    assert_eq!(route.gateway, Some(IpAddr::V4(Ipv4Addr::new(192, 168, 68, 1))));
    assert_eq!((route.index, route.metric, route.protocol), (Some(3), Some(600), PROTOCOL_DHCP));
    assert_eq!(route.preferred_source, Some(IpAddr::V4(Ipv4Addr::new(192, 168, 68, 111))));
}

#[test]
fn the_local_table_routes_are_not_default_routes_and_the_main_table_keeps_the_subnets() {
    let routes = parse_routes(ROUTES).unwrap();
    assert!(routes.iter().filter(|route| route.table == 255).all(|route| !route.is_default()));
    let subnet = routes.iter().find(|route| route.destination == Some(IpAddr::V4(Ipv4Addr::new(192, 168, 68, 0)))).unwrap();
    assert_eq!((subnet.dst_len, subnet.table, subnet.gateway, subnet.index), (24, 254, None, Some(3)));
}

#[test]
fn the_nl80211_family_has_its_multicast_groups() {
    let family = nl80211::parse_family_reply(FAMILY).unwrap();
    assert_eq!(family.id, 42);
    let group = |name: &str| family.groups.iter().find(|(group, _)| group == name).map(|(_, id)| *id);
    assert_eq!((group("scan"), group("mlme"), group("config")), (Some(23), Some(25), Some(22)));
}

#[test]
fn the_wireless_interface_is_a_station_on_phy0() {
    let interfaces = nl80211::parse_interfaces(INTERFACES, family_id()).unwrap();
    assert_eq!(interfaces.len(), 1);
    let interface = &interfaces[0];
    assert_eq!((interface.ifindex, interface.name.as_str(), interface.wiphy), (3, "wlp7s0f3u4i2", 0));
    assert_eq!(interface.interface_type, InterfaceType::Station);
    assert_eq!(interface.mac, Some(MacAddr([2, 0, 0, 0, 0, 2])));
    assert_eq!(interface.ssid, Some(Ssid(b"net1-xx".to_vec())));
    assert_eq!(interface.frequency_mhz, Some(5220));
    let wiphys = nl80211::parse_wiphys(WIPHY, family_id()).unwrap();
    assert_eq!((wiphys[0].index, wiphys[0].name.as_str()), (interface.wiphy, "phy0"));
}

#[test]
fn the_connection_joins_the_interface_with_its_station() {
    let interfaces = nl80211::parse_interfaces(INTERFACES, family_id()).unwrap();
    let stations = nl80211::parse_stations(STATION, family_id()).unwrap();
    assert_eq!(stations.len(), 1);
    assert_eq!(
        connection(&interfaces[0], &stations),
        Some(Connection { ifindex: 3, ssid: Some(Ssid(b"net1-xx".to_vec())), bssid: MacAddr([2, 0, 0, 0, 0, 7]), frequency_mhz: Some(5220), signal_dbm: Some(-54) })
    );
    assert_eq!(stations[0].signal_average_dbm, Some(-54));
}

fn scan() -> Vec<ScanResult> {
    let dump = nl80211::parse_scan_dump(SCAN, family_id()).unwrap();
    assert_eq!(dump.errors, []);
    dump.results
}

#[test]
fn the_scan_dump_has_the_associated_access_point_first() {
    let results = scan();
    assert_eq!(results.len(), 28);
    assert_eq!(
        results[0],
        ScanResult {
            ifindex: 3,
            bssid: MacAddr([2, 0, 0, 0, 0, 7]),
            ssid: Ssid(b"net1-xx".to_vec()),
            frequency_mhz: 5220,
            signal_dbm: Some(-56),
            security: Security::Wpa2,
            associated: true,
            seen_ms_ago: Some(99),
        }
    );
    assert_eq!(results.iter().filter(|result| result.associated).count(), 1);
}

#[test]
fn the_scan_dump_has_every_security_of_the_neighbourhood() {
    let results = scan();
    let count = |security| results.iter().filter(|result| result.security == security).count();
    assert_eq!((count(Security::Open), count(Security::Wpa), count(Security::Wpa2), count(Security::Wpa2Wpa3), count(Security::Wpa3)), (1, 1, 25, 1, 0));
    let transition = results.iter().find(|result| result.security == Security::Wpa2Wpa3).unwrap();
    assert_eq!((transition.ssid.to_text(), transition.frequency_mhz, transition.signal_dbm), ("net3-xxxxx".to_owned(), 2422, Some(-69)));
}

#[test]
fn hidden_networks_have_an_empty_name() {
    let results = scan();
    assert_eq!(results.iter().filter(|result| result.ssid.is_hidden()).count(), 6);
}

#[test]
fn two_access_points_of_one_network_share_a_name() {
    let results = scan();
    let named: Vec<_> = results.iter().filter(|result| result.ssid == Ssid(b"net1-xx".to_vec())).map(|result| (result.bssid.to_string(), result.frequency_mhz)).collect();
    assert_eq!(named, [("02:00:00:00:00:07".to_owned(), 5220), ("02:00:00:00:00:10".to_owned(), 2447)]);
}

#[test]
fn the_fixtures_hold_only_fake_hardware_addresses() {
    let fake = |mac: MacAddr| mac.0 == [0; 6] || mac.0[..4] == [2, 0, 0, 0];
    assert!(parse_links(LINKS).unwrap().iter().filter_map(|link| link.mac).all(fake));
    assert!(scan().iter().all(|result| fake(result.bssid)));
    assert!(nl80211::parse_stations(STATION, family_id()).unwrap().iter().all(|station| fake(station.bssid)));
}
