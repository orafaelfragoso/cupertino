//! The login screen's side of the session manager's protocol (`cupertino-login-ipc`): one connection to the manager's socket,
//! a thread that reads its events and a handle that sends requests.
//!
//! The login screen never talks to PAM: the manager runs the authentication in a worker and the screen only relays what the
//! user types. Errors name the problem, never a message: a decoding error can carry the text the user typed.

use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

use cupertino_login_ipc::message::{MAX_EVENT_FRAME, MAX_REQUEST_FRAME};
use cupertino_login_ipc::{decode, encode_secret, DecodeError, Decoder, Event, FrameError, Request, VERSION};

/// Environment variable that holds the path of the manager's socket.
pub const SOCKET_VARIABLE: &str = "CUPERTINO_LOGIN_SOCK";

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

/// What the reader thread tells the screen.
#[derive(Debug)]
pub enum Message {
    Event(Event),
    /// The connection is unusable; no further message follows.
    Broken(Error),
}

/// The sending half of the connection.
pub struct Connection {
    stream: UnixStream,
}

impl Connection {
    /// Greet the manager on `stream` and start reading its events; `on_message` runs on the reader thread.
    pub fn spawn(stream: UnixStream, on_message: impl Fn(Message) + Send + 'static) -> io::Result<Connection> {
        let mut reader = stream.try_clone()?;
        let mut connection = Connection { stream };
        connection.send(&Request::Hello { version: VERSION }).map_err(|e| io::Error::other(e.to_string()))?;
        std::thread::Builder::new().name("login-manager".into()).spawn(move || {
            let outcome = read_events(&mut reader, &on_message);
            // the thread ends with the connection
            if let Err(e) = outcome {
                on_message(Message::Broken(e));
            }
        })?;
        Ok(connection)
    }

    /// Send a request. A request that may carry a secret is written from a buffer that is zeroed afterwards.
    pub fn send(&mut self, request: &Request) -> Result<(), Error> {
        let frame = encode_secret(request, MAX_REQUEST_FRAME).map_err(Error::Frame)?;
        self.stream.write_all(&frame).map_err(Error::Io)
    }
}

/// Read events until the connection ends.
fn read_events(stream: &mut UnixStream, on_message: &impl Fn(Message)) -> Result<(), Error> {
    let mut decoder = Decoder::new(MAX_EVENT_FRAME);
    let mut buffer = [0u8; 4096];
    loop {
        let wanted = decoder.wanted().min(buffer.len());
        let count = match stream.read(&mut buffer[..wanted]) {
            Ok(0) => return Err(Error::Closed),
            Ok(count) => count,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::Io(e)),
        };
        if let Some(body) = decoder.push(&buffer[..count]).map_err(Error::Frame)? {
            on_message(Message::Event(decode(&body).map_err(Error::Decode)?));
        }
    }
}
