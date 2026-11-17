//! Links, addresses and routes decoded from rtnetlink messages.

use cupertino_netlink::message::{frame, messages, Body, NLMSG_DONE};
use cupertino_netlink::rtnetlink::{
    parse_address, parse_addresses, parse_event, parse_link, parse_links, parse_route,
    parse_routes, Family, LinkClass, OperState, RouteEvent, Scope, PROTOCOL_DHCP, RTM_DELADDR,
    RTM_DELLINK, RTM_DELROUTE, RTM_GETLINK, RTM_NEWADDR, RTM_NEWLINK, RTM_NEWROUTE, TABLE_MAIN,
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

fn v4_address() -> Body {
    let mut body = Body::new();
    body.header(&address_header(2, 24, 0, 0, 2))
        .attr(2, &[10, 0, 0, 5]);
    body
}

fn bare_route(family: u8) -> Body {
    let mut body = Body::new();
    body.header(&route_header(family, 0, 254, 4, 0, 1));
    body
}

#[test]
fn a_link_decodes_its_header_and_the_attributes_it_has() {
    let mut full = link_payload("eth0", 1, 2, IFF_UP | IFF_RUNNING | IFF_LOWER_UP);
    full.attr(1, &[0x02, 0x00, 0x5E, 0x10, 0x20, 0x30])
        .attr_u32(4, 1500)
        .attr(16, &[6])
        .attr_u32(10, 9)
        .nested(18, |info| {
            info.attr_cstr(1, "veth");
        });
    let link = parse_link(full.as_bytes()).unwrap();
    assert_eq!(
        (link.index, link.name.as_str(), link.hardware_type),
        (2, "eth0", 1)
    );
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

    let lo = parse_link(link_payload("lo", 772, 1, IFF_LOOPBACK).as_bytes()).unwrap();
    assert!(lo.flags.is_loopback() && !lo.flags.is_up() && !lo.flags.has_carrier());
    assert_eq!(
        (lo.mac, lo.mtu, lo.info_kind, lo.master, lo.oper_state),
        (None, None, None, None, OperState::Unknown)
    );

    let mut ppp = link_payload("ppp0", 512, 5, 0);
    ppp.attr(1, &[1, 2, 3, 4]);
    assert_eq!(
        parse_link(ppp.as_bytes()).unwrap().mac,
        None,
        "a hardware address that is not six bytes"
    );

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
        assert_eq!(
            parse_link(body.as_bytes()).unwrap().oper_state,
            expected,
            "{raw}"
        );
    }
}

#[test]
fn a_link_without_a_name_cut_short_or_with_a_wrong_size_attribute_is_refused() {
    let mut nameless = Body::new();
    nameless.header(&link_header(1, 2, 0)).attr_u32(4, 1500);
    assert_eq!(
        parse_link(nameless.as_bytes()).map(drop),
        Err(ParseError::MissingAttribute {
            message: "link",
            attribute: "IFLA_IFNAME"
        })
    );
    assert_eq!(
        parse_link(&[0; 15]).map(drop),
        Err(ParseError::Truncated {
            what: "ifinfomsg",
            needed: 16,
            have: 15
        })
    );
    let mut wrong = link_payload("eth0", 1, 2, 0);
    wrong.attr(4, &[1, 2]);
    assert!(matches!(
        parse_link(wrong.as_bytes()),
        Err(ParseError::AttributeSize { attribute: 4, .. })
    ));
}

#[test]
fn a_link_is_classified_by_type_flags_driver_and_the_wireless_list() {
    for (name, hardware_type, flags, kind, wireless, expected) in [
        ("lo", 772, IFF_LOOPBACK, None, &[][..], LinkClass::Loopback),
        ("lo", 1, IFF_LOOPBACK, None, &[], LinkClass::Loopback),
        ("eno1", 1, 0, None, &[7], LinkClass::Wired),
        ("br0", 1, 0, Some("bridge"), &[7], LinkClass::Virtual),
        ("tun0", 65534, 0, None, &[7], LinkClass::Virtual),
        ("wlan0", 1, 0, Some("wlan"), &[3], LinkClass::Wireless),
        ("wlan0", 1, 0, Some("wlan"), &[], LinkClass::Virtual),
    ] {
        let mut body = link_payload(name, hardware_type, 3, flags);
        if let Some(kind) = kind {
            body.nested(18, |info| {
                info.attr_cstr(1, kind);
            });
        }
        let link = parse_link(body.as_bytes()).unwrap();
        assert_eq!(link.class(wireless), expected, "{name} {wireless:?}");
    }
}

