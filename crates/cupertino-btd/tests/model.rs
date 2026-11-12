//! The model: bring-up, the merge of discovery results, power and discovery requests, connecting,
//! forgetting devices and the adapter going away.

use crate::common;

use common::*;
use cupertino_bluetooth::{
    Address, AddressType, AddressTypes, ClassOfDevice, Command, Eir, EirName, Event, IoCapability,
    KeyValue, LinkKey, LinkKeyType, LongTermKey, LongTermKeyType, Opcode, Settings, Status, Uuid,
};
use cupertino_btd::device::{DeviceKind, DeviceRef};
use cupertino_btd::keys::KeySet;
use cupertino_btd::model::{Model, CONNECT_TIMEOUT, DEVICE_TTL, DISCOVERY_DURATION, REPLY_TIMEOUT};
use cupertino_btd::request::{AdapterProps, Change, Effect, Request, RequestError, Ticket};

fn reference(device: cupertino_bluetooth::DeviceId) -> DeviceRef {
    DeviceRef {
        address: device.address,
        kind: Some(device.kind),
    }
}

fn named(text: &str, complete: bool) -> Eir {
    Eir {
        name: Some(EirName {
            text: text.to_owned(),
            complete,
        }),
        ..Eir::default()
    }
}

#[test]
fn bring_up_loads_the_keys_into_the_kernel_and_sets_the_pairing_policy() {
    let mut keys = KeySet::default();
    let link = LinkKey {
        device: phone(),
        key_type: LinkKeyType::AuthenticatedP256,
        value: KeyValue([7; 16]),
        pin_length: 0,
    };
    let ltk = LongTermKey {
        device: device(0x03, AddressType::LeRandom),
        key_type: LongTermKeyType::UnauthenticatedP256,
        central: true,
        encryption_size: 16,
        diversifier: 0,
        random: [0; 8],
        value: KeyValue([9; 16]),
    };
    keys.set_link_key(link);
    keys.set_name(&phone(), "ignored: no bond name yet");
    keys.set_long_term_key(ltk);

    let (harness, effects) = Harness::with(POWERED, keys);
    assert_eq!(
        sent(&effects),
        [
            Command::LoadLinkKeys(vec![link]),
            Command::LoadLongTermKeys(vec![ltk]),
            Command::SetIoCapability(IoCapability::KeyboardDisplay),
            Command::SetBondable(true),
            Command::SetConnectable(true),
            Command::GetConnections,
        ]
    );
    // The adapter is available and powered; the bonded devices are listed as paired, offline.
    assert_eq!(
        changes(&effects).last(),
        Some(&Change::Adapter {
            old: AdapterProps::default(),
            new: AdapterProps {
                available: true,
                powered: true,
                discovering: false
            }
        })
    );
    let devices = harness.model.devices();
    assert_eq!(
        devices
            .iter()
            .map(|d| (d.key(), d.paired, d.bonded, d.connected))
            .collect::<Vec<_>>(),
        [
            ("11:22:33:44:55:01/bredr".to_owned(), true, true, false),
            ("11:22:33:44:55:03/le-random".to_owned(), true, true, false),
        ]
    );
    assert_eq!(harness.model.adapter().unwrap().name, "test-adapter");
}

#[test]
fn a_powered_off_adapter_is_not_asked_for_its_connections() {
    let (_, effects) = Harness::with(OFF, KeySet::default());
    assert!(!sent(&effects).contains(&Command::GetConnections));
}

