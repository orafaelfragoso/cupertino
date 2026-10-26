//! rtnetlink (`NETLINK_ROUTE`): dump and monitor links, addresses and routes, and change them.
//!
//! Numbers are those of `linux/rtnetlink.h`, `linux/if_link.h` and `linux/if_addr.h`. Dumps ask for
//! `AF_UNSPEC`; messages of families other than IPv4 and IPv6 (multicast routing, MPLS) are not
//! ours and are left out of address and route dumps.

use std::net::{IpAddr, Ipv4Addr};

use crate::attr::attrs;
use crate::error::{Error, ParseError, Result};
use crate::mac::MacAddr;
use crate::message::{messages, Body, Message, NLM_F_CREATE, NLM_F_DUMP, NLM_F_REPLACE};
use crate::socket::{Drained, Mode, Socket};

pub const RTM_NEWLINK: u16 = 16;
pub const RTM_DELLINK: u16 = 17;
pub const RTM_GETLINK: u16 = 18;
pub const RTM_NEWADDR: u16 = 20;
pub const RTM_DELADDR: u16 = 21;
pub const RTM_GETADDR: u16 = 22;
pub const RTM_NEWROUTE: u16 = 24;
pub const RTM_DELROUTE: u16 = 25;
pub const RTM_GETROUTE: u16 = 26;

const AF_UNSPEC: u8 = 0;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;

/// `ARPHRD_ETHER` and `ARPHRD_LOOPBACK` of `linux/if_arp.h`.
const ARPHRD_ETHER: u16 = 1;
const ARPHRD_LOOPBACK: u16 = 772;

const IFF_UP: u32 = 0x1;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
const IFF_LOWER_UP: u32 = 0x1_0000;

const IFLA_ADDRESS: u16 = 1;
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_OPERSTATE: u16 = 16;
const IFLA_LINKINFO: u16 = 18;
const IFLA_INFO_KIND: u16 = 1;
const IFLA_MASTER: u16 = 10;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;
const IFA_BROADCAST: u16 = 4;
const IFA_FLAGS: u16 = 8;

const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PRIORITY: u16 = 6;
const RTA_PREFSRC: u16 = 7;
const RTA_TABLE: u16 = 15;

/// `RTMGRP_LINK`, `RTMGRP_IPV4_IFADDR`, `RTMGRP_IPV4_ROUTE`, `RTMGRP_IPV6_IFADDR`, `RTMGRP_IPV6_ROUTE`.
const MONITOR_GROUPS: u32 = 0x1 | 0x10 | 0x40 | 0x100 | 0x400;

/// `RT_TABLE_MAIN`: where default routes live.
pub const TABLE_MAIN: u32 = 254;
/// `RTN_UNICAST`.
const ROUTE_UNICAST: u8 = 1;
/// `RTPROT_STATIC`: a route set by an administrator.
pub const PROTOCOL_STATIC: u8 = 4;
/// `RTPROT_DHCP`: a route learned from a DHCP client.
pub const PROTOCOL_DHCP: u8 = 16;

/// IPv4 or IPv6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            AF_INET => Some(Self::V4),
            AF_INET6 => Some(Self::V6),
            _ => None,
        }
    }

    fn to_raw(self) -> u8 {
        match self {
            Self::V4 => AF_INET,
            Self::V6 => AF_INET6,
        }
    }
}

/// The `ifi_flags` of a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkFlags(pub u32);

impl LinkFlags {
    /// `IFF_UP`: the administrator set the link up.
    pub fn is_up(self) -> bool {
        self.0 & IFF_UP != 0
    }

    /// `IFF_RUNNING`: the driver reports the operational state as up or unknown.
    pub fn is_running(self) -> bool {
        self.0 & IFF_RUNNING != 0
    }

    /// `IFF_LOWER_UP`: there is a carrier.
    pub fn has_carrier(self) -> bool {
        self.0 & IFF_LOWER_UP != 0
    }

    pub fn is_loopback(self) -> bool {
        self.0 & IFF_LOOPBACK != 0
    }
}

/// RFC 2863 operational state (`IF_OPER_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperState {
    Unknown,
    NotPresent,
    Down,
    LowerLayerDown,
    Testing,
    Dormant,
    Up,
    /// A value this crate does not know.
    Other(u8),
}

