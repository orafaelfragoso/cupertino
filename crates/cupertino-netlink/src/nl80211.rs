//! Generic netlink and nl80211: wireless interfaces, the current connection, scan results and
//! the scan, connect and disconnect events.
//!
//! Numbers are those of `linux/genetlink.h` and `linux/nl80211.h`. Everything the kernel allows
//! unprivileged (dumps) works without capabilities; [`Nl80211::trigger_scan`] needs `CAP_NET_ADMIN`.

use std::os::fd::{AsFd, BorrowedFd};

use rustix::io::Errno;
use rustix::net::netlink::GENERIC;

use crate::attr::attrs;
use crate::error::{Error, ParseError, Result};
use crate::ie::{ssid_and_security, Security, Ssid};
use crate::mac::MacAddr;
use crate::message::{messages, Body, Message, NLM_F_DUMP};
use crate::socket::{Drained, Mode, Socket};

/// The generic netlink controller family (`GENL_ID_CTRL`).
const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const CTRL_ATTR_MCAST_GROUPS: u16 = 7;
const CTRL_ATTR_MCAST_GRP_NAME: u16 = 1;
const CTRL_ATTR_MCAST_GRP_ID: u16 = 2;

const FAMILY_NAME: &str = "nl80211";
/// Multicast group of scan start, results and abort events.
const GROUP_SCAN: &str = "scan";
/// Multicast group of connect and disconnect events.
const GROUP_MLME: &str = "mlme";

const CMD_GET_WIPHY: u8 = 1;
const CMD_GET_INTERFACE: u8 = 5;
const CMD_GET_STATION: u8 = 17;
const CMD_GET_SCAN: u8 = 32;
const CMD_TRIGGER_SCAN: u8 = 33;
const CMD_NEW_SCAN_RESULTS: u8 = 34;
const CMD_SCAN_ABORTED: u8 = 35;
const CMD_CONNECT: u8 = 46;
const CMD_DISCONNECT: u8 = 48;

const ATTR_WIPHY: u16 = 1;
const ATTR_WIPHY_NAME: u16 = 2;
const ATTR_IFINDEX: u16 = 3;
const ATTR_IFNAME: u16 = 4;
const ATTR_IFTYPE: u16 = 5;
const ATTR_MAC: u16 = 6;
const ATTR_STA_INFO: u16 = 21;
const ATTR_WIPHY_FREQ: u16 = 38;
const ATTR_SCAN_SSIDS: u16 = 45;
const ATTR_BSS: u16 = 47;
const ATTR_SSID: u16 = 52;
const ATTR_REASON_CODE: u16 = 54;
const ATTR_TIMED_OUT: u16 = 65;
const ATTR_DISCONNECTED_BY_AP: u16 = 71;
const ATTR_STATUS_CODE: u16 = 72;
const ATTR_SPLIT_WIPHY_DUMP: u16 = 174;

const STA_INFO_INACTIVE_TIME: u16 = 1;
const STA_INFO_SIGNAL: u16 = 7;
const STA_INFO_SIGNAL_AVG: u16 = 13;

const BSS_BSSID: u16 = 1;
const BSS_FREQUENCY: u16 = 2;
const BSS_CAPABILITY: u16 = 5;
const BSS_INFORMATION_ELEMENTS: u16 = 6;
const BSS_SIGNAL_MBM: u16 = 7;
const BSS_STATUS: u16 = 9;
const BSS_SEEN_MS_AGO: u16 = 10;
const BSS_BEACON_IES: u16 = 11;
/// `NL80211_BSS_STATUS_ASSOCIATED`.
const BSS_STATUS_ASSOCIATED: u32 = 1;

/// `NL80211_IFTYPE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceType {
    AdHoc,
    Station,
    AccessPoint,
    Monitor,
    P2pClient,
    P2pGroupOwner,
    /// A value this crate does not name.
    Other(u32),
}

impl InterfaceType {
    fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::AdHoc,
            2 => Self::Station,
            3 => Self::AccessPoint,
            6 => Self::Monitor,
            8 => Self::P2pClient,
            9 => Self::P2pGroupOwner,
            other => Self::Other(other),
        }
    }
}

/// A wireless network interface (`NL80211_CMD_NEW_INTERFACE` in a `GET_INTERFACE` dump).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WirelessInterface {
    pub ifindex: u32,
    pub name: String,
    /// The index of the PHY (`phy<index>` unless renamed).
    pub wiphy: u32,
    pub interface_type: InterfaceType,
    pub mac: Option<MacAddr>,
    /// The network the interface is connected to, if any.
    pub ssid: Option<Ssid>,
    /// The channel the interface is on, in MHz.
    pub frequency_mhz: Option<u32>,
}

