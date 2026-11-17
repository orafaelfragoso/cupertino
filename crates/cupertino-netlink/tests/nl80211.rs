//! Wireless interfaces, stations, scan results and events decoded from nl80211 messages.

use cupertino_netlink::message::{frame, Body, NLMSG_DONE};
use cupertino_netlink::nl80211::{
    connection, is_unsupported, parse_event, parse_family, parse_family_reply, parse_interface,
    parse_interfaces, parse_scan_dump, parse_scan_result, parse_station, parse_stations,
    parse_wiphy, parse_wiphys, Event, InterfaceType, Station, WirelessInterface,
};
use cupertino_netlink::{Error, MacAddr, ParseError, Security, Ssid};
use rustix::io::Errno;

const FAMILY: u16 = 0x1C;
const BSSID: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

/// A generic netlink payload: the header of command `command`, then the attributes `build` writes.
fn genl(command: u8, build: impl FnOnce(&mut Body)) -> Body {
    let mut body = Body::new();
    body.header(&[command, 1, 0, 0]);
    build(&mut body);
    body
}

fn element(id: u8, data: &[u8]) -> Vec<u8> {
    [&[id, data.len() as u8][..], data].concat()
}

fn interface_payload() -> Body {
    genl(7, |b| {
        b.attr_u32(3, 4)
            .attr_cstr(4, "wlan0")
            .attr_u32(1, 0)
            .attr_u32(5, 2)
            .attr(6, &BSSID)
            .attr(52, b"home")
            .attr_u32(38, 5180);
    })
}

#[test]
fn an_interface_decodes_with_its_connection_details() {
    let interface = parse_interface(interface_payload().as_bytes())
        .unwrap()
        .unwrap();
    assert_eq!(interface.ifindex, 4);
    assert_eq!(interface.name, "wlan0");
    assert_eq!(interface.wiphy, 0);
    assert_eq!(interface.interface_type, InterfaceType::Station);
    assert_eq!(interface.mac, Some(MacAddr(BSSID)));
    assert_eq!(interface.ssid, Some(Ssid(b"home".to_vec())));
    assert_eq!(interface.frequency_mhz, Some(5180));
}

#[test]
fn interface_types_keep_values_the_crate_does_not_name() {
    for (raw, expected) in [
        (1u32, InterfaceType::AdHoc),
        (2, InterfaceType::Station),
        (3, InterfaceType::AccessPoint),
        (6, InterfaceType::Monitor),
        (8, InterfaceType::P2pClient),
        (9, InterfaceType::P2pGroupOwner),
        (4, InterfaceType::Other(4)),
    ] {
        let payload = genl(7, |b| {
            b.attr_u32(3, 1)
                .attr_cstr(4, "w")
                .attr_u32(1, 0)
                .attr_u32(5, raw);
        });
        assert_eq!(
            parse_interface(payload.as_bytes())
                .unwrap()
                .unwrap()
                .interface_type,
            expected
        );
    }
}

#[test]
fn an_interface_without_an_index_is_not_a_network_device() {
    let p2p = genl(7, |b| {
        b.attr_cstr(4, "p2p-dev").attr_u32(1, 0).attr_u32(5, 10);
    });
    assert_eq!(parse_interface(p2p.as_bytes()), Ok(None));
}

#[test]
fn an_interface_missing_a_required_attribute_names_it() {
    let without = |skip: u16| {
        genl(7, |b| {
            for (ty, name) in [(3u16, ""), (4, "wlan0"), (1, ""), (5, "")] {
                if ty == skip {
                    continue;
                }
                if name.is_empty() {
                    b.attr_u32(ty, 1);
                } else {
                    b.attr_cstr(ty, name);
                }
            }
        })
    };
    for (skip, attribute) in [
        (4u16, "NL80211_ATTR_IFNAME"),
        (1, "NL80211_ATTR_WIPHY"),
        (5, "NL80211_ATTR_IFTYPE"),
    ] {
        assert_eq!(
            parse_interface(without(skip).as_bytes()),
            Err(ParseError::MissingAttribute {
                message: "interface",
                attribute
            })
        );
    }
}