impl OperState {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Unknown,
            1 => Self::NotPresent,
            2 => Self::Down,
            3 => Self::LowerLayerDown,
            4 => Self::Testing,
            5 => Self::Dormant,
            6 => Self::Up,
            other => Self::Other(other),
        }
    }
}

/// What a link is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkClass {
    Loopback,
    /// An Ethernet device with no software driver on top: a NIC.
    Wired,
    /// A device nl80211 manages.
    Wireless,
    /// A bridge, veth, tunnel or any other link a driver creates in software.
    Virtual,
}

/// One network device (`RTM_NEWLINK`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub index: u32,
    pub name: String,
    /// `ARPHRD_*`.
    pub hardware_type: u16,
    pub flags: LinkFlags,
    pub oper_state: OperState,
    /// Present for devices with a 6-byte hardware address.
    pub mac: Option<MacAddr>,
    pub mtu: Option<u32>,
    /// `IFLA_INFO_KIND` (`veth`, `bridge`, ...): set for links a driver creates in software.
    pub info_kind: Option<String>,
    /// The bridge or bond this link is a port of.
    pub master: Option<u32>,
}

impl Link {
    /// Classifies the link. rtnetlink does not know which links are wireless: `wireless_indexes`
    /// are the interface indexes nl80211 lists.
    pub fn class(&self, wireless_indexes: &[u32]) -> LinkClass {
        if self.hardware_type == ARPHRD_LOOPBACK || self.flags.is_loopback() {
            LinkClass::Loopback
        } else if wireless_indexes.contains(&self.index) {
            LinkClass::Wireless
        } else if self.info_kind.is_some() || self.hardware_type != ARPHRD_ETHER {
            LinkClass::Virtual
        } else {
            LinkClass::Wired
        }
    }
}

/// `RT_SCOPE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Universe,
    Site,
    Link,
    Host,
    Other(u8),
}

impl Scope {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Universe,
            200 => Self::Site,
            253 => Self::Link,
            254 => Self::Host,
            other => Self::Other(other),
        }
    }

    fn to_raw(self) -> u8 {
        match self {
            Self::Universe => 0,
            Self::Site => 200,
            Self::Link => 253,
            Self::Host => 254,
            Self::Other(raw) => raw,
        }
    }
}

/// One address of a link (`RTM_NEWADDR`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub index: u32,
    pub family: Family,
    pub prefix_len: u8,
    pub scope: Scope,
    /// `IFA_F_*`. `IFA_FLAGS` when the kernel sends it, else the 8-bit `ifa_flags`.
    pub flags: u32,
    /// The interface's own address (`IFA_LOCAL` for IPv4, where `IFA_ADDRESS` is the peer of a
    /// point-to-point link).
    pub address: IpAddr,
    pub label: Option<String>,
}

impl Address {
    /// `IFA_F_TENTATIVE`: duplicate address detection has not finished.
    pub fn is_tentative(&self) -> bool {
        self.flags & 0x40 != 0
    }
}

/// One route (`RTM_NEWROUTE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub family: Family,
    /// `None` is the default destination, with `dst_len` 0.
    pub destination: Option<IpAddr>,
    pub dst_len: u8,
    pub gateway: Option<IpAddr>,
    /// The output interface.
    pub index: Option<u32>,
    pub metric: Option<u32>,
    pub table: u32,
    /// `RTPROT_*`.
    pub protocol: u8,
    pub scope: Scope,
    /// `RTN_*`.
    pub route_type: u8,
    pub preferred_source: Option<IpAddr>,
}

impl Route {
    /// A unicast route to everything, in the main table.
    pub fn is_default(&self) -> bool {
        self.dst_len == 0 && self.route_type == ROUTE_UNICAST && self.table == TABLE_MAIN
    }
}

/// A change in the network state, from the kernel's multicast groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteEvent {
    LinkNew(Link),
    LinkDeleted(Link),
    AddressNew(Address),
    AddressDeleted(Address),
    RouteNew(Route),
    RouteDeleted(Route),
}

fn missing(message: &'static str, attribute: &'static str) -> ParseError {
    ParseError::MissingAttribute { message, attribute }
}