/// A PHY and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wiphy {
    pub index: u32,
    pub name: String,
}

/// The access point a station interface is associated with (`NL80211_CMD_NEW_STATION`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Station {
    pub bssid: MacAddr,
    /// dBm, most recent frame.
    pub signal_dbm: Option<i8>,
    /// dBm, averaged by the driver.
    pub signal_average_dbm: Option<i8>,
    /// Milliseconds since the last frame from the station.
    pub inactive_ms: Option<u32>,
}

/// The current connection of an interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub ifindex: u32,
    pub ssid: Option<Ssid>,
    pub bssid: MacAddr,
    pub frequency_mhz: Option<u32>,
    pub signal_dbm: Option<i8>,
}

/// Joins an interface with the station dump of that interface: the connection, if there is one.
pub fn connection(interface: &WirelessInterface, stations: &[Station]) -> Option<Connection> {
    let station = stations.first()?;
    Some(Connection {
        ifindex: interface.ifindex,
        ssid: interface.ssid.clone(),
        bssid: station.bssid,
        frequency_mhz: interface.frequency_mhz,
        signal_dbm: station.signal_dbm,
    })
}

/// One access point from a scan (`NL80211_ATTR_BSS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    pub ifindex: u32,
    pub bssid: MacAddr,
    pub ssid: Ssid,
    pub frequency_mhz: u32,
    /// dBm. `None` when the driver reports an unspecified scale only.
    pub signal_dbm: Option<i32>,
    pub security: Security,
    /// The interface is associated with this access point.
    pub associated: bool,
    /// Milliseconds since the last beacon or probe response.
    pub seen_ms_ago: Option<u32>,
}

/// A scan dump: the access points that decoded and what went wrong with the others.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScanDump {
    pub results: Vec<ScanResult>,
    /// One entry per access point that did not decode; the others are in `results`.
    pub errors: Vec<ParseError>,
}

/// A wireless event of the scan and mlme multicast groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A scan started.
    ScanStarted {
        ifindex: u32,
    },
    /// A scan finished: [`Nl80211::scan_results`] has the new list.
    ScanResultsReady {
        ifindex: u32,
    },
    ScanAborted {
        ifindex: u32,
    },
    /// An association finished. `status_code` is the 802.11 status (0 is success); `None` when
    /// the attempt timed out.
    Connected {
        ifindex: u32,
        bssid: Option<MacAddr>,
        status_code: Option<u16>,
    },
    /// The association ended. `reason_code` is the 802.11 reason code.
    Disconnected {
        ifindex: u32,
        reason_code: u16,
        by_access_point: bool,
    },
}

/// Splits a generic netlink payload into the command and its attributes.
fn genl(payload: &[u8]) -> std::result::Result<(u8, &[u8]), ParseError> {
    match payload.split_first_chunk::<4>() {
        Some((header, rest)) => Ok((header[0], rest)),
        None => Err(ParseError::Truncated {
            what: "genlmsghdr",
            needed: 4,
            have: payload.len(),
        }),
    }
}

fn missing(message: &'static str, attribute: &'static str) -> ParseError {
    ParseError::MissingAttribute { message, attribute }
}

/// Decodes a `NEW_INTERFACE` message. `Ok(None)` for an interface that is not a network device
/// (a P2P device has a wdev but no ifindex).
pub fn parse_interface(
    payload: &[u8],
) -> std::result::Result<Option<WirelessInterface>, ParseError> {
    let (_, rest) = genl(payload)?;
    let (
        mut ifindex,
        mut name,
        mut wiphy,
        mut interface_type,
        mut mac,
        mut ssid,
        mut frequency_mhz,
    ) = (None, None, None, None, None, None, None);
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            ATTR_IFINDEX => ifindex = Some(attr.u32()?),
            ATTR_IFNAME => name = Some(attr.string()?.to_owned()),
            ATTR_WIPHY => wiphy = Some(attr.u32()?),
            ATTR_IFTYPE => interface_type = Some(InterfaceType::from_raw(attr.u32()?)),
            ATTR_MAC => mac = Some(MacAddr(attr.mac()?)),
            ATTR_SSID => ssid = Some(Ssid(attr.data.to_vec())),
            ATTR_WIPHY_FREQ => frequency_mhz = Some(attr.u32()?),
            _ => {}
        }
    }
    let Some(ifindex) = ifindex else {
        return Ok(None);
    };
    Ok(Some(WirelessInterface {
        ifindex,
        name: name.ok_or_else(|| missing("interface", "NL80211_ATTR_IFNAME"))?,
        wiphy: wiphy.ok_or_else(|| missing("interface", "NL80211_ATTR_WIPHY"))?,
        interface_type: interface_type
            .ok_or_else(|| missing("interface", "NL80211_ATTR_IFTYPE"))?,
        mac,
        ssid,
        frequency_mhz,
    }))
}

