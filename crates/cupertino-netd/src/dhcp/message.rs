//! The DHCPv4 message of RFC 2131 (figure 1) and the options of RFC 2132.
//!
//! ```text
//! op htype hlen hops | xid | secs flags | ciaddr | yiaddr | siaddr | giaddr | chaddr[16]
//! | sname[64] | file[128] | magic cookie 99.130.83.99 | options ... 255
//! ```
//!
//! Option 52 (overload of `sname` and `file`) is not supported: a message that uses it is an error.

use std::fmt;
use std::net::Ipv4Addr;

use cupertino_netlink::MacAddr;

pub const SERVER_PORT: u16 = 67;
pub const CLIENT_PORT: u16 = 68;

/// `op`: BOOTREQUEST and BOOTREPLY (RFC 2131, 2).
const OP_REQUEST: u8 = 1;
const OP_REPLY: u8 = 2;
/// `htype` 1 and `hlen` 6: Ethernet (RFC 1700 hardware types).
const HARDWARE_ETHERNET: u8 = 1;
const HARDWARE_LENGTH: u8 = 6;
const FIXED_LEN: usize = 236;
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
/// The broadcast flag in `flags` (RFC 2131, 2): ask the server to broadcast its replies, as a client
/// that cannot receive unicast before it is configured must.
const FLAG_BROADCAST: u16 = 0x8000;

const OPTION_PAD: u8 = 0;
const OPTION_SUBNET_MASK: u8 = 1;
const OPTION_ROUTER: u8 = 3;
const OPTION_DNS_SERVER: u8 = 6;
const OPTION_DOMAIN_NAME: u8 = 15;
const OPTION_REQUESTED_ADDRESS: u8 = 50;
const OPTION_LEASE_TIME: u8 = 51;
const OPTION_OVERLOAD: u8 = 52;
const OPTION_MESSAGE_TYPE: u8 = 53;
const OPTION_SERVER_ID: u8 = 54;
const OPTION_PARAMETER_REQUEST_LIST: u8 = 55;
const OPTION_MESSAGE: u8 = 56;
const OPTION_RENEWAL_TIME: u8 = 58;
const OPTION_REBINDING_TIME: u8 = 59;
const OPTION_CLIENT_ID: u8 = 61;
const OPTION_END: u8 = 255;

/// Option 53 values (RFC 2132, 9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Discover,
    Offer,
    Request,
    Decline,
    Ack,
    Nak,
    Release,
    Inform,
}

impl MessageType {
    fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            1 => Self::Discover,
            2 => Self::Offer,
            3 => Self::Request,
            4 => Self::Decline,
            5 => Self::Ack,
            6 => Self::Nak,
            7 => Self::Release,
            8 => Self::Inform,
            _ => return None,
        })
    }

    fn to_raw(self) -> u8 {
        match self {
            Self::Discover => 1,
            Self::Offer => 2,
            Self::Request => 3,
            Self::Decline => 4,
            Self::Ack => 5,
            Self::Nak => 6,
            Self::Release => 7,
            Self::Inform => 8,
        }
    }

    /// Messages a server sends carry `op` BOOTREPLY.
    fn is_reply(self) -> bool {
        matches!(self, Self::Offer | Self::Ack | Self::Nak)
    }
}

/// The options this client reads or writes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    pub subnet_mask: Option<Ipv4Addr>,
    pub routers: Vec<Ipv4Addr>,
    pub dns_servers: Vec<Ipv4Addr>,
    pub domain_name: Option<String>,
    pub requested_address: Option<Ipv4Addr>,
    /// Seconds; `u32::MAX` is infinite (RFC 2131, 3.3).
    pub lease_time: Option<u32>,
    pub server_id: Option<Ipv4Addr>,
    /// The option numbers a client asks for.
    pub parameter_request_list: Vec<u8>,
    /// Text of a NAK.
    pub message: Option<String>,
    /// T1, seconds.
    pub renewal_time: Option<u32>,
    /// T2, seconds.
    pub rebinding_time: Option<u32>,
    pub client_id: Option<Vec<u8>>,
}