#[test]
fn a_payload_shorter_than_the_generic_header_is_truncated() {
    assert_eq!(
        parse_interface(&[1, 2, 3]),
        Err(ParseError::Truncated {
            what: "genlmsghdr",
            needed: 4,
            have: 3
        })
    );
    assert!(parse_wiphy(&[]).is_err());
    assert!(parse_station(&[]).is_err());
    assert!(parse_scan_result(&[]).is_err());
    assert!(parse_event(&[]).is_err());
    assert!(parse_family(&[]).is_err());
}

#[test]
fn a_wiphy_part_with_a_name_decodes_and_one_without_is_skipped() {
    let named = genl(3, |b| {
        b.attr_u32(1, 2).attr_cstr(2, "phy2");
    });
    let wiphy = parse_wiphy(named.as_bytes()).unwrap().unwrap();
    assert_eq!((wiphy.index, wiphy.name.as_str()), (2, "phy2"));
    let part = genl(3, |b| {
        b.attr_u32(1, 2);
    });
    assert_eq!(parse_wiphy(part.as_bytes()), Ok(None));
    let nameless = genl(3, |b| {
        b.attr_cstr(2, "phy2");
    });
    assert_eq!(
        parse_wiphy(nameless.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "wiphy",
            attribute: "NL80211_ATTR_WIPHY"
        })
    );
}

fn station_payload(signal: Option<i8>) -> Body {
    genl(19, |b| {
        b.attr(6, &BSSID).nested(21, |info| {
            info.attr_u32(1, 1200);
            if let Some(signal) = signal {
                info.attr(7, &[signal as u8])
                    .attr(13, &[(signal - 3) as u8]);
            }
        });
    })
}

#[test]
fn a_station_carries_signal_and_idle_time() {
    let station = parse_station(station_payload(Some(-61)).as_bytes()).unwrap();
    assert_eq!(station.bssid, MacAddr(BSSID));
    assert_eq!(station.signal_dbm, Some(-61));
    assert_eq!(station.signal_average_dbm, Some(-64));
    assert_eq!(station.inactive_ms, Some(1200));
    let quiet = parse_station(station_payload(None).as_bytes()).unwrap();
    assert_eq!(quiet.signal_dbm, None);
}

#[test]
fn a_station_without_its_address_or_info_is_refused() {
    let no_info = genl(19, |b| {
        b.attr(6, &BSSID);
    });
    assert_eq!(
        parse_station(no_info.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "station",
            attribute: "NL80211_ATTR_STA_INFO"
        })
    );
    let no_mac = genl(19, |b| {
        b.nested(21, |info| {
            info.attr_u32(1, 5);
        });
    });
    assert_eq!(
        parse_station(no_mac.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "station",
            attribute: "NL80211_ATTR_MAC"
        })
    );
}

fn interface(ssid: Option<&str>) -> WirelessInterface {
    WirelessInterface {
        ifindex: 4,
        name: "wlan0".to_owned(),
        wiphy: 0,
        interface_type: InterfaceType::Station,
        mac: None,
        ssid: ssid.map(|s| Ssid(s.as_bytes().to_vec())),
        frequency_mhz: Some(2412),
    }
}

#[test]
fn the_connection_is_the_interface_joined_with_its_first_station() {
    let station = Station {
        bssid: MacAddr(BSSID),
        signal_dbm: Some(-50),
        signal_average_dbm: None,
        inactive_ms: None,
    };
    let joined = connection(&interface(Some("home")), &[station]).unwrap();
    assert_eq!(joined.ifindex, 4);
    assert_eq!(joined.ssid, Some(Ssid(b"home".to_vec())));
    assert_eq!(joined.bssid, MacAddr(BSSID));
    assert_eq!(joined.frequency_mhz, Some(2412));
    assert_eq!(joined.signal_dbm, Some(-50));
    assert_eq!(connection(&interface(Some("home")), &[]), None);
}

fn scan_payload(build: impl FnOnce(&mut Body)) -> Body {
    genl(34, |b| {
        b.attr_u32(3, 4).nested(47, build);
    })
}

fn rsn_psk() -> Vec<u8> {
    element(
        48,
        &[
            1, 0, 0x00, 0x0F, 0xAC, 4, 1, 0, 0x00, 0x0F, 0xAC, 4, 1, 0, 0x00, 0x0F, 0xAC, 2,
        ],
    )
}

