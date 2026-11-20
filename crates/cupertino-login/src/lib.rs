//! Lock screen and greeter for a Wayland compositor: password entry on `ext-session-lock` surfaces (or, for the login
//! screen, layer surfaces) over a treated wallpaper. The session agent is the shell host's `agent` component
//! (`cupertino-shell-agent`).
//!
//! * [`model`]: the state machine (field, attempt phases, shake, fades), pure with injected time.
//! * [`hint`]: when failed attempts bring up the password help.
//! * [`layout`]: where the clock, avatar, name and password field sit, in points.
//! * [`lock_notices`]: the notifications the lock screen shows, as the notifications component lists them, and their cards'
//!   frames; with feature `dbus`, reading and following the list.
//! * [`login_wallpaper`]: the copy of the session's wallpaper the lock screen leaves for the login screen.
//! * [`picker`]: the greeter's user tiles, grid, selection and the name typed for "Other…".
//! * [`power`]: the greeter's power menu.
//! * [`view`]: the screens and the consent panel as CupertinoKit draws them, and what the person does on them.
//! * [`spinner`]: the frames and spoke opacities of the progress spinner.
//! * [`status`]: what the status items at the top right of the lock and login screens show, read again whenever what they show changes.
//! * [`user`]: the user's name and picture, and the clock text.
//! * [`wallpaper`]: the picture the screens stand on, dimmed, made once per output, and the control material under the
//!   field.
//! * feature `dbus`: [`control`] (the `org.cupertino.Login1` bus interface, `Lock` alone), [`logind`] (the session's lock
//!   request, suspend preparation and the sleep inhibitor) and [`login_manager`] (the session manager's
//!   `org.cupertino.LoginManager1`: the session's announcement and the `Unlock` of a fast user switch).
//! * feature `app`: the two programs of the `cupertino-login` process on `cupertino-kit-shell`, [`lock_screen`] (a user's
//!   session) and [`greeter`] (the login screen, `--greeter`), their command line ([`options`]), [`auth`] (PAM, on
//!   Kit's I/O pool) and [`manager`] (the login screen's connection to the session manager, a future of its loop).
//!
//! Sizes, positions, the shake, the fades and the hide timer come from the analysis; the wallpaper source and the notice
//! are approximate, each labelled where it is defined.

#![forbid(unsafe_code)]

#[cfg(feature = "app")]
pub mod auth;
#[cfg(feature = "dbus")]
pub mod control;
#[cfg(feature = "app")]
pub mod greeter;
pub mod hint;
pub mod layout;
pub mod lock_notices;
#[cfg(feature = "app")]
pub mod lock_screen;
#[cfg(feature = "dbus")]
pub mod login_manager;
pub mod login_wallpaper;
#[cfg(feature = "dbus")]
pub mod logind;
#[cfg(feature = "app")]
pub mod manager;
pub mod model;
#[cfg(feature = "app")]
pub mod options;
pub mod picker;
pub mod power;
#[cfg(feature = "app")]
mod screen;
pub mod spinner;
pub mod status;
pub mod user;
pub mod view;
pub mod wallpaper;

pub use model::{AuthOutcome, Key, LockScreen, NoAttempt, Password, Phase};