fn family_header<'a, const N: usize>(
    payload: &'a [u8],
    what: &'static str,
) -> std::result::Result<([u8; N], &'a [u8]), ParseError> {
    match payload.split_first_chunk::<N>() {
        Some((header, rest)) => Ok((*header, rest)),
        None => Err(ParseError::Truncated {
            what,
            needed: N,
            have: payload.len(),
        }),
    }
}

/// Decodes the payload of an `RTM_NEWLINK` or `RTM_DELLINK`: an `ifinfomsg` and attributes.
pub fn parse_link(payload: &[u8]) -> std::result::Result<Link, ParseError> {
    let (header, rest) = family_header::<16>(payload, "ifinfomsg")?;
    let hardware_type = u16::from_ne_bytes([header[2], header[3]]);
    let index = u32::from_ne_bytes([header[4], header[5], header[6], header[7]]);
    let flags = LinkFlags(u32::from_ne_bytes([
        header[8], header[9], header[10], header[11],
    ]));
    let (mut name, mut oper_state, mut mac, mut mtu, mut info_kind, mut master) =
        (None, OperState::Unknown, None, None, None, None);
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            IFLA_IFNAME => name = Some(attr.string()?.to_owned()),
            IFLA_OPERSTATE => oper_state = OperState::from_raw(attr.u8()?),
            IFLA_ADDRESS if attr.data.len() == 6 => mac = Some(MacAddr(attr.mac()?)),
            IFLA_MTU => mtu = Some(attr.u32()?),
            IFLA_MASTER => master = Some(attr.u32()?),
            IFLA_LINKINFO => {
                for child in attr.nested() {
                    let child = child?;
                    if child.ty == IFLA_INFO_KIND {
                        info_kind = Some(child.string()?.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(Link {
        index,
        name: name.ok_or_else(|| missing("link", "IFLA_IFNAME"))?,
        hardware_type,
        flags,
        oper_state,
        mac,
        mtu,
        info_kind,
        master,
    })
}

/// Decodes the payload of an `RTM_NEWADDR` or `RTM_DELADDR`: an `ifaddrmsg` and attributes.
/// `Ok(None)` for a family that is not IPv4 or IPv6.
pub fn parse_address(payload: &[u8]) -> std::result::Result<Option<Address>, ParseError> {
    let (header, rest) = family_header::<8>(payload, "ifaddrmsg")?;
    let Some(family) = Family::from_raw(header[0]) else {
        return Ok(None);
    };
    let (prefix_len, short_flags, scope) = (header[1], header[2], Scope::from_raw(header[3]));
    let index = u32::from_ne_bytes([header[4], header[5], header[6], header[7]]);
    let (mut address, mut local, mut label, mut flags) = (None, None, None, u32::from(short_flags));
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            IFA_ADDRESS => address = Some(attr.ip()?),
            IFA_LOCAL => local = Some(attr.ip()?),
            IFA_LABEL => label = Some(attr.string()?.to_owned()),
            IFA_FLAGS => flags = attr.u32()?,
            _ => {}
        }
    }
    let address = local
        .or(address)
        .ok_or_else(|| missing("address", "IFA_ADDRESS"))?;
    Ok(Some(Address {
        index,
        family,
        prefix_len,
        scope,
        flags,
        address,
        label,
    }))
}

/// Decodes the payload of an `RTM_NEWROUTE` or `RTM_DELROUTE`: an `rtmsg` and attributes.
/// `Ok(None)` for a family that is not IPv4 or IPv6.
pub fn parse_route(payload: &[u8]) -> std::result::Result<Option<Route>, ParseError> {
    let (header, rest) = family_header::<12>(payload, "rtmsg")?;
    let Some(family) = Family::from_raw(header[0]) else {
        return Ok(None);
    };
    let dst_len = header[1];
    let (header_table, protocol, scope, route_type) =
        (header[4], header[5], Scope::from_raw(header[6]), header[7]);
    let (mut destination, mut gateway, mut index, mut metric, mut preferred_source) =
        (None, None, None, None, None);
    let mut table = u32::from(header_table);
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            RTA_DST => destination = Some(attr.ip()?),
            RTA_GATEWAY => gateway = Some(attr.ip()?),
            RTA_OIF => index = Some(attr.u32()?),
            RTA_PRIORITY => metric = Some(attr.u32()?),
            RTA_PREFSRC => preferred_source = Some(attr.ip()?),
            // The 8-bit header field cannot hold tables above 255; this attribute carries the real one.
            RTA_TABLE => table = attr.u32()?,
            _ => {}
        }
    }
    Ok(Some(Route {
        family,
        destination,
        dst_len,
        gateway,
        index,
        metric,
        table,
        protocol,
        scope,
        route_type,
        preferred_source,
    }))
}

