//! What the bus shows: a copy of the model's state that the bus objects read without touching
//! the engine, and the compact device records they serve.

use std::net::Ipv4Addr;

use crate::model::{route_metric, DeviceKind, DeviceState, Failure, Model, Network};

/// One device as the bus shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceView {
    pub ifindex: u32,
    pub name: String,
    pub kind: DeviceKind,
    pub state: DeviceState,
    /// The network a Wi-Fi device is joining or has joined.
    pub ssid: Option<String>,
    pub failure: Option<Failure>,
    pub address: Option<(Ipv4Addr, u8)>,
    /// The device has a default router.
    pub has_router: bool,
    pub metric: u32,
}

/// The state of the network at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The user setting: the radio is not blocked by software.
    pub wireless_enabled: bool,
    pub devices: Vec<DeviceView>,
    /// The scan list of the Wi-Fi device.
    pub networks: Vec<Network>,
    /// The saved networks of the Wi-Fi device, in range or not, one per name: the name and whether the supplicant joins
    /// it by itself.
    pub known: Vec<(String, bool)>,
}

impl Snapshot {
    pub fn of(model: &Model) -> Self {
        let devices = model
            .devices()
            .map(|device| DeviceView {
                ifindex: device.ifindex,
                name: device.name.clone(),
                kind: device.kind,
                state: device.state,
                ssid: device.network.as_ref().map(|ssid| ssid.to_text()),
                failure: device.failure,
                address: device
                    .lease
                    .as_ref()
                    .map(|lease| (lease.address, lease.prefix_len)),
                has_router: device
                    .lease
                    .as_ref()
                    .is_some_and(|lease| !lease.routers.is_empty()),
                metric: route_metric(device.kind),
            })
            .collect();
        let networks = model
            .wifi_device()
            .map(|device| model.networks(device.ifindex))
            .unwrap_or_default();
        let mut known: Vec<(String, bool)> = Vec::new();
        for saved in model
            .wifi_device()
            .map(|device| device.known.as_slice())
            .unwrap_or_default()
        {
            let name = saved.ssid.to_text();
            match known.iter_mut().find(|(n, _)| *n == name) {
                Some((_, auto)) => *auto |= saved.auto_connect,
                None => known.push((name, saved.auto_connect)),
            }
        }
        known.sort();
        Self {
            wireless_enabled: model.wireless_enabled(),
            devices,
            networks,
            known,
        }
    }
}
