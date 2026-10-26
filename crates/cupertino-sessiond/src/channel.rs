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
        Channel { stream, decoder: Decoder::new(limit) }
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
        let frame = self.decoder.push(&buffer[..count]).map_err(ChannelError::Frame);
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

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_login_ipc::{encode, Request, VERSION};
    use std::io::Write;

    /// A channel that reads what the test writes to its peer.
    fn pair(limit: usize) -> (Channel, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        (Channel::new(ours, limit), theirs)
    }

    #[test]
    fn a_message_is_read_whole_however_the_bytes_arrive() {
        let (mut channel, mut peer) = pair(64);
        let frame = encode(&Request::Hello { version: VERSION }, 64).unwrap();
        for byte in frame.iter() {
            peer.write_all(&[*byte]).unwrap();
        }
        let mut message = None;
        while message.is_none() {
            message = channel.read_message::<Request>().unwrap();
        }
        assert!(matches!(message, Some(Request::Hello { version: VERSION })));
    }

    #[test]
    fn a_length_over_the_cap_is_an_error_before_any_body_is_read() {
        let (mut channel, mut peer) = pair(2048);
        peer.write_all(&3072u32.to_le_bytes()).unwrap();
        assert!(matches!(channel.read_message::<Request>(), Err(ChannelError::Frame(cupertino_login_ipc::FrameError::TooLarge))));
    }

    #[test]
    fn a_peer_that_closes_is_closed_and_an_unknown_type_is_a_decode_error() {
        let (mut channel, mut peer) = pair(2048);
        let body = br#"{"type":"Fly"}"#;
        peer.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
        peer.write_all(body).unwrap();
        let mut result = Ok(None);
        while matches!(result, Ok(None)) {
            result = channel.read_message::<Request>();
        }
        assert!(matches!(result, Err(ChannelError::Decode(_))));
        drop(peer);
        assert!(matches!(channel.read_message::<Request>(), Err(ChannelError::Closed)));
    }

    #[test]
    fn an_error_never_holds_the_text_that_caused_it() {
        let (mut channel, mut peer) = pair(2048);
        // a password where a version belongs: serde's own message would quote it
        let body = br#"{"type":"Hello","version":"hunter2-the-password"}"#;
        peer.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
        peer.write_all(body).unwrap();
        let error = loop {
            match channel.read_message::<Request>() {
                Ok(None) => {}
                Ok(Some(_)) => panic!("a malformed message was accepted"),
                Err(e) => break e,
            }
        };
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("data"), "the class is named: {shown}");
    }
}
