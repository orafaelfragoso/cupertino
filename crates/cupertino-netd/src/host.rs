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
    /// `None` when there is no rfkill device: nothing can switch the radio.
    rfkill: Option<Rfkill>,
}

impl SystemHost {
    /// Opens the sockets. `rfkill_path` is `/dev/rfkill` outside tests. A kernel or machine
    /// without nl80211 or rfkill is a machine without Wi-Fi, not an error; any other failure is.
    pub fn open(rfkill_path: &Path) -> Result<Self> {
        let wireless = match Nl80211::open() {
            Ok(wireless) => Some(wireless),
            Err(error) if is_unsupported(&error) => None,
            Err(error) => return Err(Error::Netlink(error)),
        };
        let rfkill = match Rfkill::open(rfkill_path) {
            Ok(rfkill) => Some(rfkill),
            Err(cupertino_netlink::Error::Device { source: Errno::NOENT, .. }) => None,
            Err(error) => return Err(Error::Netlink(error)),
        };
        Ok(Self { routes: RouteSocket::open()?, wireless, rfkill })
    }

    pub fn rfkill(&self) -> Option<&Rfkill> {
        self.rfkill.as_ref()
    }
}

impl Host for SystemHost {
    fn links(&mut self) -> Result<Vec<Link>> {
        Ok(self.routes.dump_links()?)
    }

    fn wireless_indexes(&mut self) -> Result<Vec<u32>> {
        match &mut self.wireless {
            Some(wireless) => Ok(wireless.interfaces()?.into_iter().map(|interface| interface.ifindex).collect()),
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
            Some(rfkill) => Ok(rfkill.set_soft_block(Kind::Wlan, blocked)?),
            None => Err(Error::Netlink(cupertino_netlink::Error::Device { path: cupertino_netlink::rfkill::DEVICE.into(), call: "open", source: Errno::NOENT })),
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
