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
