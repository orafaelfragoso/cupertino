//! Lock screen, greeter and session agent for a Wayland compositor: password entry on `ext-session-lock` surfaces (or, for
//! the login screen, layer surfaces) over a treated wallpaper, and the confirmation panels for log out, restart and shut down.
//!
//! * [`model`]: the state machine (field, attempt phases, shake, fades), pure with injected time.
//! * [`capabilities`]: which session menu actions work on this machine; [`probe`]: reading logind for them off the main loop.
//! * [`autostart`]: the XDG autostart entries (login items) the session agent starts once per session.
//! * [`hint`]: when failed attempts bring up the password help.
//! * [`layout`]: where the clock, avatar, name and password field sit, in points.
//! * [`lock_notices`]: the notifications the lock screen shows, as the notification daemon lists them, and their cards'
//!   frames; with feature `dbus`, reading and following the list.
//! * [`login_wallpaper`]: the copy of the session's wallpaper the session agent leaves for the login screen.
//! * [`picker`]: the greeter's user tiles, grid, selection and the name typed for "Other…".
//! * [`power`]: the greeter's power menu.
//! * [`power_button`]: what a press, a release and a hold of the power button do.
//! * [`view`]: the screens and panels as CupertinoKit draws them, and what the person does on them.
//! * [`spinner`]: the frames and spoke opacities of the progress spinner.
//! * [`status`]: what the status items at the top right of the lock and login screens show, read every few seconds.
//! * [`panel`]: the confirmation panel's countdown, keys, button frames and words.
//! * [`interrupt`]: the panel that shows when an application does not quit: Try Again, Cancel or Force Quit.
//! * [`restore`]: reopening applications at the next login: the saved choice and list, and starting the list once.
//! * [`low_battery`]: which low-battery notification to post, replace or withdraw as the warning level changes.
//! * [`user`]: the user's name and picture, and the clock text.
//! * [`wallpaper`]: the picture the screens stand on, dimmed, made once per output, and the control material under the
//!   field.
//! * feature `dbus`: [`control`] (the `org.cupertino.Login1` bus interface), [`logind`] (the session's lock request, suspend
//!   preparation and the call that ends the session), [`login_manager`] (the session manager's `org.cupertino.LoginManager1`: power
//!   actions, inhibitor locks, the session's announcement) and
//!   [`upower`] (the display device's warning level and the notification calls) and [`session`] (the server's
//!   `org.cupertino.Session1`, which carries out the log out, restart, shut down and sleep and quits the applications).
//! * feature `app`: [`auth`] (PAM), [`manager`] (the greeter's connection to the session manager) and the `cupertino-login`
//!   process.
//!
//! Sizes, positions, the shake, the fades and the hide timer come from the analysis; the wallpaper source and the notice
//! are approximate, each labelled where it is defined.

#[cfg(feature = "app")]
pub mod auth;
pub mod autostart;
pub mod capabilities;
#[cfg(feature = "dbus")]
pub mod control;
pub mod hint;
pub mod interrupt;
pub mod layout;
pub mod lock_notices;
#[cfg(feature = "dbus")]
pub mod login_manager;
pub mod login_wallpaper;
#[cfg(feature = "dbus")]
pub mod logind;
pub mod low_battery;
#[cfg(feature = "app")]
pub mod manager;
pub mod model;
pub mod panel;
pub mod picker;
pub mod power;
pub mod power_button;
pub mod probe;
pub mod restore;
#[cfg(feature = "dbus")]
pub mod session;
pub mod spinner;
pub mod status;
#[cfg(feature = "dbus")]
pub mod upower;
pub mod user;
pub mod view;
pub mod wallpaper;

pub use model::{AuthOutcome, Key, LockScreen, NoAttempt, Password, Phase};