#[test]
fn a_second_report_of_a_device_updates_its_name_and_signal_strength() {
    let mut h = Harness::powered();
    // First sighting: a shortened name and a signal strength.
    let effects = h.found(phone(), Some(-70), 0, named("Pho", false));
    let Change::DeviceAdded(info) = only_change(&effects) else {
        panic!("expected a DeviceAdded")
    };
    assert_eq!(
        (info.name.as_deref(), info.rssi, info.paired, info.connected),
        (Some("Pho"), Some(-70), false, false)
    );

    // A shortened name does not replace an existing one, a complete name does.
    assert!(changes(&h.found(phone(), Some(-70), 0, named("Xyz", false))).is_empty());
    let effects = h.found(phone(), Some(-55), 0, named("Phone of Rafael", true));
    let Change::DeviceChanged(info) = only_change(&effects) else {
        panic!("expected a DeviceChanged")
    };
    assert_eq!(
        (info.name.as_deref(), info.rssi),
        (Some("Phone of Rafael"), Some(-55))
    );

    // An unavailable RSSI keeps the last reading and changes nothing.
    assert!(changes(&h.found(phone(), None, 0, Eir::default())).is_empty());
    assert_eq!(h.model.devices()[0].rssi, Some(-55));
}

#[test]
fn class_appearance_and_uuids_accumulate_and_pick_the_icon() {
    let mut h = Harness::powered();
    // Class 0x002540: major 5 (peripheral), minor 0x10 (keyboard).
    let class = ClassOfDevice::from_wire([0x40, 0x25, 0x00]);
    h.found(
        keyboard(),
        Some(-40),
        0,
        Eir {
            class: Some(class),
            uuids: vec![Uuid::from_u16(0x1124)],
            ..Eir::default()
        },
    );
    h.found(
        keyboard(),
        Some(-40),
        0,
        Eir {
            uuids: vec![Uuid::from_u16(0x1124), Uuid::from_u16(0x180F)],
            ..Eir::default()
        },
    );
    let info = &h.model.devices()[0];
    assert_eq!(info.icon, DeviceKind::Keyboard);
    assert_eq!(info.class, Some(class));
    assert_eq!(info.uuids, [Uuid::from_u16(0x1124), Uuid::from_u16(0x180F)]);

    // An appearance wins over the class: 0x03C2 is a mouse.
    h.found(
        keyboard(),
        None,
        0,
        Eir {
            appearance: Some(0x03C2),
            ..Eir::default()
        },
    );
    assert_eq!(h.model.devices()[0].icon, DeviceKind::Mouse);
}

#[test]
fn icons_follow_the_assigned_numbers() {
    let class = |bytes: [u8; 3]| Some(ClassOfDevice::from_wire(bytes));
    // Class of device 0x5A020C: smartphone (major 2, minor 3).
    assert_eq!(
        DeviceKind::classify(class([0x0C, 0x02, 0x5A]), None),
        DeviceKind::Phone
    );
    // 0x240404: audio/video, minor 1: wearable headset.
    assert_eq!(
        DeviceKind::classify(class([0x04, 0x04, 0x24]), None),
        DeviceKind::Headset
    );
    // 0x240418: audio/video, minor 6: headphones.
    assert_eq!(
        DeviceKind::classify(class([0x18, 0x04, 0x24]), None),
        DeviceKind::Headphones
    );
    // 0x240414: audio/video, minor 5: loudspeaker.
    assert_eq!(
        DeviceKind::classify(class([0x14, 0x04, 0x24]), None),
        DeviceKind::Speaker
    );
    // 0x002580: peripheral, pointing device.
    assert_eq!(
        DeviceKind::classify(class([0x80, 0x25, 0x00]), None),
        DeviceKind::Mouse
    );
    // 0x002508: peripheral, gamepad.
    assert_eq!(
        DeviceKind::classify(class([0x08, 0x25, 0x00]), None),
        DeviceKind::Gamepad
    );
    // 0x00010C: computer, laptop (minor 3).
    assert_eq!(
        DeviceKind::classify(class([0x0C, 0x01, 0x00]), None),
        DeviceKind::Computer
    );
    // 0x00011C: computer, tablet (minor 7).
    assert_eq!(
        DeviceKind::classify(class([0x1C, 0x01, 0x00]), None),
        DeviceKind::Tablet
    );
    // Appearance 0x00C0 watch, 0x03C1 keyboard, 0x0943 headphones (wearable audio, 3), 0x0841
    // speaker (audio sink, 1), 0x0081 desktop computer.
    assert_eq!(DeviceKind::classify(None, Some(0x00C0)), DeviceKind::Watch);
    assert_eq!(
        DeviceKind::classify(None, Some(0x03C1)),
        DeviceKind::Keyboard
    );
    assert_eq!(
        DeviceKind::classify(None, Some(0x0943)),
        DeviceKind::Headphones
    );
    assert_eq!(
        DeviceKind::classify(None, Some(0x0841)),
        DeviceKind::Speaker
    );
    assert_eq!(
        DeviceKind::classify(None, Some(0x0081)),
        DeviceKind::Computer
    );
    // An appearance the table does not know falls back to the class.
    assert_eq!(
        DeviceKind::classify(class([0x0C, 0x02, 0x5A]), Some(0xFFC0)),
        DeviceKind::Phone
    );
    assert_eq!(DeviceKind::classify(None, None), DeviceKind::Unknown);
}

