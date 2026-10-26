//! Framing: a `u32` little-endian length, then that many bytes of JSON. The decoder takes exactly the bytes it asks for, so
//! it never buffers more than one frame and never allocates more than the cap it was built with.

use std::fmt;
use std::io::{self, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;
use zeroize::Zeroizing;

/// Bytes of the length prefix.
pub const HEADER_BYTES: usize = 4;

/// A frame that cannot be decoded or encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The length prefix, or an encoded message, is larger than the cap of this direction.
    TooLarge,
    /// The length prefix is zero.
    Empty,
    /// More bytes were pushed than [`Decoder::wanted`] asked for.
    Overrun,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "frame larger than the cap",
            Self::Empty => "empty frame",
            Self::Overrun => "more bytes than the decoder asked for",
        })
    }
}

impl std::error::Error for FrameError {}

/// The message in a frame is not valid. Only the class of the problem is kept: serde's text embeds the offending value, which
/// may be a password, so it must never reach a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeError(serde_json::error::Category);

impl DecodeError {
    /// `io`, `syntax`, `data` or `eof`: what a log may say.
    pub fn class(&self) -> &'static str {
        use serde_json::error::Category;
        match self.0 {
            Category::Io => "io",
            Category::Syntax => "syntax",
            Category::Data => "data",
            Category::Eof => "eof",
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed message ({})", self.class())
    }
}

impl std::error::Error for DecodeError {}

/// Reads frames from a stream of bytes of unknown chunking.
pub struct Decoder {
    limit: usize,
    header: [u8; HEADER_BYTES],
    header_filled: usize,
    /// The body being read, with its final length as capacity so it never grows; zeroed on drop.
    body: Option<Zeroizing<Vec<u8>>>,
    body_length: usize,
}

impl Decoder {
    /// A decoder that refuses frames whose body is longer than `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Decoder { limit, header: [0; HEADER_BYTES], header_filled: 0, body: None, body_length: 0 }
    }

    /// The most bytes the next [`Decoder::push`] may take: the rest of the header, or the rest of the body.
    pub fn wanted(&self) -> usize {
        match &self.body {
            None => HEADER_BYTES - self.header_filled,
            Some(body) => self.body_length - body.len(),
        }
    }

    /// Take `bytes` (at most [`Decoder::wanted`]); a complete frame's body comes back. After an error the decoder is not
    /// usable and the connection must be closed.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<Zeroizing<Vec<u8>>>, FrameError> {
        if bytes.len() > self.wanted() {
            return Err(FrameError::Overrun);
        }
        let Some(body) = &mut self.body else {
            self.header[self.header_filled..self.header_filled + bytes.len()].copy_from_slice(bytes);
            self.header_filled += bytes.len();
            if self.header_filled < HEADER_BYTES {
                return Ok(None);
            }
            let length = u32::from_le_bytes(self.header) as usize;
            self.header_filled = 0;
            if length == 0 {
                return Err(FrameError::Empty);
            }
            if length > self.limit {
                return Err(FrameError::TooLarge);
            }
            self.body_length = length;
            self.body = Some(Zeroizing::new(Vec::with_capacity(length)));
            return Ok(None);
        };
        body.extend_from_slice(bytes);
        if body.len() < self.body_length {
            return Ok(None);
        }
        Ok(self.body.take())
    }
}

/// Parse the body of a frame as `T`.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, DecodeError> {
    serde_json::from_slice(body).map_err(|e| DecodeError(e.classify()))
}

/// A buffer that refuses to grow past its cap, so an oversized message stops serialising at once. It is zeroed when dropped,
/// also when serialising fails half way.
struct Capped {
    bytes: Zeroizing<Vec<u8>>,
    cap: usize,
}

impl Write for Capped {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.bytes.len() + data.len() > self.cap {
            return Err(io::Error::new(io::ErrorKind::OutOfMemory, "frame larger than the cap"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_into<T: Serialize>(message: &T, limit: usize, mut bytes: Zeroizing<Vec<u8>>) -> Result<Zeroizing<Vec<u8>>, FrameError> {
    bytes.extend_from_slice(&[0; HEADER_BYTES]);
    let mut out = Capped { bytes, cap: HEADER_BYTES + limit };
    serde_json::to_writer(&mut out, message).map_err(|_| FrameError::TooLarge)?;
    let length = u32::try_from(out.bytes.len() - HEADER_BYTES).map_err(|_| FrameError::TooLarge)?;
    out.bytes[..HEADER_BYTES].copy_from_slice(&length.to_le_bytes());
    Ok(out.bytes)
}

/// The frame for `message`: length prefix and JSON, at most `limit` bytes of JSON. For messages without secrets.
pub fn encode<T: Serialize>(message: &T, limit: usize) -> Result<Zeroizing<Vec<u8>>, FrameError> {
    encode_into(message, limit, Zeroizing::new(Vec::new()))
}

/// As [`encode`], for a message that may carry a secret: the buffer is allocated at its full size, so it never moves (and
/// leaves an unzeroed copy behind) while it is written.
pub fn encode_secret<T: Serialize>(message: &T, limit: usize) -> Result<Zeroizing<Vec<u8>>, FrameError> {
    encode_into(message, limit, Zeroizing::new(Vec::with_capacity(HEADER_BYTES + limit)))
}