/// One DHCP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub message_type: MessageType,
    pub xid: u32,
    /// Seconds since the client began acquiring or renewing.
    pub seconds: u16,
    pub broadcast: bool,
    pub client_address: Ipv4Addr,
    pub your_address: Ipv4Addr,
    pub server_address: Ipv4Addr,
    pub gateway_address: Ipv4Addr,
    pub client_hardware: MacAddr,
    pub options: Options,
}

/// Why bytes are not a DHCP message this client can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TooShort { length: usize },
    BadMagicCookie,
    BadOp(u8),
    /// `htype`/`hlen` that are not Ethernet.
    BadHardware { hardware_type: u8, length: u8 },
    MissingMessageType,
    BadMessageType(u8),
    /// The option runs past the end of the message.
    TruncatedOption { code: u8 },
    /// The option's length does not fit its type.
    OptionLength { code: u8, length: usize },
    OptionNotUtf8 { code: u8 },
    /// Option 52.
    Overload,
    /// `op` and the message type disagree (a server message sent as a request).
    OpMismatch,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { length } => write!(f, "{length} bytes is shorter than a DHCP message"),
            Self::BadMagicCookie => write!(f, "no DHCP magic cookie"),
            Self::BadOp(op) => write!(f, "op {op} is neither a request nor a reply"),
            Self::BadHardware { hardware_type, length } => write!(f, "hardware type {hardware_type} of length {length} is not Ethernet"),
            Self::MissingMessageType => write!(f, "no message type option"),
            Self::BadMessageType(raw) => write!(f, "message type {raw} is unknown"),
            Self::TruncatedOption { code } => write!(f, "option {code} runs past the end"),
            Self::OptionLength { code, length } => write!(f, "option {code} cannot have {length} bytes"),
            Self::OptionNotUtf8 { code } => write!(f, "option {code} is not UTF-8"),
            Self::Overload => write!(f, "option overload is not supported"),
            Self::OpMismatch => write!(f, "op does not match the message type"),
        }
    }
}

impl std::error::Error for DecodeError {}

fn put_option(out: &mut Vec<u8>, code: u8, data: &[u8]) {
    out.push(code);
    out.push(u8::try_from(data.len()).expect("an option is at most 255 bytes"));
    out.extend_from_slice(data);
}

fn put_addresses(out: &mut Vec<u8>, code: u8, addresses: &[Ipv4Addr]) {
    if !addresses.is_empty() {
        put_option(out, code, &addresses.iter().flat_map(|address| address.octets()).collect::<Vec<u8>>());
    }
}