#[test]
fn a_device_whose_name_is_unknown_is_looked_up_by_the_kernel() {
    let mut h = Harness::powered();
    // Flag bit 0, "Confirm name": the kernel asks whether the name is known.
    let effects = h.found(phone(), Some(-60), 1, Eir::default());
    assert_eq!(
        sent(&effects),
        [Command::ConfirmName {
            device: phone(),
            name_known: false
        }]
    );
    let effects = h.found(phone(), Some(-60), 1, named("Phone", true));
    assert_eq!(
        sent(&effects),
        [Command::ConfirmName {
            device: phone(),
            name_known: true
        }]
    );
}

#[test]
fn a_device_that_was_only_discovered_expires_unless_paired_or_connected() {
    let mut h = Harness::powered();
    h.discover(phone());
    h.discover(keyboard());
    h.packet(Event::DeviceConnected {
        device: keyboard(),
        flags: 0,
        eir: Eir::default(),
    });
    assert_eq!(h.model.next_deadline(), Some(h.now + DEVICE_TTL));

    assert!(h.advance(DEVICE_TTL.as_secs() - 1).is_empty());
    let effects = h.advance(1);
    assert_eq!(
        changes(&effects),
        [Change::DeviceRemoved("11:22:33:44:55:01/bredr".to_owned())]
    );
    assert_eq!(
        h.model
            .devices()
            .iter()
            .map(|d| d.key())
            .collect::<Vec<_>>(),
        ["11:22:33:44:55:02/bredr"]
    );
}

#[test]
fn powering_on_sends_set_powered_and_answers_with_the_new_settings() {
    let (mut h, _) = Harness::with(OFF, KeySet::default());
    let (ticket, effects) = h.request(Request::SetPowered(true));
    assert_eq!(sent(&effects), [Command::SetPowered(true)]);
    assert!(completions(&effects).is_empty());

    let effects = h.reply(
        Opcode::SET_POWERED,
        Status::Success,
        POWERED.to_le_bytes().to_vec(),
    );
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
    assert_eq!(
        changes(&effects),
        [Change::Adapter {
            old: AdapterProps {
                available: true,
                powered: false,
                discovering: false
            },
            new: AdapterProps {
                available: true,
                powered: true,
                discovering: false
            }
        }]
    );
    assert!(h
        .model
        .adapter()
        .unwrap()
        .settings
        .contains(Settings::POWERED));
}

#[test]
fn a_command_the_controller_never_answers_fails_its_request_after_the_reply_timeout() {
    let (mut h, _) = Harness::with(OFF, KeySet::default());
    let (ticket, _) = h.request(Request::SetPowered(true));
    assert_eq!(h.model.next_deadline(), Some(h.now + REPLY_TIMEOUT));
    assert!(h.advance(REPLY_TIMEOUT.as_secs() - 1).is_empty());
    let effects = h.advance(1);
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::Timeout))]
    );
    assert_eq!(h.model.next_deadline(), None);
}