/// The messages of `raw` with type `ty`; `NLMSG_DONE` and acknowledgements are skipped.
fn payloads_of<'a>(raw: &'a [u8], ty: u16, origin: &'static str) -> Result<Vec<Message<'a>>> {
    let mut found = Vec::new();
    for message in messages(raw) {
        let message = message.map_err(Error::parse(origin))?;
        if message.header.ty == ty {
            found.push(message);
        }
    }
    Ok(found)
}

/// Decodes a `RTM_GETLINK` dump.
pub fn parse_links(raw: &[u8]) -> Result<Vec<Link>> {
    let origin = "RTM_GETLINK";
    payloads_of(raw, RTM_NEWLINK, origin)?
        .iter()
        .map(|message| parse_link(message.payload).map_err(Error::parse(origin)))
        .collect()
}

/// Decodes a `RTM_GETADDR` dump.
pub fn parse_addresses(raw: &[u8]) -> Result<Vec<Address>> {
    let origin = "RTM_GETADDR";
    let mut addresses = Vec::new();
    for message in payloads_of(raw, RTM_NEWADDR, origin)? {
        addresses.extend(parse_address(message.payload).map_err(Error::parse(origin))?);
    }
    Ok(addresses)
}

/// Decodes a `RTM_GETROUTE` dump.
pub fn parse_routes(raw: &[u8]) -> Result<Vec<Route>> {
    let origin = "RTM_GETROUTE";
    let mut routes = Vec::new();
    for message in payloads_of(raw, RTM_NEWROUTE, origin)? {
        routes.extend(parse_route(message.payload).map_err(Error::parse(origin))?);
    }
    Ok(routes)
}

/// Decodes one message of the multicast groups. `Ok(None)` for a message that is not about links,
/// addresses or routes of IPv4 or IPv6.
pub fn parse_event(message: &Message<'_>) -> std::result::Result<Option<RouteEvent>, ParseError> {
    Ok(match message.header.ty {
        RTM_NEWLINK => Some(RouteEvent::LinkNew(parse_link(message.payload)?)),
        RTM_DELLINK => Some(RouteEvent::LinkDeleted(parse_link(message.payload)?)),
        RTM_NEWADDR => parse_address(message.payload)?.map(RouteEvent::AddressNew),
        RTM_DELADDR => parse_address(message.payload)?.map(RouteEvent::AddressDeleted),
        RTM_NEWROUTE => parse_route(message.payload)?.map(RouteEvent::RouteNew),
        RTM_DELROUTE => parse_route(message.payload)?.map(RouteEvent::RouteDeleted),
        _ => None,
    })
}

/// An address to add to or remove from a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressChange {
    pub index: u32,
    pub address: IpAddr,
    pub prefix_len: u8,
}

/// A route to add or remove in the main table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteChange {
    pub family: Family,
    /// `None` is the default destination.
    pub destination: Option<(IpAddr, u8)>,
    pub gateway: Option<IpAddr>,
    pub index: u32,
    pub metric: Option<u32>,
    /// `RTPROT_*`, for example [`PROTOCOL_DHCP`].
    pub protocol: u8,
}

fn ip_bytes(address: IpAddr) -> Vec<u8> {
    match address {
        IpAddr::V4(address) => address.octets().to_vec(),
        IpAddr::V6(address) => address.octets().to_vec(),
    }
}