/// Decodes a `NEW_WIPHY` message. `Ok(None)` for a part of a split dump that carries no name.
pub fn parse_wiphy(payload: &[u8]) -> std::result::Result<Option<Wiphy>, ParseError> {
    let (_, rest) = genl(payload)?;
    let (mut index, mut name) = (None, None);
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            ATTR_WIPHY => index = Some(attr.u32()?),
            ATTR_WIPHY_NAME => name = Some(attr.string()?.to_owned()),
            _ => {}
        }
    }
    match (index, name) {
        (Some(index), Some(name)) => Ok(Some(Wiphy { index, name })),
        (Some(_), None) => Ok(None),
        (None, _) => Err(missing("wiphy", "NL80211_ATTR_WIPHY")),
    }
}

/// Decodes a `NEW_STATION` message.
pub fn parse_station(payload: &[u8]) -> std::result::Result<Station, ParseError> {
    let (_, rest) = genl(payload)?;
    let (mut bssid, mut info) = (None, None);
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            ATTR_MAC => bssid = Some(MacAddr(attr.mac()?)),
            ATTR_STA_INFO => info = Some(attr),
            _ => {}
        }
    }
    let (mut signal_dbm, mut signal_average_dbm, mut inactive_ms) = (None, None, None);
    for child in info
        .ok_or_else(|| missing("station", "NL80211_ATTR_STA_INFO"))?
        .nested()
    {
        let child = child?;
        match child.ty {
            STA_INFO_SIGNAL => signal_dbm = Some(child.i8()?),
            STA_INFO_SIGNAL_AVG => signal_average_dbm = Some(child.i8()?),
            STA_INFO_INACTIVE_TIME => inactive_ms = Some(child.u32()?),
            _ => {}
        }
    }
    Ok(Station {
        bssid: bssid.ok_or_else(|| missing("station", "NL80211_ATTR_MAC"))?,
        signal_dbm,
        signal_average_dbm,
        inactive_ms,
    })
}

/// Decodes a `NEW_SCAN_RESULTS` message of a `GET_SCAN` dump: one access point.
pub fn parse_scan_result(payload: &[u8]) -> std::result::Result<ScanResult, ParseError> {
    let (_, rest) = genl(payload)?;
    let (mut ifindex, mut bss) = (None, None);
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            ATTR_IFINDEX => ifindex = Some(attr.u32()?),
            ATTR_BSS => bss = Some(attr),
            _ => {}
        }
    }
    let (mut bssid, mut frequency_mhz, mut capability, mut elements, mut beacon_elements) =
        (None, None, 0u16, None, None);
    let (mut signal_dbm, mut associated, mut seen_ms_ago) = (None, false, None);
    for child in bss
        .ok_or_else(|| missing("scan result", "NL80211_ATTR_BSS"))?
        .nested()
    {
        let child = child?;
        match child.ty {
            BSS_BSSID => bssid = Some(MacAddr(child.mac()?)),
            BSS_FREQUENCY => frequency_mhz = Some(child.u32()?),
            BSS_CAPABILITY => capability = child.u16()?,
            BSS_INFORMATION_ELEMENTS => elements = Some(child.data),
            BSS_BEACON_IES => beacon_elements = Some(child.data),
            // The kernel reports 100 times the dBm value.
            BSS_SIGNAL_MBM => signal_dbm = Some(child.i32()? / 100),
            BSS_STATUS => associated = child.u32()? == BSS_STATUS_ASSOCIATED,
            BSS_SEEN_MS_AGO => seen_ms_ago = Some(child.u32()?),
            _ => {}
        }
    }
    // The elements of the last probe response, else those of the last beacon.
    let (ssid, security) = ssid_and_security(
        elements
            .or(beacon_elements)
            .ok_or_else(|| missing("scan result", "NL80211_BSS_INFORMATION_ELEMENTS"))?,
        capability,
    )?;
    Ok(ScanResult {
        ifindex: ifindex.ok_or_else(|| missing("scan result", "NL80211_ATTR_IFINDEX"))?,
        bssid: bssid.ok_or_else(|| missing("scan result", "NL80211_BSS_BSSID"))?,
        ssid,
        frequency_mhz: frequency_mhz
            .ok_or_else(|| missing("scan result", "NL80211_BSS_FREQUENCY"))?,
        signal_dbm,
        security,
        associated,
        seen_ms_ago,
    })
}

