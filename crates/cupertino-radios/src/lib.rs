//! The Wi-Fi and Bluetooth radios through whichever service owns them.
//!
//! The network is `org.cupertino.Network1` (`cupertino-netd`) when it is on the system bus, else NetworkManager; the
//! Bluetooth adapter is `org.cupertino.Bluetooth1` (`cupertino-btd`), else BlueZ. [`detect`] says which, the backends
//! it names read the same [`Wifi`], [`Link`] and [`Adapter`] from either and carry out the same changes, and
//! [`watch`] names the signals that tell a reading is out of date. Settings, the control panel and the menu bar all
//! go through here, so the three show the same state.
//!
//! Without the feature `dbus` the crate is the readings' types alone.

#[cfg(feature = "dbus")]
pub mod bluez;
#[cfg(feature = "dbus")]
pub mod btd;
#[cfg(feature = "dbus")]
mod bus;
// the stand-ins are test code: they live beside the tests and are built only for other crates' tests
#[cfg(feature = "fake")]
#[path = "../tests/fake/mod.rs"]
pub mod fake;
#[cfg(feature = "dbus")]
pub mod netd;
#[cfg(feature = "dbus")]
pub mod nm;
mod types;
#[cfg(feature = "dbus")]
pub mod watch;

pub use types::{
    device_kind, device_title, display_name, Adapter, BtDevice, Device, KnownNetwork, Link,
    LinkState, Network, Vpn, Wifi, MAX_NAME,
};

/// What serves the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkBackend {
    Cupertino,
    NetworkManager,
}

/// What serves the Bluetooth adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BluetoothBackend {
    Cupertino,
    BlueZ,
}

/// The services on the system bus that serve the radios; `None` where no service does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub network: Option<NetworkBackend>,
    pub bluetooth: Option<BluetoothBackend>,
}

#[cfg(feature = "dbus")]
mod services {
    use std::collections::HashSet;

    use zbus::blocking::proxy::Builder;
    use zbus::blocking::{Connection, Proxy};
    use zbus::proxy::CacheProperties;

    use super::bus::Bounded;
    use super::{
        bluez, btd, netd, nm, Adapter, BluetoothBackend, Found, Link, NetworkBackend, Wifi,
    };

    const BUS_DAEMON: &str = "org.freedesktop.DBus";

    /// Look up which services are on the system bus, running or activatable. `org.cupertino.Network1` comes before
    /// NetworkManager, whose name the network daemon also claims, and `org.cupertino.Bluetooth1` before BlueZ.
    pub fn detect(connection: &Connection) -> zbus::Result<Found> {
        let bus: Proxy<'_> = Builder::new(connection)
            .destination(BUS_DAEMON)?
            .path("/org/freedesktop/DBus")?
            .interface(BUS_DAEMON)?
            .cache_properties(CacheProperties::No)
            .build()?;
        let mut names: Vec<String> = bus.ask("ListNames", &())?;
        names.extend(bus.ask::<(), Vec<String>>("ListActivatableNames", &())?);
        let has = |name: &str| names.iter().any(|n| n.as_str() == name);
        Ok(Found {
            network: if has(netd::NAME) {
                Some(NetworkBackend::Cupertino)
            } else if has(nm::NAME) {
                Some(NetworkBackend::NetworkManager)
            } else {
                None
            },
            bluetooth: if has(btd::NAME) {
                Some(BluetoothBackend::Cupertino)
            } else if has(bluez::NAME) {
                Some(BluetoothBackend::BlueZ)
            } else {
                None
            },
        })
    }

    impl NetworkBackend {
        /// The whole reading: the radio, the devices, the networks in range and the saved ones.
        pub fn wifi(self, connection: &Connection) -> zbus::Result<Wifi> {
            match self {
                NetworkBackend::Cupertino => netd::wifi(connection),
                NetworkBackend::NetworkManager => nm::wifi(connection),
            }
        }

