//! Wireless interfaces, stations, scan results and events decoded from nl80211 messages.

use cupertino_netd::netlink::message::{frame, Body, NLMSG_DONE};
use cupertino_netd::netlink::nl80211::{
    connection, is_unsupported, parse_event, parse_family, parse_family_reply, parse_interface,
    parse_interfaces, parse_scan_dump, parse_scan_result, parse_station, parse_stations,
    parse_wiphy, parse_wiphys, Event, InterfaceType, Station, WirelessInterface,
};
use cupertino_netd::netlink::{Error, MacAddr, ParseError, Security, Ssid};
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

fn missing(message: &'static str, attribute: &'static str) -> ParseError {
    ParseError::MissingAttribute { message, attribute }
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

fn p2p_device() -> Body {
    genl(7, |b| {
        b.attr_cstr(4, "p2p-dev").attr_u32(1, 0).attr_u32(5, 10);
    })
}

#[test]
fn an_interface_decodes_with_its_connection_details_and_one_without_an_index_is_no_device() {
    assert_eq!(
        parse_interface(interface_payload().as_bytes()),
        Ok(Some(WirelessInterface {
            ifindex: 4,
            name: "wlan0".to_owned(),
            wiphy: 0,
            interface_type: InterfaceType::Station,
            mac: Some(MacAddr(BSSID)),
            ssid: Some(Ssid(b"home".to_vec())),
            frequency_mhz: Some(5180),
        }))
    );
    assert_eq!(parse_interface(p2p_device().as_bytes()), Ok(None));
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
        let interface = parse_interface(payload.as_bytes()).unwrap().unwrap();
        assert_eq!(interface.interface_type, expected, "{raw}");
    }
}

#[test]
fn a_message_shorter_than_the_generic_header_or_an_interface_without_a_required_attribute_is_refused(
) {
    assert_eq!(
        parse_interface(&[1, 2, 3]),
        Err(ParseError::Truncated {
            what: "genlmsghdr",
            needed: 4,
            have: 3
        })
    );
    for (name, failed) in [
        ("wiphy", parse_wiphy(&[]).is_err()),
        ("station", parse_station(&[]).is_err()),
        ("scan result", parse_scan_result(&[]).is_err()),
        ("event", parse_event(&[]).is_err()),
        ("family", parse_family(&[]).is_err()),
    ] {
        assert!(failed, "{name}");
    }

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
            Err(missing("interface", attribute)),
            "{attribute}"
        );
    }
}

fn phy(index: Option<u32>, name: Option<&str>) -> Body {
    genl(3, |b| {
        if let Some(index) = index {
            b.attr_u32(1, index);
        }
        if let Some(name) = name {
            b.attr_cstr(2, name);
        }
    })
}

