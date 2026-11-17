//! A stream socket that carries frames: one read per call, so the owner can poll it, and the frames of
//! `cupertino-login-ipc` ([`Decoder`] limits what is buffered).

use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

use cupertino_login_ipc::{decode, DecodeError, Decoder, FrameError};
use serde::de::DeserializeOwned;

/// Bytes read per call at most.
const CHUNK: usize = 4096;

/// Why a channel cannot be used any more. The texts never hold message content.
#[derive(Debug)]
pub enum ChannelError {
    /// The peer closed the stream.
    Closed,
    Frame(FrameError),
    Decode(DecodeError),
    Io(io::Error),
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("the peer closed the connection"),
            Self::Frame(e) => write!(f, "{e}"),
            Self::Decode(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ChannelError {}

pub struct Channel {
    stream: UnixStream,
    decoder: Decoder,
}

impl Channel {
    /// A channel on `stream` that accepts frames of at most `limit` bytes.
    pub fn new(stream: UnixStream, limit: usize) -> Self {
        Channel {
            stream,
            decoder: Decoder::new(limit),
        }
    }

    pub fn stream(&self) -> &UnixStream {
        &self.stream
    }

    /// One read; the next message when it completes a frame. Meant for after the stream polled readable (or for a blocking
    /// stream): it blocks otherwise.
    pub fn read_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, ChannelError> {
        let mut buffer = [0u8; CHUNK];
        let wanted = self.decoder.wanted().min(CHUNK);
        let count = loop {
            match self.stream.read(&mut buffer[..wanted]) {
                Ok(0) => return Err(ChannelError::Closed),
                Ok(count) => break count,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(ChannelError::Io(e)),
            }
        };
        let frame = self
            .decoder
            .push(&buffer[..count])
            .map_err(ChannelError::Frame);
        // the bytes read may be a secret
        zeroize::Zeroize::zeroize(&mut buffer[..count]);
        match frame? {
            None => Ok(None),
            Some(body) => decode(&body).map(Some).map_err(ChannelError::Decode),
        }
    }

    /// Block until a whole message has arrived.
    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<T, ChannelError> {
        loop {
            if let Some(message) = self.read_message()? {
                return Ok(message);
            }
        }
    }

    /// Write a frame (the output of `encode`).
    pub fn send_frame(&mut self, frame: &[u8]) -> Result<(), ChannelError> {
        self.stream.write_all(frame).map_err(ChannelError::Io)
    }
}
