//! The login screen's side of the session manager's protocol (`cupertino-login-ipc`): one connection to the manager's
//! socket, whose reading and writing are a future of the screen's loop ([`Pump::run`]), and a handle that queues the
//! requests in order ([`Connection::send`]).
//!
//! The login screen never talks to PAM: the manager runs the authentication in a worker and the screen only relays what the
//! user types. Errors name the problem, never a message: a decoding error can carry the text the user typed.

use std::fmt;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use async_io::Async;
use cupertino_login_ipc::message::{MAX_EVENT_FRAME, MAX_REQUEST_FRAME};
use cupertino_login_ipc::{
    decode, encode_secret, DecodeError, Decoder, Event, FrameError, Request, VERSION,
};
use futures_lite::{future, AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

/// Environment variable that holds the path of the manager's socket.
pub const SOCKET_VARIABLE: &str = "CUPERTINO_LOGIN_SOCK";

/// The bytes read from the socket at a time.
const READ_CHUNK: usize = 4096;

/// The connection cannot be used any more.
#[derive(Debug)]
pub enum Error {
    /// The manager closed the connection.
    Closed,
    Io(io::Error),
    Frame(FrameError),
    Decode(DecodeError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("the session manager closed the connection"),
            Self::Io(e) => write!(f, "talking to the session manager: {e}"),
            Self::Frame(e) => write!(f, "talking to the session manager: {e}"),
            Self::Decode(e) => write!(f, "talking to the session manager: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// The sending half of the connection: requests are written in the order they are sent.
#[derive(Clone, Debug)]
pub struct Connection {
    frames: async_channel::Sender<Zeroizing<Vec<u8>>>,
}

/// The reading and writing of a connection, until it breaks.
pub struct Pump {
    stream: Arc<Async<UnixStream>>,
    frames: async_channel::Receiver<Zeroizing<Vec<u8>>>,
}

impl Connection {
    /// Connect to the manager's socket at `path` once, and greet it (`Hello`): its answer is the first event
    /// [`Pump::run`] hands over. A local socket accepts at once or refuses; the caller tries again later.
    pub async fn connect(path: &Path) -> io::Result<(Connection, Pump)> {
        let stream = Async::<UnixStream>::connect(path).await?;
        let (sender, frames) = async_channel::unbounded();
        let connection = Connection { frames: sender };
        connection
            .send(&Request::Hello { version: VERSION })
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok((
            connection,
            Pump {
                stream: Arc::new(stream),
                frames,
            },
        ))
    }

    /// Queue a request. A request that may carry a secret is encoded into a buffer that is zeroed once it is written.
    pub fn send(&self, request: &Request) -> Result<(), Error> {
        let frame = encode_secret(request, MAX_REQUEST_FRAME).map_err(Error::Frame)?;
        self.frames.try_send(frame).map_err(|_| Error::Closed)
    }
}

impl Pump {
    /// Write the queued requests and hand each event the manager sends to `on_event`, until the connection breaks;
    /// returns why.
    pub async fn run(self, on_event: impl Fn(Event)) -> Error {
        let writing = async {
            while let Ok(frame) = self.frames.recv().await {
                if let Err(e) = (&*self.stream).write_all(&frame).await {
                    return Error::Io(e);
                }
            }
            // every handle is gone: nothing more is sent
            Error::Closed
        };
        let reading = async {
            let mut decoder = Decoder::new(MAX_EVENT_FRAME);
            let mut buffer = [0u8; READ_CHUNK];
            loop {
                let wanted = decoder.wanted().min(buffer.len());
                let count = match (&*self.stream).read(&mut buffer[..wanted]).await {
                    Ok(0) => return Error::Closed,
                    Ok(count) => count,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Error::Io(e),
                };
                match decoder.push(&buffer[..count]) {
                    Ok(Some(body)) => match decode(&body) {
                        Ok(event) => on_event(event),
                        Err(e) => return Error::Decode(e),
                    },
                    Ok(None) => {}
                    Err(e) => return Error::Frame(e),
                }
            }
        };
        future::or(writing, reading).await
    }
}