#[test]
fn a_wiphy_part_with_a_name_decodes_one_without_is_skipped_and_one_without_an_index_is_refused() {
    let wiphy = parse_wiphy(phy(Some(2), Some("phy2")).as_bytes())
        .unwrap()
        .unwrap();
    assert_eq!((wiphy.index, wiphy.name.as_str()), (2, "phy2"));
    assert_eq!(parse_wiphy(phy(Some(2), None).as_bytes()), Ok(None));
    assert_eq!(
        parse_wiphy(phy(None, Some("phy2")).as_bytes()).map(drop),
        Err(missing("wiphy", "NL80211_ATTR_WIPHY"))
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
fn a_station_carries_signal_and_idle_time_and_needs_its_address_and_info() {
    let station = |signal_dbm, signal_average_dbm| Station {
        bssid: MacAddr(BSSID),
        signal_dbm,
        signal_average_dbm,
        inactive_ms: Some(1200),
    };
    assert_eq!(
        parse_station(station_payload(Some(-61)).as_bytes()),
        Ok(station(Some(-61), Some(-64)))
    );
    assert_eq!(
        parse_station(station_payload(None).as_bytes()),
        Ok(station(None, None))
    );

    let no_info = genl(19, |b| {
        b.attr(6, &BSSID);
    });
    let no_mac = genl(19, |b| {
        b.nested(21, |info| {
            info.attr_u32(1, 5);
        });
    });
    for (payload, attribute) in [
        (no_info, "NL80211_ATTR_STA_INFO"),
        (no_mac, "NL80211_ATTR_MAC"),
    ] {
        assert_eq!(
            parse_station(payload.as_bytes()),
            Err(missing("station", attribute)),
            "{attribute}"
        );
    }
}

#[test]
fn the_connection_is_the_interface_joined_with_its_first_station() {
    let interface = WirelessInterface {
        ifindex: 4,
        name: "wlan0".to_owned(),
        wiphy: 0,
        interface_type: InterfaceType::Station,
        mac: None,
        ssid: Some(Ssid(b"home".to_vec())),
        frequency_mhz: Some(2412),
    };
    let station = Station {
        bssid: MacAddr(BSSID),
        signal_dbm: Some(-50),
        signal_average_dbm: None,
        inactive_ms: None,
    };
    let joined = connection(&interface, &[station]).unwrap();
    assert_eq!(joined.ifindex, 4);
    assert_eq!(joined.ssid, Some(Ssid(b"home".to_vec())));
    assert_eq!(joined.bssid, MacAddr(BSSID));
    assert_eq!(joined.frequency_mhz, Some(2412));
    assert_eq!(joined.signal_dbm, Some(-50));
    assert_eq!(connection(&interface, &[]), None);
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
fn a_scan_result_decodes_its_network_signal_status_and_security() {
    let elements = [element(0, b"home"), rsn_psk()].concat();
    let full = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2437)
            .attr(5, &0x0011u16.to_ne_bytes())
            .attr(6, &elements)
            .attr(7, &(-6700i32).to_ne_bytes())
            .attr_u32(9, 1)
            .attr_u32(10, 340);
    });
    let result = parse_scan_result(full.as_bytes()).unwrap();
    assert_eq!((result.ifindex, result.bssid), (4, MacAddr(BSSID)));
    assert_eq!(result.ssid.to_text(), "home");
    assert_eq!(result.frequency_mhz, 2437);
    assert_eq!(result.signal_dbm, Some(-67));
    assert_eq!(result.security, Security::Wpa2);
    assert!(result.associated);
    assert_eq!(result.seen_ms_ago, Some(340));

    let minimal = |extra: &dyn Fn(&mut Body)| {
        scan_payload(|bss| {
            bss.attr(1, &BSSID).attr_u32(2, 2412);
            extra(bss);
        })
    };
    for (name, payload, ssid, security, associated) in [
        (
            "no status or signal",
            minimal(&|bss| {
                bss.attr(6, &element(0, b"x"));
            }),
            "x",
            Security::Open,
            false,
        ),
        (
            "the privacy bit makes an unlabelled network WEP",
            minimal(&|bss| {
                bss.attr(5, &0x0010u16.to_ne_bytes())
                    .attr(6, &element(0, b"old"));
            }),
            "old",
            Security::Wep,
            false,
        ),
        (
            "a status other than one",
            minimal(&|bss| {
                bss.attr(6, &element(0, b"x")).attr_u32(9, 0);
            }),
            "x",
            Security::Open,
            false,
        ),
        (
            "the beacon stands in for a probe response",
            minimal(&|bss| {
                bss.attr(11, &element(0, b"beacon"));
            }),
            "beacon",
            Security::Open,
            false,
        ),
        (
            "the probe response wins over the beacon",
            minimal(&|bss| {
                bss.attr(11, &element(0, b"beacon"))
                    .attr(6, &element(0, b"probe"));
            }),
            "probe",
            Security::Open,
            false,
        ),
    ] {
        let result = parse_scan_result(payload.as_bytes()).unwrap();
        assert_eq!(result.ssid.to_text(), ssid, "{name}");
        assert_eq!(result.security, security, "{name}");
        assert_eq!(result.associated, associated, "{name}");
        assert_eq!(
            (result.signal_dbm, result.seen_ms_ago),
            (None, None),
            "{name}"
        );
    }
}