/// The body of `RTM_NEWLINK` that sets `IFF_UP` or clears it, touching no other flag
/// (`ifi_change` is the mask of flags to apply).
fn link_state_body(index: u32, up: bool) -> Body {
    let mut header = [0u8; 16];
    header[4..8].copy_from_slice(&index.to_ne_bytes());
    header[8..12].copy_from_slice(&(if up { IFF_UP } else { 0 }).to_ne_bytes());
    header[12..16].copy_from_slice(&IFF_UP.to_ne_bytes());
    let mut body = Body::new();
    body.header(&header);
    body
}

/// The body of `RTM_NEWADDR` and `RTM_DELADDR`: both name the address the same way.
fn address_body(change: &AddressChange) -> Body {
    let family = match change.address {
        IpAddr::V4(_) => Family::V4,
        IpAddr::V6(_) => Family::V6,
    };
    let mut header = [0u8; 8];
    header[0] = family.to_raw();
    header[1] = change.prefix_len;
    header[3] = Scope::Universe.to_raw();
    header[4..8].copy_from_slice(&change.index.to_ne_bytes());
    let mut body = Body::new();
    body.header(&header);
    let bytes = ip_bytes(change.address);
    body.attr(IFA_LOCAL, &bytes).attr(IFA_ADDRESS, &bytes);
    // A /31 or /32 has no broadcast address.
    if let IpAddr::V4(address) = change.address {
        if change.prefix_len <= 30 {
            let host_bits = u32::MAX
                .checked_shr(u32::from(change.prefix_len))
                .unwrap_or(0);
            body.attr(
                IFA_BROADCAST,
                &Ipv4Addr::from(u32::from(address) | host_bits).octets(),
            );
        }
    }
    body
}

/// The body of `RTM_NEWROUTE` and `RTM_DELROUTE`.
fn route_body(change: &RouteChange) -> Body {
    let mut header = [0u8; 12];
    header[0] = change.family.to_raw();
    header[1] = change.destination.map_or(0, |(_, length)| length);
    header[4] = TABLE_MAIN as u8;
    header[5] = change.protocol;
    header[6] = if change.gateway.is_some() {
        Scope::Universe
    } else {
        Scope::Link
    }
    .to_raw();
    header[7] = ROUTE_UNICAST;
    let mut body = Body::new();
    body.header(&header);
    if let Some((destination, _)) = change.destination {
        body.attr(RTA_DST, &ip_bytes(destination));
    }
    if let Some(gateway) = change.gateway {
        body.attr(RTA_GATEWAY, &ip_bytes(gateway));
    }
    body.attr_u32(RTA_OIF, change.index);
    if let Some(metric) = change.metric {
        body.attr_u32(RTA_PRIORITY, metric);
    }
    body
}

/// A request socket for rtnetlink.
#[derive(Debug)]
pub struct RouteSocket {
    socket: Socket,
}

impl RouteSocket {
    pub fn open() -> Result<Self> {
        Ok(Self {
            socket: Socket::open(None, 0, Mode::Blocking)?,
        })
    }

    /// The raw reply of `RTM_GETLINK`, as the kernel wrote it. [`parse_links`] decodes it.
    pub fn dump_links_raw(&mut self) -> Result<Vec<u8>> {
        let mut body = Body::new();
        body.header(&[AF_UNSPEC; 16]);
        self.socket
            .call("RTM_GETLINK", RTM_GETLINK, NLM_F_DUMP, body.as_bytes())
    }

    /// The raw reply of `RTM_GETADDR`. [`parse_addresses`] decodes it.
    pub fn dump_addresses_raw(&mut self) -> Result<Vec<u8>> {
        let mut body = Body::new();
        body.header(&[AF_UNSPEC; 8]);
        self.socket
            .call("RTM_GETADDR", RTM_GETADDR, NLM_F_DUMP, body.as_bytes())
    }

    /// The raw reply of `RTM_GETROUTE`. [`parse_routes`] decodes it.
    pub fn dump_routes_raw(&mut self) -> Result<Vec<u8>> {
        let mut body = Body::new();
        body.header(&[AF_UNSPEC; 12]);
        self.socket
            .call("RTM_GETROUTE", RTM_GETROUTE, NLM_F_DUMP, body.as_bytes())
    }

    pub fn dump_links(&mut self) -> Result<Vec<Link>> {
        parse_links(&self.dump_links_raw()?)
    }

    pub fn dump_addresses(&mut self) -> Result<Vec<Address>> {
        parse_addresses(&self.dump_addresses_raw()?)
    }