#[test]
fn powering_to_the_state_it_is_in_is_answered_at_once_and_a_failure_is_passed_on() {
    let mut h = Harness::powered();
    let (ticket, effects) = h.request(Request::SetPowered(true));
    assert_eq!(
        (sent(&effects), completions(&effects)),
        (vec![], vec![(ticket, Ok(()))])
    );

    let (ticket, _) = h.request(Request::SetPowered(false));
    let effects = h.reply(Opcode::SET_POWERED, Status::RfKilled, vec![]);
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::Blocked))]
    );
    assert!(h.model.props().powered);
}

#[test]
fn powering_off_ends_connections_discovery_and_waiting_requests() {
    let mut h = Harness::powered();
    h.discover(phone());
    h.packet(Event::DeviceConnected {
        device: phone(),
        flags: 0,
        eir: Eir::default(),
    });
    h.request(Request::StartDiscovery);
    h.reply(Opcode::START_DISCOVERY, Status::Success, vec![7]);
    assert!(h.model.props().discovering);
    let (disconnect, _) = h.request(Request::Disconnect(reference(phone())));

    let effects = h.packet(Event::NewSettings(Settings(OFF)));
    assert_eq!(
        completions(&effects),
        [(disconnect, Err(RequestError::NotPowered))]
    );
    assert!(effects.contains(&Effect::CloseLink {
        adapter: ADAPTER,
        device: phone()
    }));
    assert!(!h.model.devices()[0].connected);
    assert_eq!(
        h.model.props(),
        AdapterProps {
            available: true,
            powered: false,
            discovering: false
        }
    );
}

#[test]
fn discovery_needs_power_runs_in_rounds_and_stops_at_its_time() {
    let (mut h, _) = Harness::with(OFF, KeySet::default());
    let (ticket, effects) = h.request(Request::StartDiscovery);
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::NotPowered))]
    );

    let mut h = Harness::powered();
    let (ticket, effects) = h.request(Request::StartDiscovery);
    // BR/EDR and LE, interleaved.
    assert_eq!(sent(&effects), [Command::StartDiscovery(AddressTypes::ALL)]);
    let effects = h.reply(Opcode::START_DISCOVERY, Status::Success, vec![7]);
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
    assert!(h.model.props().discovering);

    // The kernel ends its round; the daemon starts the next while the user wants discovery.
    assert_eq!(
        sent(&h.packet(Event::Discovering {
            types: AddressTypes::ALL,
            active: false
        })),
        [Command::StartDiscovery(AddressTypes::ALL)]
    );
    h.reply(Opcode::START_DISCOVERY, Status::Success, vec![7]);
    assert_eq!(h.model.next_deadline(), Some(h.now + DISCOVERY_DURATION));

    // At the end of its time the daemon stops it.
    h.advance(DISCOVERY_DURATION.as_secs() - 1);
    assert!(h.model.props().discovering);
    let effects = h.advance(1);
    assert_eq!(sent(&effects), [Command::StopDiscovery(AddressTypes::ALL)]);
    assert!(!h.model.props().discovering);
}

#[test]
fn stopping_discovery_does_not_let_the_next_round_start() {
    let mut h = Harness::powered();
    h.request(Request::StartDiscovery);
    h.reply(Opcode::START_DISCOVERY, Status::Success, vec![7]);

    let (ticket, effects) = h.request(Request::StopDiscovery);
    assert_eq!(sent(&effects), [Command::StopDiscovery(AddressTypes::ALL)]);
    assert!(!h.model.props().discovering);
    // The kernel's event arrives before the reply: no new round.
    assert!(sent(&h.packet(Event::Discovering {
        types: AddressTypes::ALL,
        active: false
    }))
    .is_empty());
    let effects = h.reply(Opcode::STOP_DISCOVERY, Status::Success, vec![7]);
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);

    // Stopping when it is not running is a success.
    let (ticket, effects) = h.request(Request::StopDiscovery);
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
}