#[test]
fn an_address_decodes_its_local_address_flags_scope_and_label() {
    let mut v4 = Body::new();
    v4.header(&address_header(2, 24, 0, 0, 2))
        .attr(1, &[10, 0, 0, 99])
        .attr(2, &[10, 0, 0, 5])
        .attr_cstr(3, "eth0");
    let address = parse_address(v4.as_bytes()).unwrap().unwrap();
    assert_eq!(
        (
            address.index,
            address.family,
            address.prefix_len,
            address.scope
        ),
        (2, Family::V4, 24, Scope::Universe)
    );
    assert_eq!(
        address.address,
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
        "the local attribute wins over the peer"
    );
    assert_eq!(address.label.as_deref(), Some("eth0"));

    let mut v6 = Body::new();
    v6.header(&address_header(10, 64, 0x80, 253, 3))
        .attr(1, &Ipv6Addr::LOCALHOST.octets())
        .attr_u32(8, 0x40);
    let address = parse_address(v6.as_bytes()).unwrap().unwrap();
    assert_eq!((address.family, address.scope), (Family::V6, Scope::Link));
    assert_eq!(address.address, IpAddr::V6(Ipv6Addr::LOCALHOST));
    assert_eq!(address.flags, 0x40, "the wide flags win over the header");
    assert!(address.is_tentative());
    assert_eq!(address.label, None);

    let mut narrow = Body::new();
    narrow
        .header(&address_header(2, 8, 0x40, 254, 1))
        .attr(2, &[127, 0, 0, 1]);
    let address = parse_address(narrow.as_bytes()).unwrap().unwrap();
    assert_eq!(address.flags, 0x40, "without wide flags the header's apply");
    assert!(address.is_tentative());

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
            expected,
            "{raw}"
        );
    }
}

#[test]
fn a_route_decodes_its_attributes_and_is_default_only_when_unicast_to_everything_in_the_main_table()
{
    let mut body = Body::new();
    body.header(&route_header(2, 0, 254, 16, 0, 1))
        .attr(5, &[192, 168, 1, 1])
        .attr_u32(4, 3)
        .attr_u32(6, 600)
        .attr(7, &[192, 168, 1, 20]);
    let route = parse_route(body.as_bytes()).unwrap().unwrap();
    assert_eq!(route.family, Family::V4);
    assert_eq!((route.destination, route.dst_len), (None, 0));
    assert_eq!(
        route.gateway,
        Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)))
    );
    assert_eq!((route.index, route.metric), (Some(3), Some(600)));
    assert_eq!(
        (route.table, route.protocol, route.route_type),
        (TABLE_MAIN, PROTOCOL_DHCP, 1)
    );
    assert_eq!(
        route.preferred_source,
        Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)))
    );

    let mut wide_table = Body::new();
    wide_table
        .header(&route_header(2, 24, 252, 4, 253, 1))
        .attr(1, &[10, 1, 0, 0])
        .attr_u32(15, 1000);
    let route = parse_route(wide_table.as_bytes()).unwrap().unwrap();
    assert_eq!(
        route.table, 1000,
        "the table attribute wins over the header"
    );
    assert_eq!(
        route.destination,
        Some(IpAddr::V4(Ipv4Addr::new(10, 1, 0, 0)))
    );
    assert_eq!(route.scope, Scope::Link);

    for (name, dst_len, table, kind, expected) in [
        ("default", 0, 254, 1, true),
        ("a prefix", 24, 254, 1, false),
        ("another table", 0, 100, 1, false),
        ("not unicast", 0, 254, 7, false),
    ] {
        let mut body = Body::new();
        body.header(&route_header(2, dst_len, table, 4, 0, kind));
        assert_eq!(
            parse_route(body.as_bytes()).unwrap().unwrap().is_default(),
            expected,
            "{name}"
        );
    }
    assert!(!route.is_default(), "a table of its own");
}