    pub fn dump_routes(&mut self) -> Result<Vec<Route>> {
        parse_routes(&self.dump_routes_raw()?)
    }

    /// Sets or clears `IFF_UP`. Needs `CAP_NET_ADMIN`.
    pub fn set_link_up(&mut self, index: u32, up: bool) -> Result<()> {
        self.socket
            .call(
                "RTM_NEWLINK",
                RTM_NEWLINK,
                0,
                link_state_body(index, up).as_bytes(),
            )
            .map(drop)
    }

    /// Adds an address, or updates it when it exists. Needs `CAP_NET_ADMIN`.
    pub fn add_address(&mut self, change: &AddressChange) -> Result<()> {
        self.socket
            .call(
                "RTM_NEWADDR",
                RTM_NEWADDR,
                NLM_F_CREATE | NLM_F_REPLACE,
                address_body(change).as_bytes(),
            )
            .map(drop)
    }

    /// Removes an address; `ENOENT` when it is not there. Needs `CAP_NET_ADMIN`.
    pub fn remove_address(&mut self, change: &AddressChange) -> Result<()> {
        self.socket
            .call(
                "RTM_DELADDR",
                RTM_DELADDR,
                0,
                address_body(change).as_bytes(),
            )
            .map(drop)
    }

    /// Adds a route, or replaces the one with the same destination and metric. Needs `CAP_NET_ADMIN`.
    pub fn add_route(&mut self, change: &RouteChange) -> Result<()> {
        self.socket
            .call(
                "RTM_NEWROUTE",
                RTM_NEWROUTE,
                NLM_F_CREATE | NLM_F_REPLACE,
                route_body(change).as_bytes(),
            )
            .map(drop)
    }

    /// Removes a route; `ESRCH` when it is not there. Needs `CAP_NET_ADMIN`.
    pub fn remove_route(&mut self, change: &RouteChange) -> Result<()> {
        self.socket
            .call(
                "RTM_DELROUTE",
                RTM_DELROUTE,
                0,
                route_body(change).as_bytes(),
            )
            .map(drop)
    }
}

/// What one [`RouteMonitor::drain`] read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RouteEvents {
    pub events: Vec<RouteEvent>,
    /// One entry per message that did not decode.
    pub errors: Vec<ParseError>,
    /// The kernel dropped events: dump links, addresses and routes again.
    pub overflowed: bool,
    /// Datagrams larger than the receive buffer, which are dropped: dump again.
    pub truncated: usize,
}

/// The kernel's rtnetlink multicast groups for links, addresses and routes of both families.
#[derive(Debug)]
pub struct RouteMonitor {
    socket: Socket,
}

impl RouteMonitor {
    /// Opens a non-blocking socket (`AsFd`, register it with the event loop).
    pub fn open() -> Result<Self> {
        Ok(Self {
            socket: Socket::open(None, MONITOR_GROUPS, Mode::NonBlocking)?,
        })
    }

    pub fn drain(&self) -> Result<RouteEvents> {
        let Drained {
            datagrams,
            overflowed,
            truncated,
        } = self.socket.drain()?;
        let mut events = RouteEvents {
            overflowed,
            truncated,
            ..RouteEvents::default()
        };
        for datagram in &datagrams {
            for message in messages(datagram) {
                match message.and_then(|message| parse_event(&message)) {
                    Ok(Some(event)) => events.events.push(event),
                    Ok(None) => {}
                    Err(error) => events.errors.push(error),
                }
            }
        }
        Ok(events)
    }
}

impl std::os::fd::AsFd for RouteMonitor {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::frame;

    /// Link 2 `eth0`, Ethernet, up with a carrier, 1500 MTU, MAC 02:00:00:00:00:01, oper state up.
    fn eth0_payload() -> Vec<u8> {
        let mut header = [0u8; 16];
        header[2..4].copy_from_slice(&1u16.to_ne_bytes());
        header[4..8].copy_from_slice(&2u32.to_ne_bytes());
        header[8..12].copy_from_slice(&(IFF_UP | IFF_RUNNING | IFF_LOWER_UP | 0x2).to_ne_bytes());
        let mut body = Body::new();
        body.header(&header)
            .attr_cstr(IFLA_IFNAME, "eth0")
            .attr_u32(IFLA_MTU, 1500)
            .attr(IFLA_OPERSTATE, &[6])
            .attr(IFLA_ADDRESS, &[2, 0, 0, 0, 0, 1]);
        body.as_bytes().to_vec()
    }

