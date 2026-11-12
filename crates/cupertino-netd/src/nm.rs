//! The values of the `org.freedesktop.NetworkManager` subset, worked out from a [`Snapshot`].
//!
//! Numbers follow NetworkManager's D-Bus API (`NMState`, `NMConnectivityState`). There is no
//! connectivity probe: like NetworkManager with checking turned off, the daemon reports a
//! connection with a default route as `CONNECTED_GLOBAL` and the connectivity as `unknown`.

use crate::model::{DeviceKind, DeviceState};
use crate::snapshot::{DeviceView, Snapshot};

/// `NMState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum State {
    Disconnected = 20,
    Connecting = 40,
    ConnectedLocal = 50,
    ConnectedGlobal = 70,
}

/// `NMConnectivityState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Connectivity {
    Unknown = 0,
    None = 1,
}

/// The connected device that carries the default route: the lowest metric, then the lowest index.
fn primary(snapshot: &Snapshot) -> Option<&DeviceView> {
    snapshot
        .devices
        .iter()
        .filter(|device| device.state == DeviceState::Connected && device.has_router)
        .min_by_key(|device| (device.metric, device.ifindex))
}

/// The global state: a default route is global, an address without one is local, a device that
/// is on its way is connecting, and anything else is disconnected.
pub fn state(snapshot: &Snapshot) -> State {
    if primary(snapshot).is_some() {
        State::ConnectedGlobal
    } else if snapshot
        .devices
        .iter()
        .any(|device| device.state == DeviceState::Connected)
    {
        State::ConnectedLocal
    } else if snapshot
        .devices
        .iter()
        .any(|device| matches!(device.state, DeviceState::Configuring(_)))
    {
        State::Connecting
    } else {
        State::Disconnected
    }
}

impl State {
    /// The name `org.cupertino.Network1` shows.
    pub fn name(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::ConnectedLocal => "connected-local",
            Self::ConnectedGlobal => "connected",
        }
    }
}

pub fn connectivity(snapshot: &Snapshot) -> Connectivity {
    if snapshot
        .devices
        .iter()
        .any(|device| device.state == DeviceState::Connected)
    {
        Connectivity::Unknown
    } else {
        Connectivity::None
    }
}

/// The `PrimaryConnectionType` string, or the empty string when there is no primary connection.
pub fn primary_connection_type(snapshot: &Snapshot) -> &'static str {
    match primary(snapshot).map(|device| device.kind) {
        Some(DeviceKind::Ethernet) => "802-3-ethernet",
        Some(DeviceKind::Wifi) => "802-11-wireless",
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::model::Stage;

    fn device(ifindex: u32, kind: DeviceKind, state: DeviceState, has_router: bool) -> DeviceView {
        DeviceView {
            ifindex,
            name: format!("dev{ifindex}"),
            kind,
            state,
            ssid: None,
            failure: None,
            address: (state == DeviceState::Connected).then_some((Ipv4Addr::new(10, 0, 0, 9), 24)),
            has_router,
            metric: crate::model::route_metric(kind),
        }
    }

    fn snapshot(devices: Vec<DeviceView>) -> Snapshot {
        Snapshot {
            wireless_enabled: true,
            devices,
            networks: Vec::new(),
            known: Vec::new(),
        }
    }

    #[test]
    fn no_devices_or_none_ready_is_disconnected_with_no_connectivity() {
        let none = snapshot(vec![
            device(2, DeviceKind::Ethernet, DeviceState::Unavailable, false),
            device(3, DeviceKind::Wifi, DeviceState::Disconnected, false),
        ]);
        assert_eq!(
            (
                state(&none) as u32,
                connectivity(&none) as u32,
                primary_connection_type(&none)
            ),
            (20, 1, "")
        );
    }

    #[test]
    fn a_device_asking_for_an_address_is_connecting() {
        let connecting = snapshot(vec![device(
            3,
            DeviceKind::Wifi,
            DeviceState::Configuring(Stage::Addressing),
            false,
        )]);
        assert_eq!(
            (state(&connecting) as u32, connectivity(&connecting) as u32),
            (40, 1)
        );
    }

    #[test]
    fn a_lease_with_a_router_is_global_and_names_the_connection_type() {
        let global = snapshot(vec![device(
            3,
            DeviceKind::Wifi,
            DeviceState::Connected,
            true,
        )]);
        assert_eq!(
            (
                state(&global) as u32,
                connectivity(&global) as u32,
                primary_connection_type(&global)
            ),
            (70, 0, "802-11-wireless")
        );
    }

    #[test]
    fn a_lease_without_a_router_is_local_only() {
        let local = snapshot(vec![device(
            2,
            DeviceKind::Ethernet,
            DeviceState::Connected,
            false,
        )]);
        assert_eq!(
            (state(&local) as u32, primary_connection_type(&local)),
            (50, "")
        );
    }

    #[test]
    fn ethernet_is_primary_over_wifi() {
        let both = snapshot(vec![
            device(3, DeviceKind::Wifi, DeviceState::Connected, true),
            device(7, DeviceKind::Ethernet, DeviceState::Connected, true),
        ]);
        assert_eq!(primary_connection_type(&both), "802-3-ethernet");
    }
}
