//! Links, addresses and routes decoded from rtnetlink messages.

use cupertino_netlink::message::{frame, messages, Body, NLMSG_DONE};
use cupertino_netlink::rtnetlink::{
    parse_address, parse_addresses, parse_event, parse_link, parse_links, parse_route,
    parse_routes, Family, LinkClass, LinkFlags, OperState, RouteEvent, Scope, PROTOCOL_DHCP,
    RTM_DELADDR, RTM_DELLINK, RTM_DELROUTE, RTM_GETLINK, RTM_NEWADDR, RTM_NEWLINK, RTM_NEWROUTE,
    TABLE_MAIN,
};
use cupertino_netlink::{Error, MacAddr, ParseError};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

const IFF_UP: u32 = 0x1;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
const IFF_LOWER_UP: u32 = 0x1_0000;

fn link_header(hardware_type: u16, index: u32, flags: u32) -> [u8; 16] {
    let mut header = [0u8; 16];
    header[2..4].copy_from_slice(&hardware_type.to_ne_bytes());
    header[4..8].copy_from_slice(&index.to_ne_bytes());
    header[8..12].copy_from_slice(&flags.to_ne_bytes());
    header
}

fn link_payload(name: &str, hardware_type: u16, index: u32, flags: u32) -> Body {
    let mut body = Body::new();
    body.header(&link_header(hardware_type, index, flags))
        .attr_cstr(3, name);
    body
}

fn address_header(family: u8, prefix: u8, flags: u8, scope: u8, index: u32) -> [u8; 8] {
    let mut header = [0u8; 8];
    header[0] = family;
    header[1] = prefix;
    header[2] = flags;
    header[3] = scope;
    header[4..8].copy_from_slice(&index.to_ne_bytes());
    header
}

fn route_header(family: u8, dst_len: u8, table: u8, protocol: u8, scope: u8, kind: u8) -> [u8; 12] {
    let mut header = [0u8; 12];
    header[0] = family;
    header[1] = dst_len;
    header[4] = table;
    header[5] = protocol;
    header[6] = scope;
    header[7] = kind;
    header
}

#[test]
fn a_link_decodes_with_all_its_attributes() {
    let mut body = link_payload("eth0", 1, 2, IFF_UP | IFF_RUNNING | IFF_LOWER_UP);
    body.attr(1, &[0x02, 0x00, 0x5E, 0x10, 0x20, 0x30])
        .attr_u32(4, 1500)
        .attr(16, &[6])
        .attr_u32(10, 9)
        .nested(18, |info| {
            info.attr_cstr(1, "veth");
        });
    let link = parse_link(body.as_bytes()).unwrap();
    assert_eq!(link.index, 2);
    assert_eq!(link.name, "eth0");
    assert_eq!(link.hardware_type, 1);
    assert!(link.flags.is_up() && link.flags.is_running() && link.flags.has_carrier());
    assert!(!link.flags.is_loopback());
    assert_eq!(link.oper_state, OperState::Up);
    assert_eq!(
        link.mac,
        Some(MacAddr([0x02, 0x00, 0x5E, 0x10, 0x20, 0x30]))
    );
    assert_eq!(link.mtu, Some(1500));
    assert_eq!(link.info_kind.as_deref(), Some("veth"));
    assert_eq!(link.master, Some(9));
}

#[test]
fn a_link_with_only_a_name_has_no_optional_parts() {
    let link = parse_link(link_payload("lo", 772, 1, IFF_LOOPBACK).as_bytes()).unwrap();
    assert_eq!(link.mac, None);
    assert_eq!(link.mtu, None);
    assert_eq!(link.info_kind, None);
    assert_eq!(link.master, None);
    assert_eq!(link.oper_state, OperState::Unknown);
}

#[test]
fn a_hardware_address_that_is_not_six_bytes_is_left_out() {
    let mut body = link_payload("ppp0", 512, 5, 0);
    body.attr(1, &[1, 2, 3, 4]);
    assert_eq!(parse_link(body.as_bytes()).unwrap().mac, None);
}

#[test]
fn a_link_without_a_name_is_refused() {
    let mut body = Body::new();
    body.header(&link_header(1, 2, 0)).attr_u32(4, 1500);
    assert_eq!(
        parse_link(body.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "link",
            attribute: "IFLA_IFNAME"
        })
    );
}

#[test]
fn a_link_payload_shorter_than_its_header_is_truncated() {
    assert_eq!(
        parse_link(&[0; 15]),
        Err(ParseError::Truncated {
            what: "ifinfomsg",
            needed: 16,
            have: 15
        })
    );
}