#[test]
fn a_scan_result_missing_a_part_or_with_bad_elements_is_refused() {
    let no_bss = genl(34, |b| {
        b.attr_u32(3, 4);
    });
    let no_elements = scan_payload(|bss| {
        bss.attr(1, &BSSID).attr_u32(2, 2412);
    });
    let no_bssid = scan_payload(|bss| {
        bss.attr_u32(2, 2412).attr(6, &element(0, b"x"));
    });
    let bad_elements = scan_payload(|bss| {
        bss.attr(1, &BSSID).attr_u32(2, 2412).attr(6, &[0, 9, 1]);
    });
    for (name, payload, check) in [
        (
            "no BSS",
            no_bss,
            &(|e: &ParseError| *e == missing("scan result", "NL80211_ATTR_BSS"))
                as &dyn Fn(&ParseError) -> bool,
        ),
        ("no elements", no_elements, &|e: &ParseError| {
            *e == missing("scan result", "NL80211_BSS_INFORMATION_ELEMENTS")
        }),
        ("no BSSID", no_bssid, &|_: &ParseError| true),
        ("bad elements", bad_elements, &|e: &ParseError| {
            matches!(e, ParseError::InformationElement { .. })
        }),
    ] {
        let error = parse_scan_result(payload.as_bytes()).map(drop).unwrap_err();
        assert!(check(&error), "{name}: {error:?}");
    }
}