#[test]
fn a_scan_result_decodes_signal_status_and_security() {
    let elements = [element(0, b"home"), rsn_psk()].concat();
    let payload = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2437)
            .attr(5, &0x0011u16.to_ne_bytes())
            .attr(6, &elements)
            .attr(7, &(-6700i32).to_ne_bytes())
            .attr_u32(9, 1)
            .attr_u32(10, 340);
    });
    let result = parse_scan_result(payload.as_bytes()).unwrap();
    assert_eq!(result.ifindex, 4);
    assert_eq!(result.bssid, MacAddr(BSSID));
    assert_eq!(result.ssid.to_text(), "home");
    assert_eq!(result.frequency_mhz, 2437);
    assert_eq!(result.signal_dbm, Some(-67));
    assert_eq!(result.security, Security::Wpa2);
    assert!(result.associated);
    assert_eq!(result.seen_ms_ago, Some(340));
}

#[test]
fn a_scan_result_without_status_or_signal_is_not_associated_and_has_no_signal() {
    let payload = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2412)
            .attr(6, &element(0, b"x"));
    });
    let result = parse_scan_result(payload.as_bytes()).unwrap();
    assert!(!result.associated);
    assert_eq!(result.signal_dbm, None);
    assert_eq!(result.seen_ms_ago, None);
    assert_eq!(result.security, Security::Open);
}

#[test]
fn the_privacy_bit_of_the_capability_makes_an_unlabelled_network_wep() {
    let payload = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2412)
            .attr(5, &0x0010u16.to_ne_bytes())
            .attr(6, &element(0, b"old"));
    });
    assert_eq!(
        parse_scan_result(payload.as_bytes()).unwrap().security,
        Security::Wep
    );
}

#[test]
fn an_associated_status_is_only_the_value_one() {
    let payload = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2412)
            .attr(6, &element(0, b"x"))
            .attr_u32(9, 0);
    });
    assert!(!parse_scan_result(payload.as_bytes()).unwrap().associated);
}

#[test]
fn the_beacon_elements_stand_in_when_there_is_no_probe_response() {
    let beacon = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2412)
            .attr(11, &element(0, b"beacon"));
    });
    assert_eq!(
        parse_scan_result(beacon.as_bytes()).unwrap().ssid.to_text(),
        "beacon"
    );
    let both = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2412)
            .attr(11, &element(0, b"beacon"))
            .attr(6, &element(0, b"probe"));
    });
    assert_eq!(
        parse_scan_result(both.as_bytes()).unwrap().ssid.to_text(),
        "probe"
    );
}

#[test]
fn a_scan_result_missing_a_part_names_it() {
    let no_bss = genl(34, |b| {
        b.attr_u32(3, 4);
    });
    assert_eq!(
        parse_scan_result(no_bss.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "scan result",
            attribute: "NL80211_ATTR_BSS"
        })
    );
    let no_elements = scan_payload(|bss| {
        bss.attr(1, &BSSID).attr_u32(2, 2412);
    });
    assert_eq!(
        parse_scan_result(no_elements.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "scan result",
            attribute: "NL80211_BSS_INFORMATION_ELEMENTS"
        })
    );
    let no_bssid = scan_payload(|bss| {
        bss.attr_u32(2, 2412).attr(6, &element(0, b"x"));
    });
    assert!(parse_scan_result(no_bssid.as_bytes()).is_err());
}

#[test]
fn bad_information_elements_fail_the_scan_result() {
    let payload = scan_payload(|bss| {
        bss.attr(1, &BSSID).attr_u32(2, 2412).attr(6, &[0, 9, 1]);
    });
    assert!(matches!(
        parse_scan_result(payload.as_bytes()),
        Err(ParseError::InformationElement { .. })
    ));
}

#[test]
fn events_carry_the_interface_and_what_happened() {
    let started = genl(33, |b| {
        b.attr_u32(3, 4);
    });
    assert_eq!(
        parse_event(started.as_bytes()),
        Ok(Some(Event::ScanStarted { ifindex: 4 }))
    );
    let ready = genl(34, |b| {
        b.attr_u32(3, 4);
    });
    assert_eq!(
        parse_event(ready.as_bytes()),
        Ok(Some(Event::ScanResultsReady { ifindex: 4 }))
    );
    let aborted = genl(35, |b| {
        b.attr_u32(3, 4);
    });
    assert_eq!(
        parse_event(aborted.as_bytes()),
        Ok(Some(Event::ScanAborted { ifindex: 4 }))
    );
}

