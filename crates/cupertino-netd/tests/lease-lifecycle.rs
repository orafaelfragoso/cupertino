//! DHCP as seen by the host that sends requests and installs or removes the resulting lease.

use std::net::Ipv4Addr;
use std::time::Duration;

use cupertino_netd::dhcp::client::{Action, Client, ClientConfig};
use cupertino_netd::dhcp::message::{Message, MessageType};
use cupertino_netd::netlink::MacAddr;

fn sent(actions: &[Action], kind: MessageType, to: Ipv4Addr) -> &Message {
    let [Action::Send {
        message,
        destination,
    }] = actions
    else {
        panic!("expected {kind:?}, got {actions:?}")
    };
    assert_eq!((message.message_type, *destination), (kind, to));
    message
}

#[test]
fn a_lease_renews_with_its_server_then_rebinds_and_loses_configuration_on_expiry_or_nak() {
    let seconds = Duration::from_secs;
    let server = Ipv4Addr::new(192, 0, 2, 1);
    let address = Ipv4Addr::new(192, 0, 2, 17);
    let mac = MacAddr([2, 0, 0, 0, 0, 17]);
    for expired in [true, false] {
        let mut client = Client::new(ClientConfig {
            hardware: mac,
            xid_seed: 195,
        });
        let discover = client.start(seconds(0));
        let xid = sent(&discover, MessageType::Discover, Ipv4Addr::BROADCAST).xid;
        let mut offer = Message::new(MessageType::Offer, xid, mac);
        offer.your_address = address;
        offer.options.server_id = Some(server);
        for other in [
            Message {
                xid: xid + 1,
                ..offer.clone()
            },
            Message {
                client_hardware: MacAddr([2; 6]),
                ..offer.clone()
            },
        ] {
            assert!(client.receive(seconds(1), &other).is_empty());
        }
        let request = client.receive(seconds(2), &offer);
        let request = sent(&request, MessageType::Request, Ipv4Addr::BROADCAST);
        assert_eq!(
            (request.options.requested_address, request.options.server_id),
            (Some(address), Some(server))
        );
        let mut ack = Message {
            message_type: MessageType::Ack,
            ..offer
        };
        ack.options.subnet_mask = Some(Ipv4Addr::new(255, 255, 255, 0));
        ack.options.lease_time = Some(80);
        ack.options.routers.push(server);
        ack.options.dns_servers.push(server);
        let bound = client.receive(seconds(5), &ack);
        let [Action::Bound(lease)] = bound.as_slice() else {
            panic!("ACK did not bind: {bound:?}")
        };
        assert_eq!(
            (
                lease.address,
                lease.prefix_len,
                lease.dns_servers.as_slice()
            ),
            (address, 24, [server].as_slice())
        );

        // T1/T2 count from the request, not from the later ACK.
        let mut current = xid;
        for (at, to) in [(42, server), (72, Ipv4Addr::BROADCAST)] {
            assert_eq!(client.next_deadline(), Some(seconds(at)));
            assert!(client.tick(seconds(at - 1)).is_empty());
            let renewal = client.tick(seconds(at));
            let renewal = sent(&renewal, MessageType::Request, to);
            assert_eq!(renewal.client_address, address);
            current = renewal.xid;
            assert_eq!(client.lease(), Some(lease));
        }
        let actions = if expired {
            client.tick(seconds(82))
        } else {
            client.receive(seconds(75), &Message::new(MessageType::Nak, current, mac))
        };
        assert_eq!(actions.first(), Some(&Action::Lost(lease.clone())));
        assert!(client.lease().is_none());
        if expired {
            sent(&actions[1..], MessageType::Discover, Ipv4Addr::BROADCAST);
        } else {
            assert_eq!(actions.len(), 1);
            assert!(client.tick(seconds(84)).is_empty());
            sent(
                &client.tick(seconds(85)),
                MessageType::Discover,
                Ipv4Addr::BROADCAST,
            );
        }
    }
}
