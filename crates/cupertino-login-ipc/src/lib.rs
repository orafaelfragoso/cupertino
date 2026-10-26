//! The protocol between the login screen (`cupertino-login --greeter`, unprivileged) and the session manager
//! (`cupertino-sessiond`, root) over `/run/cupertino-login/greeter.sock`, which is also where passwords travel.
//! Pure: no I/O, so both sides and their tests share one definition. The design is
//! section 2.4.
//!
//! * [`message`]: [`Request`] (screen to manager) and [`Event`] (manager to screen).
//! * [`frame`]: the length prefix, the size caps and the [`Decoder`] that reads a stream in exact amounts.
//! * [`secret`]: [`Secret`], a password that is zeroed, never printed and sent as hex.
//! * [`state`]: [`State`], the connection's state machine.
//!
//! Decoding errors keep only their class: serde's text embeds the offending value, which may be a password.

pub mod frame;
mod hex;
pub mod message;
pub mod secret;
pub mod state;

pub use frame::{decode, encode, encode_secret, DecodeError, Decoder, FrameError, HEADER_BYTES};
pub use message::{
    is_login_name, Account, Event, Outcome, Picture, PowerAction, PowerMenu, PowerRefusal,
    ProtocolRefusal, Request, UserRow, MAX_EVENT_FRAME, MAX_LOGIN_NAME_BYTES, MAX_REQUEST_FRAME,
    VERSION,
};
pub use secret::{Secret, MAX_SECRET_BYTES};
pub use state::State;