#[test]
fn a_connect_event_carries_the_status_and_a_timeout_has_none() {
    let connected = genl(46, |b| {
        b.attr_u32(3, 4)
            .attr(6, &BSSID)
            .attr(72, &0u16.to_ne_bytes());
    });
    assert_eq!(
        parse_event(connected.as_bytes()),
        Ok(Some(Event::Connected {
            ifindex: 4,
            bssid: Some(MacAddr(BSSID)),
            status_code: Some(0)
        }))
    );
    let refused = genl(46, |b| {
        b.attr_u32(3, 4).attr(72, &17u16.to_ne_bytes());
    });
    assert_eq!(
        parse_event(refused.as_bytes()),
        Ok(Some(Event::Connected {
            ifindex: 4,
            bssid: None,
            status_code: Some(17)
        }))
    );
    let timed_out = genl(46, |b| {
        b.attr_u32(3, 4).attr(65, &[]);
    });
    assert_eq!(
        parse_event(timed_out.as_bytes()),
        Ok(Some(Event::Connected {
            ifindex: 4,
            bssid: None,
            status_code: None
        }))
    );
    let silent = genl(46, |b| {
        b.attr_u32(3, 4);
    });
    assert_eq!(
        parse_event(silent.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "connect event",
            attribute: "NL80211_ATTR_STATUS_CODE"
        })
    );
}

#[test]
fn a_disconnect_event_defaults_its_reason_to_unspecified() {
    let by_ap = genl(48, |b| {
        b.attr_u32(3, 4).attr(54, &3u16.to_ne_bytes()).attr(71, &[]);
    });
    assert_eq!(
        parse_event(by_ap.as_bytes()),
        Ok(Some(Event::Disconnected {
            ifindex: 4,
            reason_code: 3,
            by_access_point: true
        }))
    );
    let local = genl(48, |b| {
        b.attr_u32(3, 4);
    });
    assert_eq!(
        parse_event(local.as_bytes()),
        Ok(Some(Event::Disconnected {
            ifindex: 4,
            reason_code: 0,
            by_access_point: false
        }))
    );
}

#[test]
fn a_command_the_crate_does_not_report_is_no_event_and_an_event_needs_its_interface() {
    let other = genl(1, |b| {
        b.attr_u32(3, 4);
    });
    assert_eq!(parse_event(other.as_bytes()), Ok(None));
    let no_interface = genl(34, |_| {});
    assert_eq!(
        parse_event(no_interface.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "event",
            attribute: "NL80211_ATTR_IFINDEX"
        })
    );
}

fn family_payload() -> Body {
    genl(1, |b| {
        b.attr_cstr(2, "nl80211")
            .attr(1, &FAMILY.to_ne_bytes())
            .nested(7, |groups| {
                groups.nested(1, |group| {
                    group.attr_cstr(1, "scan").attr_u32(2, 5);
                });
                groups.nested(2, |group| {
                    group.attr_cstr(1, "mlme").attr_u32(2, 7);
                });
            });
    })
}

#[test]
fn a_family_reply_gives_the_id_and_the_multicast_groups() {
    let family = parse_family(family_payload().as_bytes()).unwrap();
    assert_eq!(family.id, FAMILY);
    assert_eq!(
        family.groups,
        [("scan".to_owned(), 5), ("mlme".to_owned(), 7)]
    );
}

#[test]
fn a_family_without_groups_has_none_and_one_without_an_id_is_refused() {
    let bare = genl(1, |b| {
        b.attr(1, &FAMILY.to_ne_bytes());
    });
    assert!(parse_family(bare.as_bytes()).unwrap().groups.is_empty());
    let no_id = genl(1, |b| {
        b.attr_cstr(2, "nl80211");
    });
    assert_eq!(
        parse_family(no_id.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "family",
            attribute: "CTRL_ATTR_FAMILY_ID"
        })
    );
}

#[test]
fn a_group_without_a_name_or_id_is_refused() {
    let no_id = genl(1, |b| {
        b.attr(1, &FAMILY.to_ne_bytes()).nested(7, |groups| {
            groups.nested(1, |group| {
                group.attr_cstr(1, "scan");
            });
        });
    });
    assert_eq!(
        parse_family(no_id.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "multicast group",
            attribute: "CTRL_ATTR_MCAST_GRP_ID"
        })
    );
    let no_name = genl(1, |b| {
        b.attr(1, &FAMILY.to_ne_bytes()).nested(7, |groups| {
            groups.nested(1, |group| {
                group.attr_u32(2, 5);
            });
        });
    });
    assert_eq!(
        parse_family(no_name.as_bytes()),
        Err(ParseError::MissingAttribute {
            message: "multicast group",
            attribute: "CTRL_ATTR_MCAST_GRP_NAME"
        })
    );
}

