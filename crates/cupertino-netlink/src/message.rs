//! Netlink framing (`linux/netlink.h`): the 16-byte header, the message iterator over a received
//! buffer and the body builder that attributes are written with.

use crate::attr::{NLA_F_NESTED, NLA_HEADER_LEN};
use crate::error::ParseError;

pub const HEADER_LEN: usize = 16;

/// `NLMSG_ERROR`: the payload starts with a negative errno; zero is an acknowledgement.
pub const NLMSG_ERROR: u16 = 2;
/// `NLMSG_DONE`: the end of a multipart reply.
pub const NLMSG_DONE: u16 = 3;

pub const NLM_F_REQUEST: u16 = 0x1;
pub const NLM_F_ACK: u16 = 0x4;
pub const NLM_F_ROOT: u16 = 0x100;
pub const NLM_F_MATCH: u16 = 0x200;
/// `NLM_F_ROOT | NLM_F_MATCH`: ask for every object.
pub const NLM_F_DUMP: u16 = NLM_F_ROOT | NLM_F_MATCH;
/// For a new object: replace one that exists.
pub const NLM_F_REPLACE: u16 = 0x100;
/// For a new object: fail when one exists.
pub const NLM_F_EXCL: u16 = 0x200;
/// For a new object: create it when it is missing.
pub const NLM_F_CREATE: u16 = 0x400;

/// Rounds up to the 4-byte alignment of netlink messages and attributes.
pub const fn align4(length: usize) -> usize {
    (length + 3) & !3
}

/// The `struct nlmsghdr` of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Length of the message including this header.
    pub length: u32,
    pub ty: u16,
    pub flags: u16,
    pub sequence: u32,
    /// The sender's port id; 0 is the kernel.
    pub pid: u32,
}

/// One message of a received buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message<'a> {
    pub header: Header,
    /// Everything after the header, without the padding after the last attribute.
    pub payload: &'a [u8],
}

/// The messages of `buffer`, which holds one or more datagrams concatenated. After the first
/// malformed message the iterator ends: the rest of the buffer cannot be framed.
pub fn messages(buffer: &[u8]) -> Messages<'_> {
    Messages { rest: buffer }
}

pub struct Messages<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Messages<'a> {
    type Item = Result<Message<'a>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let item = parse_message(self.rest);
        self.rest = match &item {
            Ok((_, consumed)) => &self.rest[*consumed..],
            Err(_) => &[],
        };
        Some(item.map(|(message, _)| message))
    }
}

/// Reads the first message of `buffer` and the number of bytes it occupies, padding included
/// (the last message of a buffer may omit its padding).
fn parse_message(buffer: &[u8]) -> Result<(Message<'_>, usize), ParseError> {
    if buffer.len() < HEADER_LEN {
        return Err(ParseError::Truncated {
            what: "netlink header",
            needed: HEADER_LEN,
            have: buffer.len(),
        });
    }
    let word = |at: usize| {
        u32::from_ne_bytes([buffer[at], buffer[at + 1], buffer[at + 2], buffer[at + 3]])
    };
    let half = |at: usize| u16::from_ne_bytes([buffer[at], buffer[at + 1]]);
    let header = Header {
        length: word(0),
        ty: half(4),
        flags: half(6),
        sequence: word(8),
        pid: word(12),
    };
    let length = header.length as usize;
    if length < HEADER_LEN || length > buffer.len() {
        return Err(ParseError::BadMessageLength {
            length: header.length,
            available: buffer.len(),
        });
    }
    Ok((
        Message {
            header,
            payload: &buffer[HEADER_LEN..length],
        },
        align4(length).min(buffer.len()),
    ))
}

/// Writes the header of a request in front of `body`.
pub fn frame(ty: u16, flags: u16, sequence: u32, body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(HEADER_LEN + body.len()).expect("a request is far below 4 GiB");
    let mut message = Vec::with_capacity(HEADER_LEN + body.len());
    message.extend_from_slice(&length.to_ne_bytes());
    message.extend_from_slice(&ty.to_ne_bytes());
    message.extend_from_slice(&flags.to_ne_bytes());
    message.extend_from_slice(&sequence.to_ne_bytes());
    // pid 0: the kernel does not read the sender's port id of a request.
    message.extend_from_slice(&0u32.to_ne_bytes());
    message.extend_from_slice(body);
    message
}