#[test]
fn an_event_carries_the_interface_and_what_happened() {
    let on = |command, build: &dyn Fn(&mut Body)| {
        genl(command, |b| {
            b.attr_u32(3, 4);
            build(b);
        })
    };
    let nothing = |_: &mut Body| {};
    for (name, payload, expected) in [
        (
            "scan started",
            on(33, &nothing),
            Some(Event::ScanStarted { ifindex: 4 }),
        ),
        (
            "scan results ready",
            on(34, &nothing),
            Some(Event::ScanResultsReady { ifindex: 4 }),
        ),
        (
            "scan aborted",
            on(35, &nothing),
            Some(Event::ScanAborted { ifindex: 4 }),
        ),
        (
            "connected",
            on(46, &|b| {
                b.attr(6, &BSSID).attr(72, &0u16.to_ne_bytes());
            }),
            Some(Event::Connected {
                ifindex: 4,
                bssid: Some(MacAddr(BSSID)),
                status_code: Some(0),
            }),
        ),
        (
            "connection refused",
            on(46, &|b| {
                b.attr(72, &17u16.to_ne_bytes());
            }),
            Some(Event::Connected {
                ifindex: 4,
                bssid: None,
                status_code: Some(17),
            }),
        ),
        (
            "connection timed out",
            on(46, &|b| {
                b.attr(65, &[]);
            }),
            Some(Event::Connected {
                ifindex: 4,
                bssid: None,
                status_code: None,
            }),
        ),
        (
            "disconnected by the access point",
            on(48, &|b| {
                b.attr(54, &3u16.to_ne_bytes()).attr(71, &[]);
            }),
            Some(Event::Disconnected {
                ifindex: 4,
                reason_code: 3,
                by_access_point: true,
            }),
        ),
        (
            "disconnected locally, reason unspecified",
            on(48, &nothing),
            Some(Event::Disconnected {
                ifindex: 4,
                reason_code: 0,
                by_access_point: false,
            }),
        ),
        ("a command not reported", on(1, &nothing), None),
    ] {
        assert_eq!(parse_event(payload.as_bytes()), Ok(expected), "{name}");
    }

    let silent = genl(46, |b| {
        b.attr_u32(3, 4);
    });
    assert_eq!(
        parse_event(silent.as_bytes()),
        Err(missing("connect event", "NL80211_ATTR_STATUS_CODE"))
    );
    assert_eq!(
        parse_event(genl(34, |_| {}).as_bytes()),
        Err(missing("event", "NL80211_ATTR_IFINDEX"))
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
fn a_family_reply_gives_the_id_and_the_multicast_groups_found_among_the_messages() {
    let family = parse_family(family_payload().as_bytes()).unwrap();
    assert_eq!(family.id, FAMILY);
    assert_eq!(
        family.groups,
        [("scan".to_owned(), 5), ("mlme".to_owned(), 7)]
    );
    let bare = genl(1, |b| {
        b.attr(1, &FAMILY.to_ne_bytes());
    });
    assert!(parse_family(bare.as_bytes()).unwrap().groups.is_empty());

    let mut raw = frame(2, 0, 1, &[0; 4]);
    raw.extend(frame(0x10, 0, 1, family_payload().as_bytes()));
    assert_eq!(parse_family_reply(&raw).unwrap().id, FAMILY);
}

#[test]
fn a_family_without_an_id_or_a_group_without_a_name_or_id_is_refused() {
    let group = |build: fn(&mut Body)| {
        genl(1, move |b| {
            b.attr(1, &FAMILY.to_ne_bytes()).nested(7, |groups| {
                groups.nested(1, build);
            });
        })
    };
    let no_id = genl(1, |b| {
        b.attr_cstr(2, "nl80211");
    });
    for (payload, expected) in [
        (no_id, missing("family", "CTRL_ATTR_FAMILY_ID")),
        (
            group(|g| {
                g.attr_cstr(1, "scan");
            }),
            missing("multicast group", "CTRL_ATTR_MCAST_GRP_ID"),
        ),
        (
            group(|g| {
                g.attr_u32(2, 5);
            }),
            missing("multicast group", "CTRL_ATTR_MCAST_GRP_NAME"),
        ),
    ] {
        assert_eq!(
            parse_family(payload.as_bytes()).map(drop),
            Err(expected.clone()),
            "{expected:?}"
        );
    }

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
fn a_dump_lists_its_objects_of_the_family_and_names_its_request_when_damaged() {
    let mut raw = frame(FAMILY + 1, 2, 1, interface_payload().as_bytes());
    raw.extend(dump(&[interface_payload(), p2p_device()]));
    let interfaces = parse_interfaces(&raw, FAMILY).unwrap();
    assert_eq!(
        interfaces
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        ["wlan0"],
        "network devices of the family only"
    );

    let raw = dump(&[
        phy(Some(1), Some("phy1")),
        phy(Some(1), None),
        phy(Some(0), Some("phy0")),
        phy(Some(1), Some("phy1")),
    ]);
    assert_eq!(
        parse_wiphys(&raw, FAMILY)
            .unwrap()
            .iter()
            .map(|w| (w.index, w.name.clone()))
            .collect::<Vec<_>>(),
        [(1, "phy1".to_owned()), (0, "phy0".to_owned())],
        "each phy once in order"
    );

    let raw = dump(&[station_payload(Some(-40)), station_payload(Some(-80))]);
    assert_eq!(parse_stations(&raw, FAMILY).unwrap().len(), 2);
    assert!(matches!(
        parse_stations(&dump(&[genl(19, |_| {})]), FAMILY),
        Err(Error::Parse {
            origin: "NL80211_CMD_GET_STATION",
            ..
        })
    ));

    let good = scan_payload(|bss| {
        bss.attr(1, &BSSID)
            .attr_u32(2, 2412)
            .attr(6, &element(0, b"ok"));
    });
    let bad = scan_payload(|bss| {
        bss.attr_u32(2, 2412);
    });
    let scan = parse_scan_dump(&dump(&[good.clone(), bad, good]), FAMILY).unwrap();
    assert_eq!(
        (scan.results.len(), scan.errors.len()),
        (2, 1),
        "a scan keeps the good results and lists the bad"
    );
}

#[test]
fn only_a_missing_family_means_the_kernel_does_not_support_it() {
    for (error, expected) in [
        (
            Error::Kernel {
                request: "CTRL_CMD_GETFAMILY",
                errno: Errno::NOENT,
            },
            true,
        ),
        (
            Error::Kernel {
                request: "CTRL_CMD_GETFAMILY",
                errno: Errno::PERM,
            },
            false,
        ),
        (
            Error::UnexpectedSequence {
                expected: 1,
                actual: 2,
            },
            false,
        ),
    ] {
        assert_eq!(is_unsupported(&error), expected, "{error:?}");
    }
}
