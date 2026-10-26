//! Who may change the network settings: the caller's process must belong to a logind session that is
//! active on a seat (rule 5). No polkit.

use cupertino_seat::{inspect, locate, SessionLookup};
use zbus::blocking::Connection;

use crate::error::{Error, Result};

/// The answer to "may this process write?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// Refused, with the reason that goes back to the caller.
    Denied(String),
}

/// Decides whether the process `pid` may change the network settings. The daemon obtains `pid` from the
/// bus (`GetConnectionCredentials`), so the caller cannot name another process. An `Err` is a
/// failure to decide, not a refusal.
pub trait Authorizer: Send + Sync {
    fn authorize(&self, pid: u32) -> Result<Decision>;
}

/// The error name logind answers `GetSessionByPID` with for a process outside any session.
const NO_SESSION_FOR_PID: &str = "org.freedesktop.login1.NoSessionForPID";

/// Asks logind: the process must be in a session that is `Active` and has a seat.
pub struct LogindAuthorizer {
    /// A connection to the system bus, where logind lives.
    system_bus: Connection,
}

impl LogindAuthorizer {
    pub fn new(system_bus: Connection) -> Self {
        Self { system_bus }
    }
}

impl Authorizer for LogindAuthorizer {
    fn authorize(&self, pid: u32) -> Result<Decision> {
        let session = match locate(&self.system_bus, &SessionLookup::ByPid(pid)) {
            Ok(session) => session,
            Err(cupertino_seat::Error::Bus { source, .. }) if matches!(source.as_ref(), zbus::Error::MethodError(name, _, _) if name.as_str() == NO_SESSION_FOR_PID) =>
            {
                return Ok(Decision::Denied(
                    "the caller is not in a login session".to_owned(),
                ));
            }
            Err(source) => return Err(Error::Seat(source)),
        };
        let info = inspect(&self.system_bus, &session).map_err(Error::Seat)?;
        Ok(if info.seat.is_empty() {
            Decision::Denied(format!("session {} has no seat", info.id))
        } else if !info.active {
            Decision::Denied(format!(
                "session {} is not the active session of {}",
                info.id, info.seat
            ))
        } else {
            Decision::Allowed
        })
    }
}

/// Allows every caller. Only for running unprivileged on a session bus.
pub struct AllowAll;

impl Authorizer for AllowAll {
    fn authorize(&self, _pid: u32) -> Result<Decision> {
        Ok(Decision::Allowed)
    }
}