/// Decodes one multicast message of the family. `Ok(None)` for a command this crate does not report.
pub fn parse_event(payload: &[u8]) -> std::result::Result<Option<Event>, ParseError> {
    let (command, rest) = genl(payload)?;
    if !matches!(
        command,
        CMD_TRIGGER_SCAN | CMD_NEW_SCAN_RESULTS | CMD_SCAN_ABORTED | CMD_CONNECT | CMD_DISCONNECT
    ) {
        return Ok(None);
    }
    let (
        mut ifindex,
        mut bssid,
        mut status_code,
        mut reason_code,
        mut by_access_point,
        mut timed_out,
    ) = (None, None, None, None, false, false);
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            ATTR_IFINDEX => ifindex = Some(attr.u32()?),
            ATTR_MAC => bssid = Some(MacAddr(attr.mac()?)),
            ATTR_STATUS_CODE => status_code = Some(attr.u16()?),
            ATTR_REASON_CODE => reason_code = Some(attr.u16()?),
            ATTR_DISCONNECTED_BY_AP => by_access_point = true,
            ATTR_TIMED_OUT => timed_out = true,
            _ => {}
        }
    }
    let ifindex = ifindex.ok_or_else(|| missing("event", "NL80211_ATTR_IFINDEX"))?;
    Ok(Some(match command {
        CMD_TRIGGER_SCAN => Event::ScanStarted { ifindex },
        CMD_NEW_SCAN_RESULTS => Event::ScanResultsReady { ifindex },
        CMD_SCAN_ABORTED => Event::ScanAborted { ifindex },
        CMD_CONNECT => Event::Connected {
            ifindex,
            bssid,
            status_code: if timed_out {
                None
            } else {
                Some(
                    status_code
                        .ok_or_else(|| missing("connect event", "NL80211_ATTR_STATUS_CODE"))?,
                )
            },
        },
        // The kernel leaves the reason out when it is 0 (unspecified).
        _ => Event::Disconnected {
            ifindex,
            reason_code: reason_code.unwrap_or(0),
            by_access_point,
        },
    }))
}

/// A generic netlink family: its id and multicast groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub id: u16,
    /// `(name, id)` of each multicast group.
    pub groups: Vec<(String, u32)>,
}

