//! Netlink attributes (`struct nlattr`): a 4-byte header (length, type) and a payload padded to 4 bytes.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::error::ParseError;
use crate::message::align4;

pub const NLA_HEADER_LEN: usize = 4;
/// The payload is a list of attributes.
pub const NLA_F_NESTED: u16 = 1 << 15;
/// The payload is in network byte order.
pub const NLA_F_NET_BYTEORDER: u16 = 1 << 14;
const NLA_TYPE_MASK: u16 = !(NLA_F_NESTED | NLA_F_NET_BYTEORDER);

/// One attribute, with the nested and byte-order flags removed from `ty`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attr<'a> {
    pub ty: u16,
    pub data: &'a [u8],
}

/// The attributes in `buffer` (the part of a message after its family header). After the first
/// malformed attribute the iterator ends.
pub fn attrs(buffer: &[u8]) -> Attrs<'_> {
    Attrs { rest: buffer }
}

pub struct Attrs<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Attrs<'a> {
    type Item = Result<Attr<'a>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let item = parse_attr(self.rest);
        self.rest = match &item {
            Ok((_, consumed)) => &self.rest[*consumed..],
            Err(_) => &[],
        };
        Some(item.map(|(attr, _)| attr))
    }
}

fn parse_attr(buffer: &[u8]) -> Result<(Attr<'_>, usize), ParseError> {
    if buffer.len() < NLA_HEADER_LEN {
        return Err(ParseError::Truncated {
            what: "attribute header",
            needed: NLA_HEADER_LEN,
            have: buffer.len(),
        });
    }
    let length = u16::from_ne_bytes([buffer[0], buffer[1]]);
    let ty = u16::from_ne_bytes([buffer[2], buffer[3]]) & NLA_TYPE_MASK;
    if (length as usize) < NLA_HEADER_LEN || length as usize > buffer.len() {
        return Err(ParseError::BadAttributeLength {
            attribute: ty,
            length,
            available: buffer.len(),
        });
    }
    Ok((
        Attr {
            ty,
            data: &buffer[NLA_HEADER_LEN..length as usize],
        },
        align4(length as usize).min(buffer.len()),
    ))
}

impl<'a> Attr<'a> {
    fn fixed<const N: usize>(&self) -> Result<[u8; N], ParseError> {
        self.data.try_into().map_err(|_| ParseError::AttributeSize {
            attribute: self.ty,
            expected: N,
            actual: self.data.len(),
        })
    }

    pub fn u8(&self) -> Result<u8, ParseError> {
        Ok(self.fixed::<1>()?[0])
    }

    pub fn i8(&self) -> Result<i8, ParseError> {
        Ok(i8::from_ne_bytes(self.fixed()?))
    }

    pub fn u16(&self) -> Result<u16, ParseError> {
        Ok(u16::from_ne_bytes(self.fixed()?))
    }

    pub fn u32(&self) -> Result<u32, ParseError> {
        Ok(u32::from_ne_bytes(self.fixed()?))
    }

    pub fn i32(&self) -> Result<i32, ParseError> {
        Ok(i32::from_ne_bytes(self.fixed()?))
    }

    /// A 6-byte hardware address.
    pub fn mac(&self) -> Result<[u8; 6], ParseError> {
        self.fixed()
    }

    /// An IPv4 (4 bytes) or IPv6 (16 bytes) address in network order.
    pub fn ip(&self) -> Result<IpAddr, ParseError> {
        match self.data.len() {
            4 => Ok(IpAddr::V4(Ipv4Addr::from(self.fixed::<4>()?))),
            16 => Ok(IpAddr::V6(Ipv6Addr::from(self.fixed::<16>()?))),
            actual => Err(ParseError::AddressSize {
                attribute: self.ty,
                actual,
            }),
        }
    }

    /// A string attribute: UTF-8, with the terminating NUL (if there is one) removed.
    pub fn string(&self) -> Result<&'a str, ParseError> {
        let bytes = self.data.strip_suffix(&[0]).unwrap_or(self.data);
        std::str::from_utf8(bytes).map_err(|_| ParseError::AttributeNotUtf8 { attribute: self.ty })
    }

    /// The children of a nested attribute.
    pub fn nested(&self) -> Attrs<'a> {
        attrs(self.data)
    }
}