#[test]
fn an_address_or_route_of_another_family_is_skipped_and_a_damaged_one_refused() {
    let mut other = Body::new();
    other.header(&address_header(16, 0, 0, 0, 1));
    assert_eq!(parse_address(other.as_bytes()), Ok(None));
    assert_eq!(parse_route(bare_route(29).as_bytes()), Ok(None));

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
fn a_dump_lists_its_objects_skipping_other_message_types_other_families_and_the_done_message() {
    let mut links = frame(RTM_GETLINK, 0, 1, &[0; 16]);
    links.extend(dump(
        RTM_NEWLINK,
        &[
            link_payload("lo", 772, 1, IFF_LOOPBACK),
            link_payload("eth0", 1, 2, IFF_UP),
        ],
    ));
    assert_eq!(
        parse_links(&links)
            .unwrap()
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        ["lo", "eth0"]
    );

    let mut other = Body::new();
    other.header(&address_header(16, 0, 0, 0, 2));
    let addresses = parse_addresses(&dump(RTM_NEWADDR, &[v4_address(), other])).unwrap();
    assert_eq!(addresses.len(), 1);

    let routes = parse_routes(&dump(RTM_NEWROUTE, &[bare_route(2), bare_route(16)])).unwrap();
    assert_eq!(routes.len(), 1);
}

#[test]
fn a_damaged_dump_says_which_request_it_answered() {
    let mut bad_length = dump(RTM_NEWLINK, &[link_payload("eth0", 1, 2, 0)]);
    bad_length[0..4].copy_from_slice(&4000u32.to_ne_bytes());
    let mut nameless = Body::new();
    nameless.header(&link_header(1, 2, 0));
    let nameless = dump(RTM_NEWLINK, &[nameless]);
    for (name, raw) in [("a bad length", bad_length), ("a nameless link", nameless)] {
        let error = parse_links(&raw).unwrap_err();
        assert!(
            matches!(
                &error,
                Error::Parse {
                    origin: "RTM_GETLINK",
                    ..
                }
            ),
            "{name}: {error:?}"
        );
    }
}

#[test]
fn a_message_of_the_multicast_groups_becomes_its_event_and_any_other_none() {
    let decode = |ty: u16, payload: &Body| {
        let raw = frame(ty, 0, 0, payload.as_bytes());
        parse_event(&messages(&raw).next().unwrap().unwrap())
    };
    let link = link_payload("eth0", 1, 2, 0);
    let address = v4_address();
    let route = bare_route(2);
    for (name, event, matched) in [
        (
            "new link",
            decode(RTM_NEWLINK, &link),
            &(|e: &RouteEvent| matches!(e, RouteEvent::LinkNew(l) if l.name == "eth0"))
                as &dyn Fn(&RouteEvent) -> bool,
        ),
        (
            "deleted link",
            decode(RTM_DELLINK, &link),
            &|e: &RouteEvent| matches!(e, RouteEvent::LinkDeleted(l) if l.index == 2),
        ),
        (
            "new address",
            decode(RTM_NEWADDR, &address),
            &|e: &RouteEvent| matches!(e, RouteEvent::AddressNew(_)),
        ),
        (
            "deleted address",
            decode(RTM_DELADDR, &address),
            &|e: &RouteEvent| matches!(e, RouteEvent::AddressDeleted(_)),
        ),
        (
            "new route",
            decode(RTM_NEWROUTE, &route),
            &|e: &RouteEvent| matches!(e, RouteEvent::RouteNew(_)),
        ),
        (
            "deleted route",
            decode(RTM_DELROUTE, &route),
            &|e: &RouteEvent| matches!(e, RouteEvent::RouteDeleted(_)),
        ),
    ] {
        let event = event.unwrap().unwrap();
        assert!(matched(&event), "{name}: {event:?}");
    }

    let mut other_family = Body::new();
    other_family.header(&address_header(16, 0, 0, 0, 2));
    let mut not_network = Body::new();
    not_network.header(&[0; 4]);
    for (name, event) in [
        ("not about the network", decode(2, &not_network)),
        ("another family", decode(RTM_NEWADDR, &other_family)),
    ] {
        assert_eq!(event, Ok(None), "{name}");
    }
    let mut broken = Body::new();
    broken.header(&[0; 16]);
    assert!(matches!(
        decode(RTM_NEWLINK, &broken),
        Err(ParseError::MissingAttribute { .. })
    ));
}