#[test]
fn a_link_attribute_of_the_wrong_size_fails_the_link() {
    let mut body = link_payload("eth0", 1, 2, 0);
    body.attr(4, &[1, 2]);
    assert!(matches!(
        parse_link(body.as_bytes()),
        Err(ParseError::AttributeSize { attribute: 4, .. })
    ));
}

#[test]
fn operational_states_follow_rfc_2863() {
    for (raw, expected) in [
        (0u8, OperState::Unknown),
        (1, OperState::NotPresent),
        (2, OperState::Down),
        (3, OperState::LowerLayerDown),
        (4, OperState::Testing),
        (5, OperState::Dormant),
        (6, OperState::Up),
        (7, OperState::Other(7)),
    ] {
        let mut body = link_payload("x", 1, 1, 0);
        body.attr(16, &[raw]);
        assert_eq!(parse_link(body.as_bytes()).unwrap().oper_state, expected);
    }
}

#[test]
fn link_flags_are_independent_bits() {
    let flags = LinkFlags(IFF_UP | IFF_LOOPBACK);
    assert!(flags.is_up() && flags.is_loopback());
    assert!(!flags.is_running() && !flags.has_carrier());
}

fn classify(name: &str, hardware_type: u16, flags: u32, kind: Option<&str>) -> (u32, LinkClass) {
    let mut body = link_payload(name, hardware_type, 3, flags);
    if let Some(kind) = kind {
        body.nested(18, |info| {
            info.attr_cstr(1, kind);
        });
    }
    let link = parse_link(body.as_bytes()).unwrap();
    (link.index, link.class(&[7]))
}

#[test]
fn a_link_is_classified_by_type_flags_driver_and_the_wireless_list() {
    assert_eq!(
        classify("lo", 772, IFF_LOOPBACK, None).1,
        LinkClass::Loopback
    );
    assert_eq!(classify("lo", 1, IFF_LOOPBACK, None).1, LinkClass::Loopback);
    assert_eq!(classify("eno1", 1, 0, None).1, LinkClass::Wired);
    assert_eq!(classify("br0", 1, 0, Some("bridge")).1, LinkClass::Virtual);
    assert_eq!(classify("tun0", 65534, 0, None).1, LinkClass::Virtual);

    let mut body = link_payload("wlan0", 1, 7, 0);
    body.nested(18, |info| {
        info.attr_cstr(1, "wlan");
    });
    let wireless = parse_link(body.as_bytes()).unwrap();
    assert_eq!(wireless.class(&[7]), LinkClass::Wireless);
    assert_eq!(wireless.class(&[]), LinkClass::Virtual);
}

#[test]
fn an_ipv4_address_prefers_the_local_attribute_over_the_peer() {
    let mut body = Body::new();
    body.header(&address_header(2, 24, 0, 0, 2))
        .attr(1, &[10, 0, 0, 99])
        .attr(2, &[10, 0, 0, 5])
        .attr_cstr(3, "eth0");
    let address = parse_address(body.as_bytes()).unwrap().unwrap();
    assert_eq!(address.index, 2);
    assert_eq!(address.family, Family::V4);
    assert_eq!(address.prefix_len, 24);
    assert_eq!(address.scope, Scope::Universe);
    assert_eq!(address.address, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)));
    assert_eq!(address.label.as_deref(), Some("eth0"));
}

#[test]
fn an_ipv6_address_uses_its_address_attribute_and_the_wide_flags() {
    let mut body = Body::new();
    body.header(&address_header(10, 64, 0x80, 253, 3))
        .attr(1, &Ipv6Addr::LOCALHOST.octets())
        .attr_u32(8, 0x40);
    let address = parse_address(body.as_bytes()).unwrap().unwrap();
    assert_eq!(address.family, Family::V6);
    assert_eq!(address.scope, Scope::Link);
    assert_eq!(address.address, IpAddr::V6(Ipv6Addr::LOCALHOST));
    assert_eq!(address.flags, 0x40);
    assert!(address.is_tentative());
    assert_eq!(address.label, None);
}

#[test]
fn without_the_wide_flags_the_header_flags_apply() {
    let mut body = Body::new();
    body.header(&address_header(2, 8, 0x40, 254, 1))
        .attr(2, &[127, 0, 0, 1]);
    let address = parse_address(body.as_bytes()).unwrap().unwrap();
    assert_eq!(address.flags, 0x40);
    assert!(address.is_tentative());
    assert_eq!(address.scope, Scope::Host);
}