    #[test]
    fn decodes_a_link() {
        let link = parse_link(&eth0_payload()).unwrap();
        assert_eq!(link.index, 2);
        assert_eq!(link.name, "eth0");
        assert_eq!(link.mtu, Some(1500));
        assert_eq!(link.oper_state, OperState::Up);
        assert_eq!(link.mac, Some(MacAddr([2, 0, 0, 0, 0, 1])));
        assert!(link.flags.is_up() && link.flags.has_carrier() && link.flags.is_running());
        assert_eq!(link.class(&[]), LinkClass::Wired);
        assert_eq!(link.class(&[2]), LinkClass::Wireless);
    }

    #[test]
    fn a_link_with_a_driver_kind_is_virtual() {
        let mut header = [0u8; 16];
        header[2..4].copy_from_slice(&1u16.to_ne_bytes());
        header[4..8].copy_from_slice(&9u32.to_ne_bytes());
        let mut body = Body::new();
        body.header(&header)
            .attr_cstr(IFLA_IFNAME, "veth0")
            .nested(IFLA_LINKINFO, |info| {
                info.attr_cstr(IFLA_INFO_KIND, "veth");
            });
        let link = parse_link(body.as_bytes()).unwrap();
        assert_eq!(link.info_kind.as_deref(), Some("veth"));
        assert_eq!(link.class(&[]), LinkClass::Virtual);
    }

    #[test]
    fn a_link_without_a_name_is_an_error() {
        let mut body = Body::new();
        body.header(&[0; 16]);
        assert_eq!(
            parse_link(body.as_bytes()),
            Err(ParseError::MissingAttribute {
                message: "link",
                attribute: "IFLA_IFNAME"
            })
        );
    }