/// Decodes the reply of `CTRL_CMD_GETFAMILY`.
pub fn parse_family(payload: &[u8]) -> std::result::Result<Family, ParseError> {
    let (_, rest) = genl(payload)?;
    let (mut id, mut groups) = (None, Vec::new());
    for attr in attrs(rest) {
        let attr = attr?;
        match attr.ty {
            CTRL_ATTR_FAMILY_ID => id = Some(attr.u16()?),
            CTRL_ATTR_MCAST_GROUPS => {
                // an array of nests, each `{ NAME, ID }`
                for entry in attr.nested() {
                    let (mut name, mut group_id) = (None, None);
                    for field in entry?.nested() {
                        let field = field?;
                        match field.ty {
                            CTRL_ATTR_MCAST_GRP_NAME => name = Some(field.string()?.to_owned()),
                            CTRL_ATTR_MCAST_GRP_ID => group_id = Some(field.u32()?),
                            _ => {}
                        }
                    }
                    groups.push((
                        name.ok_or_else(|| missing("multicast group", "CTRL_ATTR_MCAST_GRP_NAME"))?,
                        group_id
                            .ok_or_else(|| missing("multicast group", "CTRL_ATTR_MCAST_GRP_ID"))?,
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(Family {
        id: id.ok_or_else(|| missing("family", "CTRL_ATTR_FAMILY_ID"))?,
        groups,
    })
}

/// The body of a generic netlink request: the header (`cmd`, version 0) and nothing else yet.
fn command(cmd: u8) -> Body {
    let mut body = Body::new();
    body.header(&[cmd, 0, 0, 0]);
    body
}

/// Asks the controller for the `nl80211` family. The error is [`Errno::NOENT`] when the kernel has no
/// wireless support.
pub fn resolve_family(socket: &mut Socket) -> Result<Family> {
    parse_family_reply(&family_raw(socket)?)
}

/// The raw reply of `CTRL_CMD_GETFAMILY` for `nl80211`.
pub fn family_raw(socket: &mut Socket) -> Result<Vec<u8>> {
    let mut body = Body::new();
    body.header(&[CTRL_CMD_GETFAMILY, 1, 0, 0])
        .attr_cstr(CTRL_ATTR_FAMILY_NAME, FAMILY_NAME);
    socket.call("CTRL_CMD_GETFAMILY", GENL_ID_CTRL, 0, body.as_bytes())
}

/// Decodes the reply of [`family_raw`].
pub fn parse_family_reply(raw: &[u8]) -> Result<Family> {
    for message in messages(raw) {
        let message = message.map_err(Error::parse("CTRL_CMD_GETFAMILY"))?;
        if message.header.ty == GENL_ID_CTRL {
            return parse_family(message.payload).map_err(Error::parse("CTRL_CMD_GETFAMILY"));
        }
    }
    Err(Error::Parse {
        origin: "CTRL_CMD_GETFAMILY",
        source: missing("reply", "the family message"),
    })
}

/// The messages of `raw` that belong to `family`.
fn family_messages<'a>(
    raw: &'a [u8],
    family: u16,
    origin: &'static str,
) -> Result<Vec<Message<'a>>> {
    let mut found = Vec::new();
    for message in messages(raw) {
        let message = message.map_err(Error::parse(origin))?;
        if message.header.ty == family {
            found.push(message);
        }
    }
    Ok(found)
}

/// Decodes a `GET_INTERFACE` dump.
pub fn parse_interfaces(raw: &[u8], family: u16) -> Result<Vec<WirelessInterface>> {
    let origin = "NL80211_CMD_GET_INTERFACE";
    let mut interfaces = Vec::new();
    for message in family_messages(raw, family, origin)? {
        interfaces.extend(parse_interface(message.payload).map_err(Error::parse(origin))?);
    }
    Ok(interfaces)
}

/// Decodes a `GET_WIPHY` dump (with the split flag): the PHYs, once each (the kernel sends
/// several parts per PHY, each with its name).
pub fn parse_wiphys(raw: &[u8], family: u16) -> Result<Vec<Wiphy>> {
    let origin = "NL80211_CMD_GET_WIPHY";
    let mut wiphys: Vec<Wiphy> = Vec::new();
    for message in family_messages(raw, family, origin)? {
        if let Some(wiphy) = parse_wiphy(message.payload).map_err(Error::parse(origin))? {
            if !wiphys.iter().any(|known| known.index == wiphy.index) {
                wiphys.push(wiphy);
            }
        }
    }
    Ok(wiphys)
}

/// Decodes a `GET_STATION` dump.
pub fn parse_stations(raw: &[u8], family: u16) -> Result<Vec<Station>> {
    let origin = "NL80211_CMD_GET_STATION";
    family_messages(raw, family, origin)?
        .iter()
        .map(|message| parse_station(message.payload).map_err(Error::parse(origin)))
        .collect()
}

/// Decodes a `GET_SCAN` dump. An access point that does not decode is listed in `errors`; the
/// others are still returned.
pub fn parse_scan_dump(raw: &[u8], family: u16) -> Result<ScanDump> {
    let mut dump = ScanDump::default();
    for message in family_messages(raw, family, "NL80211_CMD_GET_SCAN")? {
        match parse_scan_result(message.payload) {
            Ok(result) => dump.results.push(result),
            Err(error) => dump.errors.push(error),
        }
    }
    Ok(dump)
}

/// A request socket for nl80211.
#[derive(Debug)]
pub struct Nl80211 {
    socket: Socket,
    family: Family,
}

impl Nl80211 {
    /// Opens the socket and resolves the family. `Err(Error::Kernel { errno: Errno::NOENT, .. })`
    /// means the kernel has no nl80211.
    pub fn open() -> Result<Self> {
        let mut socket = Socket::open(Some(GENERIC), 0, Mode::Blocking)?;
        let family = resolve_family(&mut socket)?;
        Ok(Self { socket, family })
    }

    pub fn family(&self) -> &Family {
        &self.family
    }

    fn dump_raw(
        &mut self,
        request: &'static str,
        cmd: u8,
        ifindex: Option<u32>,
    ) -> Result<Vec<u8>> {
        let mut body = command(cmd);
        if let Some(ifindex) = ifindex {
            body.attr_u32(ATTR_IFINDEX, ifindex);
        }
        self.socket
            .call(request, self.family.id, NLM_F_DUMP, body.as_bytes())
    }

    /// The raw reply of `NL80211_CMD_GET_INTERFACE`.
    pub fn interfaces_raw(&mut self) -> Result<Vec<u8>> {
        self.dump_raw("NL80211_CMD_GET_INTERFACE", CMD_GET_INTERFACE, None)
    }

    /// The raw reply of `NL80211_CMD_GET_WIPHY` in split mode.
    pub fn wiphys_raw(&mut self) -> Result<Vec<u8>> {
        let mut body = command(CMD_GET_WIPHY);
        body.attr(ATTR_SPLIT_WIPHY_DUMP, &[]);
        self.socket.call(
            "NL80211_CMD_GET_WIPHY",
            self.family.id,
            NLM_F_DUMP,
            body.as_bytes(),
        )
    }

    /// The raw reply of `NL80211_CMD_GET_STATION` for one interface.
    pub fn stations_raw(&mut self, ifindex: u32) -> Result<Vec<u8>> {
        self.dump_raw("NL80211_CMD_GET_STATION", CMD_GET_STATION, Some(ifindex))
    }

    /// The raw reply of `NL80211_CMD_GET_SCAN` for one interface.
    pub fn scan_results_raw(&mut self, ifindex: u32) -> Result<Vec<u8>> {
        self.dump_raw("NL80211_CMD_GET_SCAN", CMD_GET_SCAN, Some(ifindex))
    }

    pub fn interfaces(&mut self) -> Result<Vec<WirelessInterface>> {
        let raw = self.interfaces_raw()?;
        parse_interfaces(&raw, self.family.id)
    }

    pub fn wiphys(&mut self) -> Result<Vec<Wiphy>> {
        let raw = self.wiphys_raw()?;
        parse_wiphys(&raw, self.family.id)
    }

    pub fn stations(&mut self, ifindex: u32) -> Result<Vec<Station>> {
        let raw = self.stations_raw(ifindex)?;
        parse_stations(&raw, self.family.id)
    }

    /// The network the interface is connected to, or `None` when it is not connected.
    pub fn connection(&mut self, interface: &WirelessInterface) -> Result<Option<Connection>> {
        Ok(connection(interface, &self.stations(interface.ifindex)?))
    }

    /// The access points the kernel knows from earlier scans, with their last signal.
    pub fn scan_results(&mut self, ifindex: u32) -> Result<ScanDump> {
        let raw = self.scan_results_raw(ifindex)?;
        parse_scan_dump(&raw, self.family.id)
    }

    /// Starts a scan on all channels with a wildcard probe. Returns once the kernel accepted it;
    /// [`Event::ScanResultsReady`] says when it is done. `EBUSY` when a scan runs already, `EPERM`
    /// without `CAP_NET_ADMIN`.
    pub fn trigger_scan(&mut self, ifindex: u32) -> Result<()> {
        let mut body = command(CMD_TRIGGER_SCAN);
        body.attr_u32(ATTR_IFINDEX, ifindex)
            .nested(ATTR_SCAN_SSIDS, |ssids| {
                // one empty SSID: a wildcard probe request
                ssids.attr(1, &[]);
            });
        self.socket
            .call(
                "NL80211_CMD_TRIGGER_SCAN",
                self.family.id,
                0,
                body.as_bytes(),
            )
            .map(drop)
    }
}

/// What one [`Nl80211Monitor::drain`] read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Events {
    pub events: Vec<Event>,
    /// One entry per message that did not decode.
    pub errors: Vec<ParseError>,
    /// The kernel dropped events: read the interfaces and the connection again.
    pub overflowed: bool,
    /// Datagrams larger than the receive buffer, which are dropped: read again.
    pub truncated: usize,
}

/// The scan and mlme multicast groups of nl80211.
#[derive(Debug)]
pub struct Nl80211Monitor {
    socket: Socket,
    family: u16,
}

impl Nl80211Monitor {
    /// Opens a non-blocking socket (`AsFd`, register it with the event loop). A bind joins
    /// groups 1 to 32 only; a group id above that is [`Error::GroupOutOfRange`].
    pub fn open() -> Result<Self> {
        let mut resolver = Socket::open(Some(GENERIC), 0, Mode::Blocking)?;
        let family = resolve_family(&mut resolver)?;
        let mut mask = 0u32;
        for group in [GROUP_SCAN, GROUP_MLME] {
            let (name, id) = family.groups.iter().find(|(name, _)| name == group).ok_or(
                Error::MissingGroup {
                    family: FAMILY_NAME,
                    group,
                },
            )?;
            if !(1..=32).contains(id) {
                return Err(Error::GroupOutOfRange {
                    name: name.clone(),
                    id: *id,
                });
            }
            mask |= 1 << (id - 1);
        }
        Ok(Self {
            socket: Socket::open(Some(GENERIC), mask, Mode::NonBlocking)?,
            family: family.id,
        })
    }

    pub fn drain(&self) -> Result<Events> {
        let Drained {
            datagrams,
            overflowed,
            truncated,
        } = self.socket.drain()?;
        let mut events = Events {
            overflowed,
            truncated,
            ..Events::default()
        };
        for datagram in &datagrams {
            for message in messages(datagram) {
                match message.and_then(|message| {
                    if message.header.ty == self.family {
                        parse_event(message.payload)
                    } else {
                        Ok(None)
                    }
                }) {
                    Ok(Some(event)) => events.events.push(event),
                    Ok(None) => {}
                    Err(error) => events.errors.push(error),
                }
            }
        }
        Ok(events)
    }
}

impl AsFd for Nl80211Monitor {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

/// Whether `error` says the kernel has no nl80211 (the family is unknown).
pub fn is_unsupported(error: &Error) -> bool {
    matches!(error, Error::Kernel { errno, .. } if *errno == Errno::NOENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// genlmsghdr with `cmd`, then attributes.
    fn message(cmd: u8, build: impl FnOnce(&mut Body)) -> Vec<u8> {
        let mut body = command(cmd);
        build(&mut body);
        body.as_bytes().to_vec()
    }

    #[test]
    fn decodes_the_family_and_its_groups() {
        let payload = message(1, |body| {
            body.attr_cstr(CTRL_ATTR_FAMILY_NAME, "nl80211")
                .attr(CTRL_ATTR_FAMILY_ID, &27u16.to_ne_bytes())
                .nested(CTRL_ATTR_MCAST_GROUPS, |groups| {
                    groups.nested(1, |group| {
                        group
                            .attr_cstr(CTRL_ATTR_MCAST_GRP_NAME, "scan")
                            .attr_u32(CTRL_ATTR_MCAST_GRP_ID, 5);
                    });
                    groups.nested(2, |group| {
                        group
                            .attr_cstr(CTRL_ATTR_MCAST_GRP_NAME, "mlme")
                            .attr_u32(CTRL_ATTR_MCAST_GRP_ID, 7);
                    });
                });
        });
        assert_eq!(
            parse_family(&payload),
            Ok(Family {
                id: 27,
                groups: vec![("scan".to_owned(), 5), ("mlme".to_owned(), 7)]
            })
        );
    }

    #[test]
    fn a_scan_started_connect_and_disconnect_event_decode() {
        let started = message(CMD_TRIGGER_SCAN, |body| {
            body.attr_u32(ATTR_IFINDEX, 3);
        });
        assert_eq!(
            parse_event(&started),
            Ok(Some(Event::ScanStarted { ifindex: 3 }))
        );
        let connected = message(CMD_CONNECT, |body| {
            body.attr_u32(ATTR_IFINDEX, 3)
                .attr(ATTR_MAC, &[2, 0, 0, 0, 0, 9])
                .attr(ATTR_STATUS_CODE, &0u16.to_ne_bytes());
        });
        assert_eq!(
            parse_event(&connected),
            Ok(Some(Event::Connected {
                ifindex: 3,
                bssid: Some(MacAddr([2, 0, 0, 0, 0, 9])),
                status_code: Some(0)
            }))
        );
        let disconnected = message(CMD_DISCONNECT, |body| {
            body.attr_u32(ATTR_IFINDEX, 3)
                .attr(ATTR_REASON_CODE, &3u16.to_ne_bytes())
                .attr(ATTR_DISCONNECTED_BY_AP, &[]);
        });
        assert_eq!(
            parse_event(&disconnected),
            Ok(Some(Event::Disconnected {
                ifindex: 3,
                reason_code: 3,
                by_access_point: true
            }))
        );
    }

    #[test]
    fn a_connect_that_timed_out_has_no_status_code() {
        let timed_out = message(CMD_CONNECT, |body| {
            body.attr_u32(ATTR_IFINDEX, 3).attr(ATTR_TIMED_OUT, &[]);
        });
        assert_eq!(
            parse_event(&timed_out),
            Ok(Some(Event::Connected {
                ifindex: 3,
                bssid: None,
                status_code: None
            }))
        );
    }

    #[test]
    fn a_connect_without_status_or_timeout_is_an_error() {
        let broken = message(CMD_CONNECT, |body| {
            body.attr_u32(ATTR_IFINDEX, 3);
        });
        assert_eq!(
            parse_event(&broken),
            Err(ParseError::MissingAttribute {
                message: "connect event",
                attribute: "NL80211_ATTR_STATUS_CODE"
            })
        );
    }

    #[test]
    fn other_commands_are_not_reported() {
        assert_eq!(parse_event(&message(19, |_| {})), Ok(None));
    }

    #[test]
    fn a_scan_result_decodes_signal_and_security() {
        // information elements: SSID "lab", RSN with one AKM suite, 00-0F-AC:8 (SAE)
        let elements = [
            &[0u8, 3, b'l', b'a', b'b'][..],
            &[
                48, 20, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f,
                0xac, 8, 0, 0,
            ],
        ]
        .concat();
        let payload = message(CMD_NEW_SCAN_RESULTS, |body| {
            body.attr_u32(ATTR_IFINDEX, 3).nested(ATTR_BSS, |bss| {
                bss.attr(BSS_BSSID, &[2, 0, 0, 0, 0, 7])
                    .attr_u32(BSS_FREQUENCY, 5180)
                    .attr(BSS_CAPABILITY, &0x0411u16.to_ne_bytes())
                    .attr(BSS_SIGNAL_MBM, &(-6150i32).to_ne_bytes())
                    .attr_u32(BSS_STATUS, BSS_STATUS_ASSOCIATED)
                    .attr_u32(BSS_SEEN_MS_AGO, 120)
                    .attr(BSS_INFORMATION_ELEMENTS, &elements);
            });
        });
        let result = parse_scan_result(&payload).unwrap();
        assert_eq!(
            result,
            ScanResult {
                ifindex: 3,
                bssid: MacAddr([2, 0, 0, 0, 0, 7]),
                ssid: Ssid(b"lab".to_vec()),
                frequency_mhz: 5180,
                signal_dbm: Some(-61),
                security: Security::Wpa3,
                associated: true,
                seen_ms_ago: Some(120),
            }
        );
    }

    #[test]
    fn beacon_elements_are_used_when_there_is_no_probe_response() {
        let payload = message(CMD_NEW_SCAN_RESULTS, |body| {
            body.attr_u32(ATTR_IFINDEX, 3).nested(ATTR_BSS, |bss| {
                bss.attr(BSS_BSSID, &[2, 0, 0, 0, 0, 7])
                    .attr_u32(BSS_FREQUENCY, 2412)
                    .attr(BSS_BEACON_IES, &[0, 2, b'h', b'i']);
            });
        });
        let result = parse_scan_result(&payload).unwrap();
        assert_eq!(
            (result.ssid, result.security, result.signal_dbm),
            (Ssid(b"hi".to_vec()), Security::Open, None)
        );
    }

    #[test]
    fn a_bad_access_point_does_not_cost_the_good_ones() {
        let good = message(CMD_NEW_SCAN_RESULTS, |body| {
            body.attr_u32(ATTR_IFINDEX, 3).nested(ATTR_BSS, |bss| {
                bss.attr(BSS_BSSID, &[2, 0, 0, 0, 0, 7])
                    .attr_u32(BSS_FREQUENCY, 2412)
                    .attr(BSS_INFORMATION_ELEMENTS, &[0, 2, b'h', b'i']);
            });
        });
        let bad = message(CMD_NEW_SCAN_RESULTS, |body| {
            body.attr_u32(ATTR_IFINDEX, 3).nested(ATTR_BSS, |bss| {
                bss.attr(BSS_BSSID, &[2, 0, 0, 0, 0, 8]);
            });
        });
        let mut raw = crate::message::frame(27, 2, 1, &good);
        raw.extend(crate::message::frame(27, 2, 1, &bad));
        raw.extend(crate::message::frame(
            crate::message::NLMSG_DONE,
            2,
            1,
            &0i32.to_ne_bytes(),
        ));
        let dump = parse_scan_dump(&raw, 27).unwrap();
        assert_eq!(dump.results.len(), 1);
        assert_eq!(
            dump.errors,
            [ParseError::MissingAttribute {
                message: "scan result",
                attribute: "NL80211_BSS_INFORMATION_ELEMENTS"
            }]
        );
    }

    #[test]
    fn a_wdev_only_interface_is_left_out() {
        let payload = message(7, |body| {
            body.attr_u32(ATTR_WIPHY, 0).attr_u32(ATTR_IFTYPE, 10);
        });
        assert_eq!(parse_interface(&payload), Ok(None));
    }

    #[test]
    fn the_connection_joins_the_interface_and_the_station() {
        let interface = WirelessInterface {
            ifindex: 3,
            name: "wlan0".to_owned(),
            wiphy: 0,
            interface_type: InterfaceType::Station,
            mac: None,
            ssid: Some(Ssid(b"lab".to_vec())),
            frequency_mhz: Some(5220),
        };
        let station = Station {
            bssid: MacAddr([2, 0, 0, 0, 0, 7]),
            signal_dbm: Some(-48),
            signal_average_dbm: None,
            inactive_ms: None,
        };
        assert_eq!(
            connection(&interface, &[station]),
            Some(Connection {
                ifindex: 3,
                ssid: Some(Ssid(b"lab".to_vec())),
                bssid: MacAddr([2, 0, 0, 0, 0, 7]),
                frequency_mhz: Some(5220),
                signal_dbm: Some(-48)
            })
        );
        assert_eq!(connection(&interface, &[]), None);
    }

    #[test]
    fn a_split_wiphy_part_without_a_name_is_left_out() {
        let first = message(3, |body| {
            body.attr_u32(ATTR_WIPHY, 0)
                .attr_cstr(ATTR_WIPHY_NAME, "phy0");
        });
        let later = message(3, |body| {
            body.attr_u32(ATTR_WIPHY, 0);
        });
        assert_eq!(
            parse_wiphy(&first),
            Ok(Some(Wiphy {
                index: 0,
                name: "phy0".to_owned()
            }))
        );
        assert_eq!(parse_wiphy(&later), Ok(None));
    }
}