#[test]
fn scopes_keep_values_the_crate_does_not_name() {
    for (raw, expected) in [
        (0u8, Scope::Universe),
        (200, Scope::Site),
        (253, Scope::Link),
        (254, Scope::Host),
        (77, Scope::Other(77)),
    ] {
        let mut body = Body::new();
        body.header(&address_header(2, 8, 0, raw, 1))
            .attr(2, &[1, 1, 1, 1]);
        assert_eq!(
            parse_address(body.as_bytes()).unwrap().unwrap().scope,
            expected
        );
    }
}

#[test]
fn an_address_of_another_family_is_skipped_and_one_without_an_address_is_refused() {
    let mut other = Body::new();
    other.header(&address_header(16, 0, 0, 0, 1));
    assert_eq!(parse_address(other.as_bytes()), Ok(None));
    let mut none = Body::new();
    none.header(&address_header(2, 8, 0, 0, 1));
    assert_eq!(
        parse_address(none.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "address",
            attribute: "IFA_ADDRESS"
        })
    );
    assert!(matches!(
        parse_address(&[2, 0, 0]),
        Err(ParseError::Truncated {
            what: "ifaddrmsg",
            ..
        })
    ));
}

#[test]
fn a_default_route_has_no_destination_and_goes_through_a_gateway() {
    let mut body = Body::new();
    body.header(&route_header(2, 0, 254, 16, 0, 1))
        .attr(5, &[192, 168, 1, 1])
        .attr_u32(4, 3)
        .attr_u32(6, 600)
        .attr(7, &[192, 168, 1, 20]);
    let route = parse_route(body.as_bytes()).unwrap().unwrap();
    assert_eq!(route.family, Family::V4);
    assert_eq!(route.destination, None);
    assert_eq!(route.dst_len, 0);
    assert_eq!(
        route.gateway,
        Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)))
    );
    assert_eq!(route.index, Some(3));
    assert_eq!(route.metric, Some(600));
    assert_eq!(route.table, TABLE_MAIN);
    assert_eq!(route.protocol, PROTOCOL_DHCP);
    assert_eq!(route.route_type, 1);
    assert_eq!(
        route.preferred_source,
        Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)))
    );
    assert!(route.is_default());
}

#[test]
fn a_route_is_default_only_when_it_is_unicast_to_everything_in_the_main_table() {
    let route = |dst_len, table, kind| {
        let mut body = Body::new();
        body.header(&route_header(2, dst_len, table, 4, 0, kind));
        parse_route(body.as_bytes()).unwrap().unwrap().is_default()
    };
    assert!(route(0, 254, 1));
    assert!(!route(24, 254, 1));
    assert!(!route(0, 100, 1));
    assert!(!route(0, 254, 7));
}

#[test]
fn the_table_attribute_overrides_the_eight_bit_header_field() {
    let mut body = Body::new();
    body.header(&route_header(2, 24, 252, 4, 253, 1))
        .attr(1, &[10, 1, 0, 0])
        .attr_u32(15, 1000);
    let route = parse_route(body.as_bytes()).unwrap().unwrap();
    assert_eq!(route.table, 1000);
    assert_eq!(
        route.destination,
        Some(IpAddr::V4(Ipv4Addr::new(10, 1, 0, 0)))
    );
    assert_eq!(route.scope, Scope::Link);
    assert!(!route.is_default());
}

#[test]
fn a_route_of_another_family_is_skipped_and_a_short_one_is_truncated() {
    let mut other = Body::new();
    other.header(&route_header(29, 0, 254, 4, 0, 1));
    assert_eq!(parse_route(other.as_bytes()), Ok(None));
    assert!(matches!(
        parse_route(&[2; 11]),
        Err(ParseError::Truncated { what: "rtmsg", .. })
    ));
}

fn dump(ty: u16, payloads: &[Body]) -> Vec<u8> {
    let mut raw = Vec::new();
    for (sequence, payload) in payloads.iter().enumerate() {
        raw.extend(frame(ty, 2, sequence as u32 + 1, payload.as_bytes()));
    }
    raw.extend(frame(NLMSG_DONE, 2, 99, &[0; 4]));
    raw
}

#[test]
fn a_link_dump_lists_the_links_and_skips_the_done_message() {
    let raw = dump(
        RTM_NEWLINK,
        &[
            link_payload("lo", 772, 1, IFF_LOOPBACK),
            link_payload("eth0", 1, 2, IFF_UP),
        ],
    );
    let links = parse_links(&raw).unwrap();
    assert_eq!(
        links.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        ["lo", "eth0"]
    );
}