#[test]
fn a_discovery_the_kernel_refused_is_not_left_on() {
    let mut h = Harness::powered();
    let (ticket, _) = h.request(Request::StartDiscovery);
    let effects = h.reply(Opcode::START_DISCOVERY, Status::Busy, vec![7]);
    assert_eq!(completions(&effects), [(ticket, Err(RequestError::Busy))]);
    assert!(!h.model.props().discovering);
}

#[test]
fn connecting_opens_a_link_and_finishes_when_the_kernel_reports_the_connection() {
    let mut h = Harness::powered();
    h.discover(phone());
    let (ticket, effects) = h.request(Request::Connect(reference(phone())));
    assert_eq!(
        effects,
        [Effect::OpenLink {
            index: C0,
            adapter: ADAPTER,
            device: phone(),
            ticket
        }]
    );

    let effects = h.packet(Event::DeviceConnected {
        device: phone(),
        flags: 8,
        eir: named("Phone", true),
    });
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
    let info = &h.model.devices()[0];
    assert_eq!(
        (info.connected, info.name.as_deref()),
        (true, Some("Phone"))
    );

    let (ticket, effects) = h.request(Request::Connect(reference(phone())));
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::AlreadyConnected))]
    );
}

#[test]
fn a_connection_that_fails_or_takes_too_long_is_an_error() {
    let mut h = Harness::powered();
    h.discover(phone());
    let (ticket, _) = h.request(Request::Connect(reference(phone())));
    let effects = h.packet(Event::ConnectFailed {
        device: phone(),
        status: Status::ConnectFailed,
    });
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::ConnectFailed))]
    );
    assert!(effects.contains(&Effect::CloseLink {
        adapter: ADAPTER,
        device: phone()
    }));

    let (ticket, _) = h.request(Request::Connect(reference(phone())));
    assert!(h.advance(CONNECT_TIMEOUT.as_secs() - 1).is_empty());
    let effects = h.advance(1);
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::Timeout))]
    );
    assert!(effects.contains(&Effect::CloseLink {
        adapter: ADAPTER,
        device: phone()
    }));
}

#[test]
fn a_link_that_cannot_be_opened_fails_the_request() {
    let mut h = Harness::powered();
    h.discover(phone());
    let (ticket, _) = h.request(Request::Connect(reference(phone())));
    let effects = h
        .model
        .link_failed(ticket, "connect(l2cap): host is down".to_owned());
    assert_eq!(
        completions(&effects),
        [(
            ticket,
            Err(RequestError::Transport(
                "connect(l2cap): host is down".to_owned()
            ))
        )]
    );
}

#[test]
fn disconnecting_sends_the_command_and_follows_the_events() {
    let mut h = Harness::powered();
    h.discover(phone());
    let (ticket, effects) = h.request(Request::Disconnect(reference(phone())));
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::NotConnected))]
    );

    h.packet(Event::DeviceConnected {
        device: phone(),
        flags: 0,
        eir: Eir::default(),
    });
    let (ticket, effects) = h.request(Request::Disconnect(reference(phone())));
    assert_eq!(sent(&effects), [Command::Disconnect(phone())]);
    // The event may come before or after the reply; either order finishes the request once.
    let effects = h.packet(Event::DeviceDisconnected {
        device: phone(),
        reason: cupertino_bluetooth::DisconnectReason::LocalHost,
    });
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
    assert!(effects.contains(&Effect::CloseLink {
        adapter: ADAPTER,
        device: phone()
    }));
    assert!(h
        .reply(Opcode::DISCONNECT, Status::Success, device_reply(phone()))
        .is_empty());
    assert!(!h.model.devices()[0].connected);
}