/// The body of a request: a fixed-size family header followed by attributes, each padded to 4 bytes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Body {
    bytes: Vec<u8>,
}

impl Body {
    pub fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Appends the family header (`ifinfomsg`, `ifaddrmsg`, `rtmsg`, `genlmsghdr`), padded to 4 bytes.
    pub fn header(&mut self, bytes: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(bytes);
        self.pad();
        self
    }

    /// Appends one attribute.
    pub fn attr(&mut self, ty: u16, data: &[u8]) -> &mut Self {
        let length =
            u16::try_from(NLA_HEADER_LEN + data.len()).expect("an attribute is far below 64 KiB");
        self.bytes.extend_from_slice(&length.to_ne_bytes());
        self.bytes.extend_from_slice(&ty.to_ne_bytes());
        self.bytes.extend_from_slice(data);
        self.pad();
        self
    }

    pub fn attr_u32(&mut self, ty: u16, value: u32) -> &mut Self {
        self.attr(ty, &value.to_ne_bytes())
    }

    /// Appends a NUL-terminated string attribute.
    pub fn attr_cstr(&mut self, ty: u16, value: &str) -> &mut Self {
        let mut data = value.as_bytes().to_vec();
        data.push(0);
        self.attr(ty, &data)
    }

    /// Appends a nested attribute whose children `build` writes.
    pub fn nested(&mut self, ty: u16, build: impl FnOnce(&mut Body)) -> &mut Self {
        let mut inner = Body::new();
        build(&mut inner);
        self.attr(ty | NLA_F_NESTED, &inner.bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn pad(&mut self) {
        self.bytes.resize(align4(self.bytes.len()), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_a_request_with_a_nested_attribute() {
        let mut body = Body::new();
        body.header(&[2, 0, 0, 0])
            .attr_u32(3, 7)
            .nested(45, |inner| {
                inner.attr(1, &[]);
            });
        let message = frame(18, NLM_F_REQUEST, 9, body.as_bytes());
        assert_eq!(
            message,
            [
                // nlmsghdr: length 36, type 18, flags 1, sequence 9, pid 0
                &36u32.to_ne_bytes()[..],
                &18u16.to_ne_bytes(),
                &1u16.to_ne_bytes(),
                &9u32.to_ne_bytes(),
                &0u32.to_ne_bytes(),
                // family header
                &[2, 0, 0, 0],
                // attribute 3 = 7
                &8u16.to_ne_bytes(),
                &3u16.to_ne_bytes(),
                &7u32.to_ne_bytes(),
                // attribute 45 | NESTED holding an empty attribute 1
                &8u16.to_ne_bytes(),
                &(45u16 | NLA_F_NESTED).to_ne_bytes(),
                &4u16.to_ne_bytes(),
                &1u16.to_ne_bytes(),
            ]
            .concat()
        );
    }

    #[test]
    fn splits_a_buffer_into_messages_and_pads() {
        let mut buffer = frame(16, 2, 1, &[1, 2, 3]);
        buffer.resize(align4(buffer.len()), 0);
        buffer.extend(frame(NLMSG_DONE, 2, 1, &0i32.to_ne_bytes()));
        let parsed: Vec<_> = messages(&buffer).collect::<Result<_, _>>().unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].header.ty, 16);
        assert_eq!(parsed[0].payload, [1, 2, 3]);
        assert_eq!(parsed[1].header.ty, NLMSG_DONE);
    }

    #[test]
    fn a_length_past_the_buffer_is_an_error_and_ends_the_iteration() {
        let mut buffer = frame(16, 0, 1, &[0; 8]);
        buffer[0..4].copy_from_slice(&200u32.to_ne_bytes());
        let mut iterator = messages(&buffer);
        assert_eq!(
            iterator.next(),
            Some(Err(ParseError::BadMessageLength {
                length: 200,
                available: 24
            }))
        );
        assert_eq!(iterator.next(), None);
    }

    #[test]
    fn a_partial_header_is_truncated() {
        let mut iterator = messages(&[0; 10]);
        assert_eq!(
            iterator.next(),
            Some(Err(ParseError::Truncated {
                what: "netlink header",
                needed: 16,
                have: 10
            }))
        );
    }
}