        /// The radio and its first Wi-Fi device, without the networks.
        pub fn link(self, connection: &Connection) -> zbus::Result<Link> {
            match self {
                NetworkBackend::Cupertino => netd::link(connection),
                NetworkBackend::NetworkManager => nm::link(connection),
            }
        }

        pub fn set_wireless_enabled(self, connection: &Connection, on: bool) -> zbus::Result<()> {
            match self {
                NetworkBackend::Cupertino => netd::set_wireless_enabled(connection, on),
                NetworkBackend::NetworkManager => nm::set_wireless_enabled(connection, on),
            }
        }

        /// Join the network of key `ssid` ([`crate::Network::ssid`]); an empty `passphrase` joins an open network
        /// or one whose credentials the service holds.
        pub fn join(
            self,
            connection: &Connection,
            ssid: &str,
            passphrase: &str,
        ) -> zbus::Result<()> {
            match self {
                NetworkBackend::Cupertino => netd::join(connection, ssid, passphrase),
                NetworkBackend::NetworkManager => nm::join(connection, ssid, passphrase),
            }
        }

        /// Leave the network, or give up joining one.
        pub fn disconnect(self, connection: &Connection) -> zbus::Result<()> {
            match self {
                NetworkBackend::Cupertino => netd::disconnect(connection),
                NetworkBackend::NetworkManager => nm::disconnect(connection),
            }
        }

        /// Ask for a scan; its networks come with a later reading.
        pub fn scan(self, connection: &Connection) -> zbus::Result<()> {
            match self {
                NetworkBackend::Cupertino => netd::scan(connection),
                NetworkBackend::NetworkManager => nm::scan(connection),
            }
        }
    }

    impl BluetoothBackend {
        /// The adapter and its devices; `pairing` are the BlueZ devices whose pairing the caller is waiting for (the
        /// Cupertino daemon reports its own).
        pub fn adapter(
            self,
            connection: &Connection,
            pairing: &HashSet<String>,
        ) -> zbus::Result<Adapter> {
            match self {
                BluetoothBackend::Cupertino => btd::adapter(connection),
                BluetoothBackend::BlueZ => bluez::read(connection, pairing),
            }
        }

        pub fn set_powered(self, connection: &Connection, on: bool) -> zbus::Result<()> {
            match self {
                BluetoothBackend::Cupertino => btd::set_powered(connection, on),
                BluetoothBackend::BlueZ => bluez::set_powered(connection, on),
            }
        }

        /// Connect the device `id` ([`crate::BtDevice::id`]). BlueZ answers once the link is up or failed, which
        /// takes seconds: call it from a thread that may wait.
        pub fn connect(self, connection: &Connection, id: &str) -> zbus::Result<()> {
            match self {
                BluetoothBackend::Cupertino => btd::connect(connection, id),
                BluetoothBackend::BlueZ => bluez::connect(connection, id),
            }
        }

        pub fn disconnect(self, connection: &Connection, id: &str) -> zbus::Result<()> {
            match self {
                BluetoothBackend::Cupertino => btd::disconnect(connection, id),
                BluetoothBackend::BlueZ => bluez::disconnect(connection, id),
            }
        }
    }

    /// The text a service sent with its refusal, else the error as the bus words it.
    pub fn refusal(error: &zbus::Error) -> String {
        match error {
            zbus::Error::MethodError(_, Some(text), _) if !text.is_empty() => text.clone(),
            zbus::Error::FDO(e) => zbus::DBusError::description(e.as_ref())
                .filter(|text| !text.is_empty())
                .map_or_else(|| e.to_string(), str::to_owned),
            other => other.to_string(),
        }
    }
}

#[cfg(feature = "dbus")]
pub use bus::{call_timeout, set_call_timeout, unanswered, CALL_TIMEOUT, LINK_TIMEOUT};
#[cfg(feature = "dbus")]
pub use services::{detect, refusal};
