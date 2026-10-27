//! The session manager: the root-owned half of the login. It owns `/run/cupertino-login/greeter.sock`, where the login screen
//! (`cupertino-login --greeter`, an unprivileged compositor client) draws the picker and sends passwords; it runs one worker
//! per authentication attempt and per session; and it owns the console (virtual terminals, the login screen's logind
//! session). The design is.
//!
//! * [`filter`], [`users`], [`picture`], [`roster`]: which accounts the screen lists and with what (pure but for the files).
//! * [`caller`]: who may call the manager's bus interface (pure); feature `app`: [`service`], the interface itself.
//! * [`channel`], [`worker_protocol`]: frames over a socket, and the messages between the manager and a worker.
//! * feature `app`: [`worker`] (the PAM transaction and the privilege drop), [`manager`] (the main loop) and the process.

pub mod auto_login;
pub mod caller;
pub mod channel;
pub mod filter;
pub mod limits;
#[cfg(feature = "app")]
pub mod logind;
#[cfg(feature = "app")]
pub mod manager;
pub mod picture;
pub mod roster;
#[cfg(feature = "app")]
pub mod service;
pub mod users;
#[cfg(feature = "app")]
pub mod vt;
#[cfg(feature = "app")]
pub mod worker;
pub mod worker_protocol;
#[cfg(feature = "app")]
pub mod zeroing;
