//! The session agent of the Cupertino desktop, a component of the shell host (`cupertino-shell`, component `agent`):
//! what the session does at its start and at its end besides the windows.
//!
//! * At login: the login items ([`autostart`]) once per session, then the applications the last session saved to
//!   reopen ([`restore`]), each once, never a program of the shell itself.
//! * At its end: the log out, restart and shut down panels ([`panel`]), the power-button panel and the key's hold
//!   ([`power_button`]), and the panel that interrupts an end an application held up ([`interrupt`]); the end itself
//!   is the server's `org.cupertino.Session1` ([`session`]), which quits the applications first.
//! * Throughout: the low-battery notifications ([`low_battery`]) and the session menu's state ([`capabilities`]),
//!   which it publishes on the host bus as `SessionMenu` for the menu bar.
//! * [`control`]: `org.cupertino.SessionAgent1`, what the server asks of the agent: the panels, the power key and a
//!   new reading of the capabilities.
//! * [`system`]: logind, the session manager, the power daemon and the notification service, on the system and
//!   session buses.
//! * [`view`]: the panels as CupertinoKit draws them.
//! * [`Agent`]: the component.
//!
//! The panels' sizes, words and countdown come from the analysis; each approximate value is labelled where it is
//! defined.

#![forbid(unsafe_code)]

pub mod autostart;
pub mod capabilities;
mod component;
pub mod control;
pub mod interrupt;
pub mod low_battery;
pub mod panel;
pub mod power_button;
pub mod restore;
pub mod session;
mod startup;
pub mod system;
pub mod view;

pub use component::{Agent, Message, Model};