#[test]
fn messages_of_other_types_in_a_dump_are_ignored() {
    let mut raw = frame(RTM_GETLINK, 0, 1, &[0; 16]);
    raw.extend(dump(RTM_NEWLINK, &[link_payload("eth0", 1, 2, 0)]));
    assert_eq!(parse_links(&raw).unwrap().len(), 1);
}

#[test]
fn a_damaged_dump_says_which_request_it_answered() {
    let mut raw = dump(RTM_NEWLINK, &[link_payload("eth0", 1, 2, 0)]);
    raw[0..4].copy_from_slice(&4000u32.to_ne_bytes());
    let error = parse_links(&raw).unwrap_err();
    assert!(matches!(
        &error,
        Error::Parse {
            origin: "RTM_GETLINK",
            source: ParseError::BadMessageLength { .. }
        }
    ));
    assert!(error.to_string().starts_with("RTM_GETLINK: "));

    let mut nameless = Body::new();
    nameless.header(&link_header(1, 2, 0));
    let error = parse_links(&dump(RTM_NEWLINK, &[nameless])).unwrap_err();
    assert!(matches!(
        error,
        Error::Parse {
            origin: "RTM_GETLINK",
            ..
        }
    ));
}

#[test]
fn address_and_route_dumps_skip_other_families() {
    let mut v4 = Body::new();
    v4.header(&address_header(2, 24, 0, 0, 2))
        .attr(2, &[10, 0, 0, 5]);
    let mut other = Body::new();
    other.header(&address_header(16, 0, 0, 0, 2));
    let addresses = parse_addresses(&dump(RTM_NEWADDR, &[v4, other])).unwrap();
    assert_eq!(addresses.len(), 1);

    let mut route = Body::new();
    route.header(&route_header(2, 0, 254, 4, 0, 1));
    let mut other = Body::new();
    other.header(&route_header(16, 0, 254, 4, 0, 1));
    let routes = parse_routes(&dump(RTM_NEWROUTE, &[route, other])).unwrap();
    assert_eq!(routes.len(), 1);
}

#[test]
fn a_message_of_the_multicast_groups_becomes_an_event() {
    let link = frame(RTM_DELLINK, 0, 0, link_payload("eth0", 1, 2, 0).as_bytes());
    let mut address = Body::new();
    address
        .header(&address_header(2, 24, 0, 0, 2))
        .attr(2, &[10, 0, 0, 5]);
    let new_address = frame(RTM_NEWADDR, 0, 0, address.as_bytes());
    let del_address = frame(RTM_DELADDR, 0, 0, address.as_bytes());
    let mut route = Body::new();
    route.header(&route_header(2, 0, 254, 4, 0, 1));
    let del_route = frame(RTM_DELROUTE, 0, 0, route.as_bytes());
    let new_route = frame(RTM_NEWROUTE, 0, 0, route.as_bytes());
    let new_link = frame(RTM_NEWLINK, 0, 0, link_payload("eth0", 1, 2, 0).as_bytes());

    let decode = |bytes: &[u8]| {
        parse_event(&messages(bytes).next().unwrap().unwrap())
            .unwrap()
            .unwrap()
    };
    assert!(matches!(decode(&new_link), RouteEvent::LinkNew(l) if l.name == "eth0"));
    assert!(matches!(decode(&link), RouteEvent::LinkDeleted(l) if l.index == 2));
    assert!(matches!(decode(&new_address), RouteEvent::AddressNew(_)));
    assert!(matches!(
        decode(&del_address),
        RouteEvent::AddressDeleted(_)
    ));
    assert!(matches!(decode(&new_route), RouteEvent::RouteNew(_)));
    assert!(matches!(decode(&del_route), RouteEvent::RouteDeleted(_)));
}

#[test]
fn a_message_that_is_not_about_the_network_is_no_event() {
    let other = frame(2, 0, 0, &[0; 4]);
    assert_eq!(
        parse_event(&messages(&other).next().unwrap().unwrap()),
        Ok(None)
    );
    let mut v6_other = Body::new();
    v6_other.header(&address_header(16, 0, 0, 0, 2));
    let skipped = frame(RTM_NEWADDR, 0, 0, v6_other.as_bytes());
    assert_eq!(
        parse_event(&messages(&skipped).next().unwrap().unwrap()),
        Ok(None)
    );
}

#[test]
fn a_damaged_event_is_an_error_for_that_event() {
    let broken = frame(RTM_NEWLINK, 0, 0, &[0; 16]);
    assert!(matches!(
        parse_event(&messages(&broken).next().unwrap().unwrap()),
        Err(ParseError::MissingAttribute { .. })
    ));
}
