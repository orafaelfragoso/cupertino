//! The shell host (`docs/architecture/shell-host.md`): the process the server starts on a connection with the `host`
//! role alone, which runs the components the server lists, each a Kit instance on a thread of its own with its own
//! Wayland and bus connections, and restarts a failed one alone.
//!
//! * [`run`]: the host program: it connects on `WAYLAND_SOCKET`, asks the server for the connection of every listed
//!   component it was built with and answers `unavailable` for the others, starts each on its connection
//!   (`hosting::Runtime::start`), and follows the server's lists, restarts and quiet moment until the server says to
//!   exit or its connection ends.
//! * [`watchdog`]: what the host reports of a component's loop that has been out of its wait (D17).
//! * [`COMPONENTS`]: the components this build of the host runs, by the names of the server's table.
//!
//! The host starts no thread and opens no socket itself: `cupertino-kit-shell`'s `hosting` does that, and the host
//! keeps the policy. A component that fails is torn down by the runtime inside `catch_unwind`; the host shuts its
//! sockets, sends `failed` and then `connect`, and the server answers once the component's restart time has passed
//! (D16, whose times and counts the server keeps).

#![forbid(unsafe_code)]

mod host;
pub mod watchdog;

use cupertino_kit_shell::Entry;

pub use host::run;

/// The on-screen displays (`cupertino-osd`).
pub static OSD: Entry = Entry::of::<cupertino_osd::Osd>();

/// The banners and the notification list (`cupertino-notifications`).
pub static NOTIFICATIONS: Entry = Entry::of::<cupertino_notifications::Notifications>();

/// The control panel (`cupertino-controlcenter`).
pub static CONTROLCENTER: Entry = Entry::of::<cupertino_controlcenter::ControlCenter>();

/// The menu bar of every output (`cupertino-menubar`).
pub static MENUBAR: Entry = Entry::of::<cupertino_menubar::MenuBar>();

/// The items of the Desktop folder on the desktop (`cupertino-shell-desktop`).
pub static DESKTOP: Entry = Entry::of::<cupertino_shell_desktop::Desktop>();

/// The components this build of the host runs. A component the server lists that is not here is answered
/// `unavailable` (`absent`), and the server runs it the way the rest of its configuration says.
pub static COMPONENTS: [&Entry; 5] = [&OSD, &NOTIFICATIONS, &CONTROLCENTER, &MENUBAR, &DESKTOP];