impl Message {
    /// A request from `client_hardware` with every address zero and no options but the type.
    pub fn new(message_type: MessageType, xid: u32, client_hardware: MacAddr) -> Self {
        Self {
            message_type,
            xid,
            seconds: 0,
            broadcast: false,
            client_address: Ipv4Addr::UNSPECIFIED,
            your_address: Ipv4Addr::UNSPECIFIED,
            server_address: Ipv4Addr::UNSPECIFIED,
            gateway_address: Ipv4Addr::UNSPECIFIED,
            client_hardware,
            options: Options::default(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FIXED_LEN + 64);
        out.extend_from_slice(&[if self.message_type.is_reply() { OP_REPLY } else { OP_REQUEST }, HARDWARE_ETHERNET, HARDWARE_LENGTH, 0]);
        out.extend_from_slice(&self.xid.to_be_bytes());
        out.extend_from_slice(&self.seconds.to_be_bytes());
        out.extend_from_slice(&(if self.broadcast { FLAG_BROADCAST } else { 0 }).to_be_bytes());
        for address in [self.client_address, self.your_address, self.server_address, self.gateway_address] {
            out.extend_from_slice(&address.octets());
        }
        out.extend_from_slice(&self.client_hardware.0);
        // chaddr is 16 bytes, sname 64, file 128: all zero after the hardware address
        out.resize(FIXED_LEN, 0);
        out.extend_from_slice(&MAGIC_COOKIE);

        let options = &self.options;
        put_option(&mut out, OPTION_MESSAGE_TYPE, &[self.message_type.to_raw()]);
        if let Some(address) = options.subnet_mask {
            put_option(&mut out, OPTION_SUBNET_MASK, &address.octets());
        }
        put_addresses(&mut out, OPTION_ROUTER, &options.routers);
        put_addresses(&mut out, OPTION_DNS_SERVER, &options.dns_servers);
        if let Some(name) = &options.domain_name {
            put_option(&mut out, OPTION_DOMAIN_NAME, name.as_bytes());
        }
        if let Some(address) = options.requested_address {
            put_option(&mut out, OPTION_REQUESTED_ADDRESS, &address.octets());
        }
        for (code, value) in [(OPTION_LEASE_TIME, options.lease_time), (OPTION_RENEWAL_TIME, options.renewal_time), (OPTION_REBINDING_TIME, options.rebinding_time)] {
            if let Some(seconds) = value {
                put_option(&mut out, code, &seconds.to_be_bytes());
            }
        }
        if let Some(address) = options.server_id {
            put_option(&mut out, OPTION_SERVER_ID, &address.octets());
        }
        if !options.parameter_request_list.is_empty() {
            put_option(&mut out, OPTION_PARAMETER_REQUEST_LIST, &options.parameter_request_list);
        }
        if let Some(text) = &options.message {
            put_option(&mut out, OPTION_MESSAGE, text.as_bytes());
        }
        if let Some(id) = &options.client_id {
            put_option(&mut out, OPTION_CLIENT_ID, id);
        }
        out.push(OPTION_END);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < FIXED_LEN + MAGIC_COOKIE.len() {
            return Err(DecodeError::TooShort { length: bytes.len() });
        }
        let op = bytes[0];
        if op != OP_REQUEST && op != OP_REPLY {
            return Err(DecodeError::BadOp(op));
        }
        if bytes[1] != HARDWARE_ETHERNET || bytes[2] != HARDWARE_LENGTH {
            return Err(DecodeError::BadHardware { hardware_type: bytes[1], length: bytes[2] });
        }
        if bytes[FIXED_LEN..FIXED_LEN + 4] != MAGIC_COOKIE {
            return Err(DecodeError::BadMagicCookie);
        }
        let address = |at: usize| Ipv4Addr::new(bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]);
        let mut options = Options::default();
        let mut message_type = None;

        let mut rest = &bytes[FIXED_LEN + 4..];
        while let Some((&code, tail)) = rest.split_first() {
            match code {
                OPTION_PAD => {
                    rest = tail;
                    continue;
                }
                OPTION_END => break,
                _ => {}
            }
            let (&length, tail) = tail.split_first().ok_or(DecodeError::TruncatedOption { code })?;
            let length = usize::from(length);
            let data = tail.get(..length).ok_or(DecodeError::TruncatedOption { code })?;
            rest = &tail[length..];
            let bad_length = || DecodeError::OptionLength { code, length };
            let one_address = || <[u8; 4]>::try_from(data).map(Ipv4Addr::from).map_err(|_| bad_length());
            let number = || <[u8; 4]>::try_from(data).map(u32::from_be_bytes).map_err(|_| bad_length());
            let address_list = || {
                if data.is_empty() || data.len() % 4 != 0 {
                    return Err(bad_length());
                }
                Ok(data.as_chunks::<4>().0.iter().map(|chunk| Ipv4Addr::from(*chunk)).collect::<Vec<_>>())
            };
            let text = || String::from_utf8(data.to_vec()).map_err(|_| DecodeError::OptionNotUtf8 { code });
            match code {
                OPTION_SUBNET_MASK => options.subnet_mask = Some(one_address()?),
                OPTION_ROUTER => options.routers = address_list()?,
                OPTION_DNS_SERVER => options.dns_servers = address_list()?,
                OPTION_DOMAIN_NAME => options.domain_name = Some(text()?),
                OPTION_REQUESTED_ADDRESS => options.requested_address = Some(one_address()?),
                OPTION_LEASE_TIME => options.lease_time = Some(number()?),
                OPTION_OVERLOAD => return Err(DecodeError::Overload),
                OPTION_MESSAGE_TYPE => {
                    let [raw] = data else { return Err(bad_length()) };
                    message_type = Some(MessageType::from_raw(*raw).ok_or(DecodeError::BadMessageType(*raw))?);
                }
                OPTION_SERVER_ID => options.server_id = Some(one_address()?),
                OPTION_PARAMETER_REQUEST_LIST => options.parameter_request_list = data.to_vec(),
                OPTION_MESSAGE => options.message = Some(text()?),
                OPTION_RENEWAL_TIME => options.renewal_time = Some(number()?),
                OPTION_REBINDING_TIME => options.rebinding_time = Some(number()?),
                OPTION_CLIENT_ID => options.client_id = Some(data.to_vec()),
                // an option this client does not use
                _ => {}
            }
        }

        let message_type = message_type.ok_or(DecodeError::MissingMessageType)?;
        if message_type.is_reply() != (op == OP_REPLY) {
            return Err(DecodeError::OpMismatch);
        }
        Ok(Self {
            message_type,
            xid: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            seconds: u16::from_be_bytes([bytes[8], bytes[9]]),
            broadcast: u16::from_be_bytes([bytes[10], bytes[11]]) & FLAG_BROADCAST != 0,
            client_address: address(12),
            your_address: address(16),
            server_address: address(20),
            gateway_address: address(24),
            client_hardware: MacAddr([bytes[28], bytes[29], bytes[30], bytes[31], bytes[32], bytes[33]]),
            options,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: MacAddr = MacAddr([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);

    /// A DHCPOFFER built field by field from RFC 2131 figure 1 and the RFC 2132 option layouts.
    fn offer_bytes() -> Vec<u8> {
        let mut bytes = vec![
            2, 1, 6, 0, // op BOOTREPLY, htype Ethernet, hlen 6, hops 0
            0xde, 0xad, 0xbe, 0xef, // xid
            0, 0, 0x80, 0, // secs 0, flags broadcast
            0, 0, 0, 0, // ciaddr
            192, 168, 7, 20, // yiaddr
            192, 168, 7, 1, // siaddr
            0, 0, 0, 0, // giaddr
            0x02, 0, 0, 0, 0, 1, // chaddr (6 of 16 bytes)
        ];
        bytes.resize(236, 0); // rest of chaddr, sname, file
        bytes.extend_from_slice(&[99, 130, 83, 99]); // magic cookie
        bytes.extend_from_slice(&[53, 1, 2]); // message type: offer
        bytes.extend_from_slice(&[54, 4, 192, 168, 7, 1]); // server identifier
        bytes.extend_from_slice(&[51, 4, 0, 0, 0x0e, 0x10]); // lease time 3600 s
        bytes.extend_from_slice(&[58, 4, 0, 0, 0x07, 0x08]); // T1 1800 s
        bytes.extend_from_slice(&[59, 4, 0, 0, 0x0c, 0x4e]); // T2 3150 s
        bytes.extend_from_slice(&[1, 4, 255, 255, 255, 0]); // subnet mask
        bytes.extend_from_slice(&[3, 4, 192, 168, 7, 1]); // router
        bytes.extend_from_slice(&[6, 8, 192, 168, 7, 1, 1, 1, 1, 1]); // two DNS servers
        bytes.extend_from_slice(&[15, 5, b'l', b'a', b'b', b'.', b'x']); // domain name
        bytes.extend_from_slice(&[0, 0]); // padding
        bytes.push(255);
        bytes
    }

    #[test]
    fn decodes_an_offer_with_the_lease_options() {
        let message = Message::decode(&offer_bytes()).unwrap();
        assert_eq!(message.message_type, MessageType::Offer);
        assert_eq!(message.xid, 0xdead_beef);
        assert!(message.broadcast);
        assert_eq!(message.your_address, Ipv4Addr::new(192, 168, 7, 20));
        assert_eq!(message.client_hardware, MAC);
        assert_eq!(
            message.options,
            Options {
                subnet_mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
                routers: vec![Ipv4Addr::new(192, 168, 7, 1)],
                dns_servers: vec![Ipv4Addr::new(192, 168, 7, 1), Ipv4Addr::new(1, 1, 1, 1)],
                domain_name: Some("lab.x".to_owned()),
                lease_time: Some(3600),
                server_id: Some(Ipv4Addr::new(192, 168, 7, 1)),
                renewal_time: Some(1800),
                rebinding_time: Some(3150),
                ..Options::default()
            }
        );
    }

    #[test]
    fn encodes_a_discover_byte_for_byte() {
        let mut discover = Message::new(MessageType::Discover, 0x0102_0304, MAC);
        discover.broadcast = true;
        discover.options.parameter_request_list = vec![1, 3, 6];
        discover.options.client_id = Some(vec![1, 2, 0, 0, 0, 0, 1]);
        let mut expected = vec![1, 1, 6, 0, 1, 2, 3, 4, 0, 0, 0x80, 0];
        expected.extend_from_slice(&[0; 16]); // ciaddr, yiaddr, siaddr, giaddr
        expected.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
        expected.resize(236, 0);
        expected.extend_from_slice(&[99, 130, 83, 99, 53, 1, 1, 55, 3, 1, 3, 6, 61, 7, 1, 2, 0, 0, 0, 0, 1, 255]);
        assert_eq!(discover.encode(), expected);
    }

    #[test]
    fn a_message_survives_a_round_trip() {
        let mut request = Message::new(MessageType::Request, 7, MAC);
        request.client_address = Ipv4Addr::new(10, 0, 0, 5);
        request.options.requested_address = Some(Ipv4Addr::new(10, 0, 0, 5));
        request.options.server_id = Some(Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(Message::decode(&request.encode()), Ok(request));
    }

    #[test]
    fn a_nak_carries_its_text() {
        let mut nak = Message::new(MessageType::Nak, 9, MAC);
        nak.options.message = Some("wrong network".to_owned());
        let decoded = Message::decode(&nak.encode()).unwrap();
        assert_eq!((decoded.message_type, decoded.options.message.as_deref()), (MessageType::Nak, Some("wrong network")));
    }

    #[test]
    fn short_input_and_a_missing_cookie_are_errors() {
        assert_eq!(Message::decode(&[0; 100]), Err(DecodeError::TooShort { length: 100 }));
        let mut bytes = offer_bytes();
        bytes[236] = 0;
        assert_eq!(Message::decode(&bytes), Err(DecodeError::BadMagicCookie));
    }

    #[test]
    fn an_option_longer_than_the_message_is_an_error() {
        let mut bytes = offer_bytes();
        bytes.truncate(236 + 4);
        bytes.extend_from_slice(&[53, 1, 2, 15, 40, b'x']);
        assert_eq!(Message::decode(&bytes), Err(DecodeError::TruncatedOption { code: 15 }));
    }

    #[test]
    fn an_address_option_with_the_wrong_size_is_an_error() {
        let mut bytes = offer_bytes();
        bytes.truncate(236 + 4);
        bytes.extend_from_slice(&[53, 1, 2, 1, 3, 1, 2, 3, 255]);
        assert_eq!(Message::decode(&bytes), Err(DecodeError::OptionLength { code: 1, length: 3 }));
    }

    #[test]
    fn a_message_without_a_type_is_an_error() {
        let mut bytes = offer_bytes();
        bytes.truncate(236 + 4);
        bytes.push(255);
        assert_eq!(Message::decode(&bytes), Err(DecodeError::MissingMessageType));
    }

    #[test]
    fn a_reply_type_in_a_request_is_an_error() {
        let mut bytes = offer_bytes();
        bytes[0] = 1;
        assert_eq!(Message::decode(&bytes), Err(DecodeError::OpMismatch));
    }

    #[test]
    fn option_overload_is_refused() {
        let mut bytes = offer_bytes();
        bytes.truncate(236 + 4);
        bytes.extend_from_slice(&[53, 1, 2, 52, 1, 3, 255]);
        assert_eq!(Message::decode(&bytes), Err(DecodeError::Overload));
    }
}