#[test]
fn get_connections_marks_the_devices_that_are_already_connected() {
    let mut h = Harness::powered();
    let mut data = vec![1, 0];
    data.extend(device_reply(phone()));
    let effects = h.reply(Opcode::GET_CONNECTIONS, Status::Success, data);
    let Change::DeviceAdded(info) = only_change(&effects) else {
        panic!("expected a DeviceAdded")
    };
    assert!(info.connected);
}

#[test]
fn removing_a_discovered_device_forgets_it_without_asking_the_kernel() {
    let mut h = Harness::powered();
    h.discover(phone());
    let (ticket, effects) = h.request(Request::Remove(reference(phone())));
    assert_eq!(sent(&effects), []);
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
    assert_eq!(
        changes(&effects),
        [Change::DeviceRemoved("11:22:33:44:55:01/bredr".to_owned())]
    );
    assert!(h.model.devices().is_empty());
}

#[test]
fn removing_a_paired_device_unpairs_it_and_deletes_its_keys() {
    let mut keys = KeySet::default();
    keys.set_link_key(LinkKey {
        device: phone(),
        key_type: LinkKeyType::Combination,
        value: KeyValue([1; 16]),
        pin_length: 0,
    });
    let (mut h, _) = Harness::with(POWERED, keys);

    let (ticket, effects) = h.request(Request::Remove(reference(phone())));
    assert_eq!(
        sent(&effects),
        [Command::UnpairDevice {
            device: phone(),
            disconnect: true
        }]
    );
    let effects = h.reply(
        Opcode::UNPAIR_DEVICE,
        Status::Success,
        device_reply(phone()),
    );
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
    assert!(effects.contains(&Effect::SaveKeys {
        address: ADAPTER,
        keys: KeySet::default()
    }));
    assert_eq!(
        changes(&effects),
        [Change::DeviceRemoved("11:22:33:44:55:01/bredr".to_owned())]
    );
    assert!(h.model.devices().is_empty());
}

#[test]
fn unpairing_from_another_client_removes_the_keys_here_too() {
    let mut keys = KeySet::default();
    keys.set_link_key(LinkKey {
        device: phone(),
        key_type: LinkKeyType::Combination,
        value: KeyValue([1; 16]),
        pin_length: 0,
    });
    let (mut h, _) = Harness::with(POWERED, keys);
    let effects = h.packet(Event::DeviceUnpaired(phone()));
    assert!(effects.contains(&Effect::SaveKeys {
        address: ADAPTER,
        keys: KeySet::default()
    }));
    let info = &h.model.devices()[0];
    assert_eq!((info.paired, info.bonded), (false, false));
}

#[test]
fn an_address_shared_by_two_address_types_must_be_named_in_full() {
    let mut h = Harness::powered();
    let classic = device(0x09, AddressType::BrEdr);
    let low_energy = device(0x09, AddressType::LePublic);
    h.discover(classic);
    h.discover(low_energy);

    let by_address = DeviceRef {
        address: classic.address,
        kind: None,
    };
    let (ticket, effects) = h.request(Request::Connect(by_address));
    assert_eq!(
        completions(&effects),
        [(
            ticket,
            Err(RequestError::AmbiguousDevice(
                "11:22:33:44:55:09".to_owned()
            ))
        )]
    );
    let (_, effects) = h.request(Request::Connect(reference(low_energy)));
    assert!(
        matches!(effects.as_slice(), [Effect::OpenLink { device, .. }] if *device == low_energy)
    );
    let (ticket, effects) = h.request(Request::Connect(DeviceRef {
        address: Address::from_wire([0xFF; 6]),
        kind: None,
    }));
    assert_eq!(
        completions(&effects),
        [(
            ticket,
            Err(RequestError::UnknownDevice("FF:FF:FF:FF:FF:FF".to_owned()))
        )]
    );
}

