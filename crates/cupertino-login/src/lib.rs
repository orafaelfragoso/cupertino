//! Lock screen, greeter and session agent for a Wayland compositor: password entry on `ext-session-lock` surfaces (or, for
//! the login screen, layer surfaces) over a treated wallpaper, and the confirmation panels for log out, restart and shut down.
//!
//! * [`model`]: the state machine (field, attempt phases, shake, fades), pure with injected time.
//! * [`autostart`]: the XDG autostart entries (login items) the session agent starts once per session.
//! * [`hint`]: when failed attempts bring up the password help.
//! * [`layout`]: where the clock, avatar, name and password field sit, in points.
//! * [`picker`]: the greeter's user tiles, grid, selection and the name typed for "Other…".
//! * [`power`]: the greeter's power menu.
//! * [`power_button`]: what a press, a release and a hold of the power button do.
//! * [`render`]: the CPU drawing.
//! * [`spinner`]: the frames and spoke opacities of the progress spinner.
//! * [`panel`]: the confirmation panel's countdown, keys, button frames and words.
//! * [`logout`]: the timing of quitting the applications one at a time before the session ends.
//! * [`interrupt`]: the panel that shows when an application does not quit: Cancel or Force Quit.
//! * [`low_battery`]: which low-battery notification to post, replace or withdraw as the warning level changes.
//! * [`user`]: the user's name and picture, and the clock text.
//! * feature `dbus`: [`control`] (the `org.cupertino.Login1` bus interface), [`logind`] (the session's lock request, suspend
//!   preparation and the call that ends the session), [`login_manager`] (the session manager's `org.cupertino.LoginManager1`: power
//!   actions, inhibitor locks, the session's announcement) and
//!   [`upower`] (the display device's warning level and the notification calls) and [`session`] (the server's
//!   `org.cupertino.Session1`, which quits the applications).
//! * feature `app`: [`auth`] (PAM), [`manager`] (the greeter's connection to the session manager) and the `cupertino-login`
//!   process.
//!
//! Sizes, positions, the shake, the fades and the hide timer come from the analysis; the wallpaper source, the field material, the dots and the notice are approximate, each
//! labelled where it is defined.

#[cfg(feature = "app")]
pub mod auth;
pub mod autostart;
#[cfg(feature = "dbus")]
pub mod control;
pub mod hint;
pub mod interrupt;
pub mod layout;
#[cfg(feature = "dbus")]
pub mod logind;
#[cfg(feature = "dbus")]
pub mod login_manager;
#[cfg(feature = "app")]
pub mod manager;
pub mod logout;
pub mod low_battery;
pub mod model;
pub mod panel;
pub mod picker;
pub mod power;
pub mod power_button;
pub mod render;
pub mod spinner;
#[cfg(feature = "dbus")]
pub mod session;
#[cfg(feature = "dbus")]
pub mod upower;
pub mod user;

pub use model::{AuthOutcome, Key, LockScreen, NoAttempt, Password, Phase};
