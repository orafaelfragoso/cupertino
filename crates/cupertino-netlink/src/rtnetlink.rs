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