#[test]
fn a_removed_adapter_takes_its_devices_and_waiting_requests_with_it() {
    let mut h = Harness::powered();
    h.discover(phone());
    let (ticket, _) = h.request(Request::Connect(reference(phone())));
    let effects = h.packet(Event::IndexRemoved);
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::NoAdapter))]
    );
    assert!(
        changes(&effects).contains(&Change::DeviceRemoved("11:22:33:44:55:01/bredr".to_owned()))
    );
    assert_eq!(h.model.props(), AdapterProps::default());
    let (ticket, effects) = h.request(Request::SetPowered(true));
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::NoAdapter))]
    );
}

#[test]
fn an_adapter_that_appears_later_is_read_and_set_up() {
    let mut h = Harness::powered();
    let second = cupertino_bluetooth::ControllerIndex(1);
    let effects = h.model.on_packet(
        h.now,
        cupertino_bluetooth::Packet {
            index: second,
            event: Event::IndexAdded,
        },
    );
    assert_eq!(
        effects,
        [Effect::Send {
            index: second,
            command: Command::ReadControllerInfo
        }]
    );
    // The second adapter is not the default, so its devices stay off the bus.
    h.model.on_packet(
        h.now,
        cupertino_bluetooth::Packet {
            index: second,
            event: complete(
                Opcode::READ_CONTROLLER_INFO,
                Status::Success,
                info_reply(POWERED),
            ),
        },
    );
    h.model.keys_loaded(h.now, second, KeySet::default());
    h.model.on_packet(
        h.now,
        cupertino_bluetooth::Packet {
            index: second,
            event: Event::DeviceFound {
                device: phone(),
                rssi: None,
                flags: cupertino_bluetooth::FoundFlags(0),
                eir: Eir::default(),
            },
        },
    );
    assert!(h.model.devices().is_empty());
}

#[test]
fn a_read_only_model_reads_and_refuses_every_request() {
    let mut model = Model::new(true);
    let now = std::time::Duration::ZERO;
    model.on_packet(
        now,
        cupertino_bluetooth::Packet {
            index: C0,
            event: complete(
                Opcode::READ_CONTROLLER_INFO,
                Status::Success,
                info_reply(POWERED),
            ),
        },
    );
    // Ready at once, with nothing sent to the controller.
    assert_eq!(
        model.props(),
        AdapterProps {
            available: true,
            powered: true,
            discovering: false
        }
    );
    let effects = model.request(now, Ticket(1), Request::SetPowered(false));
    assert_eq!(
        completions(&effects),
        [(Ticket(1), Err(RequestError::ReadOnly))]
    );
    assert!(sent(&effects).is_empty());
}

#[test]
fn a_failed_command_nobody_waits_for_is_reported() {
    let mut h = Harness::powered();
    let effects = h.reply(Opcode::SET_BONDABLE, Status::PermissionDenied, vec![]);
    assert_eq!(
        effects,
        [Effect::Log(
            "command 0x0009 failed: permission denied".to_owned()
        )]
    );
}

#[test]
fn a_key_the_kernel_says_not_to_store_pairs_the_device_without_writing_a_file() {
    let mut h = Harness::powered();
    h.discover(phone());
    let key = LinkKey {
        device: phone(),
        key_type: LinkKeyType::UnauthenticatedP256,
        value: KeyValue([5; 16]),
        pin_length: 0,
    };
    let effects = h.packet(Event::NewLinkKey { store: false, key });
    assert!(!effects.iter().any(|e| matches!(e, Effect::SaveKeys { .. })));
    let info = &h.model.devices()[0];
    assert_eq!((info.paired, info.bonded), (true, false));

    let key = LinkKey {
        device: keyboard(),
        key_type: LinkKeyType::UnauthenticatedP256,
        value: KeyValue([6; 16]),
        pin_length: 0,
    };
    let effects = h.packet(Event::NewLinkKey { store: true, key });
    let saved = effects.iter().find_map(|e| match e {
        Effect::SaveKeys { keys, .. } => Some(keys.clone()),
        _ => None,
    });
    assert_eq!(saved.unwrap().link_keys(), [key]);
}
