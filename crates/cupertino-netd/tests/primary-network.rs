use std::net::Ipv4Addr;

use cupertino_netd::dhcp::client::Lease;
use cupertino_netd::model::{Effect, Input, Model};
use cupertino_netd::netlink::rtnetlink::{Link, LinkClass, LinkFlags, OperState};
use cupertino_netd::netlink::Ssid;
use cupertino_netd::nm;
use cupertino_netd::snapshot::Snapshot;
use cupertino_netd::supplicant::SupplicantEvent;

fn link(index: u32, name: &str, class: LinkClass) -> Input {
    Input::LinkUpdated {
        class,
        link: Link {
            index,
            name: name.into(),
            hardware_type: 1,
            flags: LinkFlags(0x10001), // IFF_UP | IFF_LOWER_UP (carrier)
            oper_state: OperState::Up,
            mac: None,
            mtu: None,
            info_kind: None,
            master: None,
        },
    }
}

fn lease(subnet: u8) -> Lease {
    let gateway = Ipv4Addr::new(192, 168, subnet, 1);
    Lease {
        address: Ipv4Addr::new(192, 168, subnet, 2),
        prefix_len: 24,
        routers: vec![gateway],
        dns_servers: vec![gateway],
        domain_name: None,
        server: gateway,
        duration: None,
    }
}

#[test]
fn dns_and_the_reported_default_follow_ethernet_then_fall_back_to_wifi() {
    let mut model = Model::new();
    model.apply(link(4, "wlan0", LinkClass::Wireless));
    model.apply(Input::Supplicant(SupplicantEvent::Connected {
        ifindex: 4,
        ssid: Ssid(b"Office".to_vec()),
    }));
    let wifi = lease(4);
    let wired = lease(9);
    for (event, primary, dns) in [
        (
            Input::LeaseBound {
                ifindex: 4,
                lease: wifi.clone(),
            },
            "802-11-wireless",
            Some(wifi.server),
        ),
        (link(9, "eth0", LinkClass::Wired), "802-11-wireless", None),
        (
            Input::LeaseBound {
                ifindex: 9,
                lease: wired.clone(),
            },
            "802-3-ethernet",
            Some(wired.server),
        ),
        (
            Input::LeaseBound {
                ifindex: 9,
                lease: wired,
            },
            "802-3-ethernet",
            None,
        ),
        (
            Input::LinkRemoved { ifindex: 9 },
            "802-11-wireless",
            Some(wifi.server),
        ),
        (Input::LinkRemoved { ifindex: 4 }, "", None),
    ] {
        let effects = model.apply(event);
        let snapshot = Snapshot::of(&model);
        assert_eq!(nm::primary_connection_type(&snapshot), primary);
        assert_eq!(
            nm::state(&snapshot).name(),
            if primary.is_empty() {
                "disconnected"
            } else {
                "connected"
            }
        );
        let writes: Vec<_> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::WriteDns { servers, .. } => Some(servers.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            writes,
            dns.map(|server| vec![vec![server]]).unwrap_or_default()
        );
    }
}
