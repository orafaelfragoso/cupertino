//! Controller reports and user requests through the Bluetooth daemon's public model.

use std::time::Duration;

use cupertino_btd::bluetooth::{
    AddressType, ControllerIndex, DeviceId, Eir, Event, Opcode, Packet, Status,
};
use cupertino_btd::device::DeviceRef;
use cupertino_btd::keys::KeySet;
use cupertino_btd::model::{Model, CONNECT_TIMEOUT};
use cupertino_btd::request::{Effect, Request, RequestError, Ticket};

fn info(index: ControllerIndex) -> Packet {
    // Management Read Controller Information: a powered adapter, no name or class.
    let mut data = vec![0u8; 6 + 1 + 2 + 4 + 4 + 3 + 249 + 11];
    data[0] = index.0 as u8;
    data[13] = 1;
    Packet {
        index,
        event: Event::CommandComplete {
            opcode: Opcode::READ_CONTROLLER_INFO,
            status: Status::Success,
            data,
        },
    }
}

fn bond() -> (KeySet, DeviceId) {
    let keys = KeySet::parse("cupertino-bluetooth-keys 1\ndevice 02:00:00:00:00:01 bredr\nname 48656164736574\nlink 08 04 30313233343536373839616263646566\n").unwrap();
    let device = DeviceId {
        address: "02:00:00:00:00:01".parse().unwrap(),
        kind: AddressType::BrEdr,
    };
    (keys, device)
}

#[test]
fn the_lowest_ready_adapter_supplies_the_inventory_and_unplug_falls_back_to_the_other() {
    let now = Duration::ZERO;
    let (low, high) = (ControllerIndex(1), ControllerIndex(3));
    let (keys, device) = bond();
    let mut model = Model::new(false);
    for index in [high, low] {
        let effects = model.on_packet(now, info(index));
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::LoadKeys { index: i, .. } if *i == index)));
    }
    assert!(!model.props().available);
    model.keys_loaded(now, high, keys);
    assert_eq!(model.adapter().unwrap().index, high);
    model.keys_loaded(now, low, KeySet::default());
    assert_eq!(model.adapter().unwrap().index, low);
    assert!(model.devices().is_empty());
    model.on_packet(
        now,
        Packet {
            index: low,
            event: Event::IndexRemoved,
        },
    );
    assert_eq!(model.adapter().unwrap().index, high);
    let inventory = model.devices();
    let [headset] = inventory.as_slice() else {
        panic!("expected one restored bond")
    };
    assert_eq!(
        (
            headset.id,
            headset.name.as_deref(),
            headset.paired,
            headset.bonded
        ),
        (device, Some("Headset"), true, true)
    );
    model.on_packet(
        now,
        Packet {
            index: high,
            event: Event::IndexRemoved,
        },
    );
    assert!(!model.props().available);
    assert!(model.devices().is_empty());
}

#[test]
fn connect_waits_for_the_device_and_completes_the_callers_ticket_once() {
    let now = Duration::from_secs(10);
    let index = ControllerIndex(2);
    let ticket = Ticket(42);
    let (keys, device) = bond();
    for (event, expected, connected) in [
        (
            Some(Event::DeviceConnected {
                device,
                flags: 0,
                eir: Eir::default(),
            }),
            Ok(()),
            Some(true),
        ),
        (
            Some(Event::ConnectFailed {
                device,
                status: Status::ConnectFailed,
            }),
            Err(RequestError::ConnectFailed),
            Some(false),
        ),
        (None, Err(RequestError::Timeout), Some(false)),
        (
            Some(Event::IndexRemoved),
            Err(RequestError::NoAdapter),
            None,
        ),
    ] {
        let mut model = Model::new(false);
        model.on_packet(now, info(index));
        model.keys_loaded(now, index, keys.clone());
        let reference = DeviceRef {
            address: device.address,
            kind: Some(device.kind),
        };
        let begin = model.request(now, ticket, Request::Connect(reference));
        assert!(begin.iter().any(|e| matches!(e, Effect::OpenLink { ticket: t, device: d, .. } if (*t,*d) == (ticket,device))));
        assert!(!begin.iter().any(|e| matches!(e, Effect::Complete { .. })));
        assert!(model.devices()[0].connecting);
        let effects = match event {
            Some(event) => model.on_packet(now, Packet { index, event }),
            None => {
                assert!(model
                    .tick(now + CONNECT_TIMEOUT - Duration::from_millis(1))
                    .is_empty());
                model.tick(now + CONNECT_TIMEOUT)
            }
        };
        let replies: Vec<_> = effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::Complete { ticket, result } => Some((ticket, result)),
                _ => None,
            })
            .collect();
        assert_eq!(replies, [(ticket, expected)]);
        let rows = model.devices();
        assert_eq!(
            rows.iter()
                .find(|row| row.id == device)
                .map(|row| (row.connecting, row.connected)),
            connected.map(|v| (false, v))
        );
        assert!(!model
            .tick(now + CONNECT_TIMEOUT * 2)
            .iter()
            .any(|e| matches!(e, Effect::Complete { .. })));
    }
}
