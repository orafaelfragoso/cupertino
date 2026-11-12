//! What the daemon needs from the program that authenticates to Wi-Fi networks (decision D2: the
//! daemon does not implement WPA itself). [`crate::iwd`] is the implementation; tests use
//! [`fake::FakeSupplicant`].

use std::os::fd::BorrowedFd;

use cupertino_netlink::{Security, Ssid};

use crate::error::Result;

/// The kind of credentials a network needs, as the supplicant names it (iwd's `Network.Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkClass {
    /// No credentials (also opportunistic encryption, which needs none).
    Open,
    Wep,
    /// A passphrase: WPA, WPA2, WPA3 and the transition mode.
    Psk,
    /// 802.1X with a provisioned profile.
    Enterprise,
    /// An authentication the stack cannot start.
    Unsupported,
}

impl From<Security> for NetworkClass {
    fn from(security: Security) -> Self {
        match security {
            Security::Open | Security::Owe => Self::Open,
            Security::Wep => Self::Wep,
            Security::Wpa | Security::Wpa2 | Security::Wpa3 | Security::Wpa2Wpa3 => Self::Psk,
            Security::Enterprise => Self::Enterprise,
            Security::Unsupported => Self::Unsupported,
        }
    }
}

/// A network the supplicant has saved credentials for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownNetwork {
    pub ssid: Ssid,
    pub class: NetworkClass,
    /// Whether the supplicant joins the network by itself when it is in range.
    pub auto_connect: bool,
}

/// Why a connection attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectFailure {
    /// The attempt failed after a passphrase was supplied. The supplicant reports every failure of
    /// the handshake alike, so this is the likeliest cause and not a certain one.
    WrongPassphrase,
    /// The network is not in range.
    NotFound,
    /// Any other failure.
    Failed,
}

/// A change the supplicant reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupplicantEvent {
    /// Joining a network has begun (also when the supplicant picks a saved network itself).
    Connecting {
        ifindex: u32,
    },
    Connected {
        ifindex: u32,
        ssid: Ssid,
    },
    /// The association ended.
    Disconnected {
        ifindex: u32,
    },
    /// A connection attempt that this daemon asked for failed.
    Failed {
        ifindex: u32,
        reason: ConnectFailure,
    },
}

impl SupplicantEvent {
    pub fn ifindex(&self) -> u32 {
        match self {
            Self::Connecting { ifindex }
            | Self::Connected { ifindex, .. }
            | Self::Disconnected { ifindex }
            | Self::Failed { ifindex, .. } => *ifindex,
        }
    }
}

/// A network device, as the supplicant knows it by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interface<'a> {
    pub index: u32,
    pub name: &'a str,
}

/// The supplicant. Every call returns without waiting for a connection: the outcome arrives as a
/// [`SupplicantEvent`].
pub trait Supplicant {
    /// Powers the device on, so the supplicant scans and joins saved networks by itself.
    fn power_on(&mut self, interface: Interface<'_>) -> Result<()>;

    /// Asks for a scan. The results appear in the kernel's scan list when it ends.
    fn scan(&mut self, interface: Interface<'_>) -> Result<()>;

    /// The networks with saved credentials.
    fn known_networks(&mut self, interface: Interface<'_>) -> Result<Vec<KnownNetwork>>;

    /// Starts a connection. `passphrase` is for a [`NetworkClass::Psk`] network the supplicant has
    /// no credentials for; it goes to the supplicant only when the supplicant asks for it.
    fn connect(
        &mut self,
        interface: Interface<'_>,
        ssid: &Ssid,
        class: NetworkClass,
        passphrase: Option<&str>,
    ) -> Result<()>;

    fn disconnect(&mut self, interface: Interface<'_>) -> Result<()>;

    /// Takes the saved credentials of a network away, so the supplicant no longer joins it.
    fn forget(&mut self, ssid: &Ssid, class: NetworkClass) -> Result<()>;

    /// Lets the supplicant join a saved network by itself, or stops it.
    fn set_auto_connect(&mut self, ssid: &Ssid, class: NetworkClass, on: bool) -> Result<()>;

    /// The events and failures to watch since the last call, in order.
    fn events(&mut self) -> Vec<Result<SupplicantEvent>>;

    /// A descriptor that becomes readable when [`Supplicant::events`] has something.
    fn as_fd(&self) -> Option<BorrowedFd<'_>>;
}

#[cfg(test)]
pub mod fake {
    use super::*;

    /// One call the engine made, in order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Call {
        PowerOn(u32),
        Scan(u32),
        Connect {
            ifindex: u32,
            ssid: Ssid,
            class: NetworkClass,
            passphrase: Option<String>,
        },
        Disconnect(u32),
        Forget(Ssid, NetworkClass),
        AutoConnect(Ssid, NetworkClass, bool),
    }

    /// Records calls and returns the events a test queued.
    #[derive(Debug, Default)]
    pub struct FakeSupplicant {
        pub calls: Vec<Call>,
        pub known: Vec<KnownNetwork>,
        pub queued: Vec<SupplicantEvent>,
    }

    impl Supplicant for FakeSupplicant {
        fn power_on(&mut self, interface: Interface<'_>) -> Result<()> {
            self.calls.push(Call::PowerOn(interface.index));
            Ok(())
        }

        fn scan(&mut self, interface: Interface<'_>) -> Result<()> {
            self.calls.push(Call::Scan(interface.index));
            Ok(())
        }

        fn known_networks(&mut self, _interface: Interface<'_>) -> Result<Vec<KnownNetwork>> {
            Ok(self.known.clone())
        }

        fn connect(
            &mut self,
            interface: Interface<'_>,
            ssid: &Ssid,
            class: NetworkClass,
            passphrase: Option<&str>,
        ) -> Result<()> {
            self.calls.push(Call::Connect {
                ifindex: interface.index,
                ssid: ssid.clone(),
                class,
                passphrase: passphrase.map(str::to_owned),
            });
            Ok(())
        }

        fn disconnect(&mut self, interface: Interface<'_>) -> Result<()> {
            self.calls.push(Call::Disconnect(interface.index));
            Ok(())
        }

        fn forget(&mut self, ssid: &Ssid, class: NetworkClass) -> Result<()> {
            self.calls.push(Call::Forget(ssid.clone(), class));
            self.known
                .retain(|k| !(k.ssid == *ssid && k.class == class));
            Ok(())
        }

        fn set_auto_connect(&mut self, ssid: &Ssid, class: NetworkClass, on: bool) -> Result<()> {
            self.calls.push(Call::AutoConnect(ssid.clone(), class, on));
            for k in &mut self.known {
                if k.ssid == *ssid && k.class == class {
                    k.auto_connect = on;
                }
            }
            Ok(())
        }

        fn events(&mut self) -> Vec<Result<SupplicantEvent>> {
            std::mem::take(&mut self.queued)
                .into_iter()
                .map(Ok)
                .collect()
        }

        fn as_fd(&self) -> Option<BorrowedFd<'_>> {
            None
        }
    }
}