#[test]
fn the_family_reply_is_found_among_the_messages_and_its_absence_is_an_error() {
    let mut raw = frame(2, 0, 1, &[0; 4]);
    raw.extend(frame(0x10, 0, 1, family_payload().as_bytes()));
    assert_eq!(parse_family_reply(&raw).unwrap().id, FAMILY);

    let none = frame(NLMSG_DONE, 0, 1, &[0; 4]);
    assert!(matches!(
        parse_family_reply(&none),
        Err(Error::Parse {
            origin: "CTRL_CMD_GETFAMILY",
            source: ParseError::MissingAttribute { .. }
        })
    ));
    assert!(matches!(
        parse_family_reply(&[1, 2, 3]),
        Err(Error::Parse { .. })
    ));
}

fn dump(payloads: &[Body]) -> Vec<u8> {
    let mut raw = Vec::new();
    for (sequence, payload) in payloads.iter().enumerate() {
        raw.extend(frame(FAMILY, 2, sequence as u32 + 1, payload.as_bytes()));
    }
    raw.extend(frame(NLMSG_DONE, 2, 99, &[0; 4]));
    raw
}

#[test]
fn an_interface_dump_keeps_network_devices_of_the_family_only() {
    let p2p = genl(7, |b| {
        b.attr_cstr(4, "p2p-dev").attr_u32(1, 0).attr_u32(5, 10);
    });
    let mut raw = frame(FAMILY + 1, 2, 1, interface_payload().as_bytes());
    raw.extend(dump(&[interface_payload(), p2p]));
    let interfaces = parse_interfaces(&raw, FAMILY).unwrap();
    assert_eq!(interfaces.len(), 1);
    assert_eq!(interfaces[0].name, "wlan0");
}

#[test]
fn a_wiphy_dump_lists_each_phy_once_in_order() {
    let phy = |index: u32, name: Option<&str>| {
        genl(3, |b| {
            b.attr_u32(1, index);
            if let Some(name) = name {
                b.attr_cstr(2, name);
            }
        })
    };
    let raw = dump(&[
        phy(1, Some("phy1")),
        phy(1, None),
        phy(0, Some("phy0")),
        phy(1, Some("phy1")),
    ]);
    let wiphys = parse_wiphys(&raw, FAMILY).unwrap();
    assert_eq!(
        wiphys
            .iter()
            .map(|w| (w.index, w.name.as_str()))
            .collect::<Vec<_>>(),
        [(1, "phy1"), (0, "phy0")]
    );
}

#[test]
fn a_station_dump_lists_every_station_and_fails_with_the_request_name() {
    let raw = dump(&[station_payload(Some(-40)), station_payload(Some(-80))]);
    assert_eq!(parse_stations(&raw, FAMILY).unwrap().len(), 2);
    let broken = dump(&[genl(19, |_| {})]);
    assert!(matches!(
        parse_stations(&broken, FAMILY),
        Err(Error::Parse {
            origin: "NL80211_CMD_GET_STATION",
            ..
        })
    ));
}

#[test]
fn a_scan_dump_keeps_the_good_results_and_lists_the_bad_ones() {
    let good = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2412)
            .attr(6, &element(0, b"ok"));
    });
    let bad = scan_payload(|bss| {
        bss.attr_u32(2, 2412);
    });
    let dump = parse_scan_dump(&dump(&[good.clone(), bad, good]), FAMILY).unwrap();
    assert_eq!(dump.results.len(), 2);
    assert_eq!(dump.errors.len(), 1);
}

#[test]
fn a_kernel_without_the_family_is_reported_as_unsupported() {
    assert!(is_unsupported(&Error::Kernel {
        request: "CTRL_CMD_GETFAMILY",
        errno: Errno::NOENT
    }));
    assert!(!is_unsupported(&Error::Kernel {
        request: "CTRL_CMD_GETFAMILY",
        errno: Errno::PERM
    }));
    assert!(!is_unsupported(&Error::UnexpectedSequence {
        expected: 1,
        actual: 2
    }));
}
