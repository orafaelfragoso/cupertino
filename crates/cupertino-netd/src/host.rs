//! The real [`Host`]: rtnetlink, nl80211, `/dev/rfkill` and packet sockets.

use std::path::Path;

use cupertino_netlink::nl80211::{is_unsupported, Nl80211, ScanDump};
use cupertino_netlink::rfkill::{Kind, Rfkill};
use cupertino_netlink::rtnetlink::{AddressChange, Link, RouteChange, RouteSocket};
use rustix::io::Errno;
use rustix::rand::{getrandom, GetRandomFlags};

use crate::dhcp::transport::{PacketTransport, Transport};
use crate::engine::Host;
use crate::error::{Error, Result};

/// The machine the daemon runs on.
pub struct SystemHost {
    routes: RouteSocket,
    /// `None` on a kernel without nl80211: there is no Wi-Fi to manage.
    wireless: Option<Nl80211>,
    /// The errno that left the radio without a switch (no device, or one this user may not open).
    rfkill: std::result::Result<Rfkill, Errno>,
}

impl SystemHost {
    /// Opens the sockets. `rfkill_path` is `/dev/rfkill` outside tests. A kernel or machine
    /// without nl80211 or rfkill is a machine without Wi-Fi, not an error. With `tolerate_denied` (the unprivileged
    /// session daemon) so is an rfkill device this user may not open: it is reported and leaves the radio switch
    /// unavailable. The system daemon fails on it, so a unit without access to the device is not left half working. Any
    /// other failure is an error.
    pub fn open(rfkill_path: &Path, tolerate_denied: bool) -> Result<Self> {
        let wireless = match Nl80211::open() {
            Ok(wireless) => Some(wireless),
            Err(error) if is_unsupported(&error) => None,
            Err(error) => return Err(Error::Netlink(error)),
        };
        let rfkill = match Rfkill::open(rfkill_path) {
            Ok(rfkill) => Ok(rfkill),
            Err(cupertino_netlink::Error::Device {
                source: Errno::NOENT,
                ..
            }) => Err(Errno::NOENT),
            Err(
                error @ cupertino_netlink::Error::Device {
                    source: Errno::ACCESS,
                    ..
                },
            ) if tolerate_denied => {
                eprintln!("cupertino-netd: {error}; the radio switch is unavailable");
                Err(Errno::ACCESS)
            }
            Err(error) => return Err(Error::Netlink(error)),
        };
        Ok(Self {
            routes: RouteSocket::open()?,
            wireless,
            rfkill,
        })
    }

    pub fn rfkill(&self) -> Option<&Rfkill> {
        self.rfkill.as_ref().ok()
    }
}

impl Host for SystemHost {
    fn links(&mut self) -> Result<Vec<Link>> {
        Ok(self.routes.dump_links()?)
    }

    fn wireless_indexes(&mut self) -> Result<Vec<u32>> {
        match &mut self.wireless {
            Some(wireless) => Ok(wireless
                .interfaces()?
                .into_iter()
                .map(|interface| interface.ifindex)
                .collect()),
            None => Ok(Vec::new()),
        }
    }

    fn scan_results(&mut self, ifindex: u32) -> Result<ScanDump> {
        match &mut self.wireless {
            Some(wireless) => Ok(wireless.scan_results(ifindex)?),
            None => Ok(ScanDump::default()),
        }
    }

    fn set_link_up(&mut self, ifindex: u32, up: bool) -> Result<()> {
        Ok(self.routes.set_link_up(ifindex, up)?)
    }

    fn add_address(&mut self, change: &AddressChange) -> Result<()> {
        Ok(self.routes.add_address(change)?)
    }

    fn remove_address(&mut self, change: &AddressChange) -> Result<()> {
        Ok(self.routes.remove_address(change)?)
    }

    fn add_route(&mut self, change: &RouteChange) -> Result<()> {
        Ok(self.routes.add_route(change)?)
    }

    fn remove_route(&mut self, change: &RouteChange) -> Result<()> {
        Ok(self.routes.remove_route(change)?)
    }

    fn set_wireless_blocked(&mut self, blocked: bool) -> Result<()> {
        match &self.rfkill {
            Ok(rfkill) => Ok(rfkill.set_soft_block(Kind::Wlan, blocked)?),
            Err(source) => Err(Error::Netlink(cupertino_netlink::Error::Device {
                path: cupertino_netlink::rfkill::DEVICE.into(),
                call: "open",
                source: *source,
            })),
        }
    }

    fn open_transport(&mut self, ifindex: u32) -> Result<Box<dyn Transport>> {
        Ok(Box::new(PacketTransport::open(ifindex)?))
    }

    fn random_u32(&mut self) -> Result<u32> {
        let mut bytes = [0u8; 4];
        getrandom(&mut bytes, GetRandomFlags::empty()).map_err(Error::syscall("getrandom"))?;
        Ok(u32::from_ne_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A device file this user may not open, or `None` when this user is root and may open anything.
    fn denied_device() -> Option<std::path::PathBuf> {
        let path =
            std::env::temp_dir().join(format!("cupertino-netd-rfkill-{}", std::process::id()));
        std::fs::write(&path, []).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_ok() {
            std::fs::remove_file(&path).unwrap();
            eprintln!("skipping: root may open any file");
            return None;
        }
        Some(path)
    }

    #[test]
    fn a_session_daemon_serves_without_an_rfkill_device_it_may_not_open_and_says_why_the_switch_fails(
    ) {
        let Some(path) = denied_device() else { return };
        let host = SystemHost::open(&path, true);
        std::fs::remove_file(&path).unwrap();
        let mut host = host.unwrap();
        assert!(host.rfkill().is_none());
        assert!(matches!(
            host.set_wireless_blocked(true),
            Err(Error::Netlink(cupertino_netlink::Error::Device {
                source: Errno::ACCESS,
                ..
            }))
        ));
    }

    #[test]
    fn the_system_daemon_fails_on_an_rfkill_device_it_may_not_open() {
        let Some(path) = denied_device() else { return };
        let host = SystemHost::open(&path, false);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            host,
            Err(Error::Netlink(cupertino_netlink::Error::Device {
                source: Errno::ACCESS,
                ..
            }))
        ));
    }
}
