//! The state of one connection, enforced by the manager. A message the state does not allow closes the connection.

use crate::message::{Event, Outcome, ProtocolRefusal, Request, VERSION};

/// Where a connection is in the protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Connected; the screen has not said `Hello`.
    New,
    /// Greeted; the screen is on the picker.
    Idle,
    /// An attempt is open: prompts and answers.
    Authenticating,
    /// The attempt was accepted and the session starts; only `Reset` ends this.
    Starting,
}

impl State {
    /// The state after `request`, or why the request is refused. Whether an `Answer` has a prompt to answer is the manager's
    /// to check: the state does not see the prompts.
    pub fn next(self, request: &Request) -> Result<State, ProtocolRefusal> {
        match (self, request) {
            (State::New, Request::Hello { version }) if *version == VERSION => Ok(State::Idle),
            (State::New, Request::Hello { .. }) => Err(ProtocolRefusal::Version),
            (State::Idle, Request::Authenticate { .. } | Request::AuthenticatePower { .. }) => Ok(State::Authenticating),
            (State::Authenticating, Request::Answer { .. }) => Ok(State::Authenticating),
            // an abort with nothing to abort, or after the attempt was accepted, changes nothing
            (State::Idle | State::Starting, Request::Cancel {}) => Ok(self),
            (State::Authenticating, Request::Cancel {}) => Ok(State::Idle),
            // the power menu stays reachable on every screen; the request changes nothing about the attempt
            (State::Idle | State::Authenticating | State::Starting, Request::Power { .. }) => Ok(self),
            _ => Err(ProtocolRefusal::State),
        }
    }

    /// The state after the manager sent `event`.
    pub fn after(self, event: &Event) -> State {
        match (self, event) {
            (State::Authenticating, Event::Result { outcome: Outcome::Accepted }) => State::Starting,
            (State::Authenticating, Event::Result { .. }) => State::Idle,
            (State::Starting | State::Authenticating, Event::Reset) => State::Idle,
            _ => self,
        }
    }
}