    #[test]
    fn decodes_a_v4_address_preferring_the_local_attribute() {
        let mut header = [0u8; 8];
        header[0] = AF_INET;
        header[1] = 24;
        header[2] = 0x80;
        header[4..8].copy_from_slice(&2u32.to_ne_bytes());
        let mut body = Body::new();
        body.header(&header)
            .attr(IFA_ADDRESS, &[10, 0, 0, 1])
            .attr(IFA_LOCAL, &[10, 0, 0, 9])
            .attr_cstr(IFA_LABEL, "eth0");
        let address = parse_address(body.as_bytes()).unwrap().unwrap();
        assert_eq!(address.address, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)));
        assert_eq!(
            (address.prefix_len, address.flags, address.index),
            (24, 0x80, 2)
        );
        assert_eq!(address.label.as_deref(), Some("eth0"));
    }

    #[test]
    fn the_flags_attribute_replaces_the_short_flags() {
        let mut header = [0u8; 8];
        header[0] = AF_INET6;
        header[1] = 64;
        header[2] = 0x80;
        let mut body = Body::new();
        body.header(&header)
            .attr(
                IFA_ADDRESS,
                &[0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            )
            .attr_u32(IFA_FLAGS, 0x1_0080 | 0x40);
        let address = parse_address(body.as_bytes()).unwrap().unwrap();
        assert!(address.is_tentative());
        assert_eq!(address.flags, 0x1_00c0);
    }

    #[test]
    fn other_address_families_are_left_out() {
        let mut body = Body::new();
        body.header(&[28, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(parse_address(body.as_bytes()), Ok(None));
        let mut body = Body::new();
        body.header(&[128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(parse_route(body.as_bytes()), Ok(None));
    }

    #[test]
    fn decodes_a_default_route_with_a_table_attribute() {
        let mut header = [0u8; 12];
        header[0] = AF_INET;
        header[4] = 252; // the 8-bit field is overridden by RTA_TABLE
        header[5] = PROTOCOL_DHCP;
        header[7] = ROUTE_UNICAST;
        let mut body = Body::new();
        body.header(&header)
            .attr(RTA_GATEWAY, &[10, 0, 0, 1])
            .attr_u32(RTA_OIF, 2)
            .attr_u32(RTA_PRIORITY, 600)
            .attr_u32(RTA_TABLE, TABLE_MAIN);
        let route = parse_route(body.as_bytes()).unwrap().unwrap();
        assert!(route.is_default());
        assert_eq!(route.gateway, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert_eq!(
            (route.index, route.metric, route.protocol),
            (Some(2), Some(600), PROTOCOL_DHCP)
        );
    }

    #[test]
    fn a_route_in_the_local_table_is_not_default() {
        let mut header = [0u8; 12];
        header[0] = AF_INET;
        header[4] = 255;
        header[7] = ROUTE_UNICAST;
        let mut body = Body::new();
        body.header(&header);
        assert!(!parse_route(body.as_bytes()).unwrap().unwrap().is_default());
    }

    #[test]
    fn a_dump_reply_decodes_links_and_ignores_done() {
        let mut raw = frame(RTM_NEWLINK, 2, 1, &eth0_payload());
        raw.extend(frame(crate::message::NLMSG_DONE, 2, 1, &0i32.to_ne_bytes()));
        let links = parse_links(&raw).unwrap();
        assert_eq!(
            links
                .iter()
                .map(|link| link.name.as_str())
                .collect::<Vec<_>>(),
            ["eth0"]
        );
    }

    #[test]
    fn an_event_carries_its_direction() {
        let raw = frame(RTM_DELLINK, 0, 0, &eth0_payload());
        let message = messages(&raw).next().unwrap().unwrap();
        assert!(
            matches!(parse_event(&message), Ok(Some(RouteEvent::LinkDeleted(link))) if link.index == 2)
        );
    }

    #[test]
    fn link_up_touches_only_the_up_flag() {
        let body = link_state_body(5, true);
        let mut expected = [0u8; 16];
        expected[4..8].copy_from_slice(&5u32.to_ne_bytes());
        expected[8..12].copy_from_slice(&1u32.to_ne_bytes());
        expected[12..16].copy_from_slice(&1u32.to_ne_bytes());
        assert_eq!(body.as_bytes(), expected);
        assert_eq!(
            &link_state_body(5, false).as_bytes()[8..16],
            [0, 0, 0, 0, 1, 0, 0, 0]
        );
    }

    #[test]
    fn a_v4_address_request_names_the_address_and_its_broadcast() {
        let body = address_body(&AddressChange {
            index: 3,
            address: IpAddr::V4(Ipv4Addr::new(192, 168, 7, 20)),
            prefix_len: 24,
        });
        let mut expected = Body::new();
        expected
            .header(&[AF_INET, 24, 0, 0, 3, 0, 0, 0])
            .attr(IFA_LOCAL, &[192, 168, 7, 20])
            .attr(IFA_ADDRESS, &[192, 168, 7, 20])
            .attr(IFA_BROADCAST, &[192, 168, 7, 255]);
        assert_eq!(body, expected);
    }

    #[test]
    fn a_host_address_has_no_broadcast() {
        let body = address_body(&AddressChange {
            index: 3,
            address: IpAddr::V4(Ipv4Addr::new(192, 168, 7, 20)),
            prefix_len: 32,
        });
        assert_eq!(
            attrs(&body.as_bytes()[8..])
                .map(|attr| attr.unwrap().ty)
                .collect::<Vec<_>>(),
            [IFA_LOCAL, IFA_ADDRESS]
        );
    }

    #[test]
    fn a_default_route_request_has_a_gateway_and_no_destination() {
        let body = route_body(&RouteChange {
            family: Family::V4,
            destination: None,
            gateway: Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
            index: 2,
            metric: Some(600),
            protocol: PROTOCOL_DHCP,
        });
        let mut expected = Body::new();
        expected
            .header(&[
                AF_INET,
                0,
                0,
                0,
                254,
                PROTOCOL_DHCP,
                0,
                ROUTE_UNICAST,
                0,
                0,
                0,
                0,
            ])
            .attr(RTA_GATEWAY, &[10, 0, 0, 1])
            .attr_u32(RTA_OIF, 2)
            .attr_u32(RTA_PRIORITY, 600);
        assert_eq!(body, expected);
    }
}
