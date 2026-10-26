//! A cursor over received bytes that reports what it was reading when the bytes ran out.

use crate::address::{Address, AddressType, DeviceId};
use crate::error::ParseError;

/// A NUL-terminated string in a fixed-size field. The text comes from the remote side or the
/// controller's owner, so invalid UTF-8 becomes U+FFFD instead of failing the packet.
pub(crate) fn fixed_string(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    what: &'static str,
}

impl<'a> Reader<'a> {
    /// `what` names the structure being read, for error messages.
    pub(crate) fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, what }
    }

    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], ParseError> {
        if self.bytes.len() < count {
            return Err(ParseError::Truncated { what: self.what, needed: count, found: self.bytes.len() });
        }
        let (head, tail) = self.bytes.split_at(count);
        self.bytes = tail;
        Ok(head)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, ParseError> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, ParseError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, ParseError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// A byte that must be 0 or 1.
    pub(crate) fn bool(&mut self, what: &'static str) -> Result<bool, ParseError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(ParseError::InvalidBool { what, value }),
        }
    }

    pub(crate) fn device(&mut self) -> Result<DeviceId, ParseError> {
        let address = Address::from_wire(self.array()?);
        Ok(DeviceId { address, kind: AddressType::from_wire(self.u8()?)? })
    }

    pub(crate) fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.bytes)
    }

    /// Fails when bytes are left over.
    pub(crate) fn finish(self) -> Result<(), ParseError> {
        match self.bytes.len() {
            0 => Ok(()),
            extra => Err(ParseError::TrailingBytes { what: self.what, extra }),
        }
    }
}
