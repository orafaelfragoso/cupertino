use std::fmt;
use std::io;
use std::os::fd::RawFd;
use std::path::PathBuf;

use cupertino_devices::DevT;
use rustix::io::Errno;
use smithay::backend::session::AsErrno;

/// Every failure of this crate.
#[derive(Debug)]
pub enum Error {
    /// A D-Bus call or the bus connection failed; `what` names the step.
    Bus {
        what: &'static str,
        source: Box<zbus::Error>,
    },
    /// `XDG_SESSION_ID` is set but is not valid Unicode.
    SessionIdNotUnicode,
    /// The signal reader thread could not be started.
    Thread(io::Error),
    /// The session's signals stopped: the bus connection closed, or logind left the bus name the
    /// session is bound to (it exited or restarted).
    SignalsEnded,
    /// logind sent a `PauseDevice` type this crate does not know.
    UnknownPauseType(String),
    /// logind reported `Active` as invalidated instead of sending its value.
    ActiveInvalidated,
    /// `stat` on a device path failed.
    Stat { path: PathBuf, errno: Errno },
    /// The path is not a character device, which is all `TakeDevice` accepts.
    NotCharDevice(PathBuf),
    /// A device is already held by this session.
    AlreadyTaken(DevT),
    /// A descriptor this session did not hand out (or already took back).
    UnknownFd(RawFd),
    /// A virtual terminal number that does not fit logind's unsigned argument.
    InvalidVt(i32),
    /// The session was dropped together with its notifier.
    SessionLost,
}

impl Error {
    /// The adapter for `map_err` on a D-Bus call: names the step that failed. The `zbus` error is
    /// boxed because it is large and would bloat every `Result` of this crate.
    pub(crate) fn bus(what: &'static str) -> impl Fn(zbus::Error) -> Self + Copy {
        move |source| Self::Bus {
            what,
            source: Box::new(source),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus { what, source } => write!(f, "{what}: {source}"),
            Self::SessionIdNotUnicode => write!(f, "XDG_SESSION_ID is not valid Unicode"),
            Self::Thread(source) => write!(f, "starting the signal reader: {source}"),
            Self::SignalsEnded => write!(f, "the logind signal stream ended"),
            Self::UnknownPauseType(kind) => write!(f, "unknown PauseDevice type {kind:?}"),
            Self::ActiveInvalidated => {
                write!(f, "logind invalidated Active instead of sending its value")
            }
            Self::Stat { path, errno } => write!(f, "stat {}: {errno}", path.display()),
            Self::NotCharDevice(path) => write!(f, "{} is not a character device", path.display()),
            Self::AlreadyTaken(dev) => write!(f, "device {dev} is already taken"),
            Self::UnknownFd(fd) => write!(f, "descriptor {fd} was not handed out by this session"),
            Self::InvalidVt(vt) => write!(f, "invalid virtual terminal number {vt}"),
            Self::SessionLost => write!(f, "the session is closed"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bus { source, .. } => Some(source.as_ref()),
            Self::Thread(source) => Some(source),
            _ => None,
        }
    }
}

impl AsErrno for Error {
    fn as_errno(&self) -> Option<i32> {
        match self {
            Self::Stat { errno, .. } => Some(errno.raw_os_error()),
            _ => None,
        }
    }
}
