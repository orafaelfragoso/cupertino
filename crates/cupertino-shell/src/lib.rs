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

/// The launcher's panel and its application browser (`cupertino-launcher`).
pub static LAUNCHER: Entry = Entry::of::<cupertino_launcher::component::Launcher>();

/// The session agent: restore, login items, the session's panels, the power key and the session menu
/// (`cupertino-shell-agent`).
pub static AGENT: Entry = Entry::of::<cupertino_shell_agent::Agent>();

/// The items of the Desktop folder on the desktop (`cupertino-shell-desktop`).
pub static DESKTOP: Entry = Entry::of::<cupertino_shell_desktop::Desktop>();

/// The overview: the windows in holes of a surface over the output, the desktops band, opened and closed with the
/// gestures' transactions (`cupertino-shell-overview`).
pub static OVERVIEW: Entry = Entry::of::<cupertino_shell_overview::Overview>();

/// The dock: the bar of applications, folders and the trash, the strips that move it to another output, the Force Quit
/// dialog and the application switcher's panel (`cupertino-dock`).
pub static DOCK: Entry = Entry::of::<cupertino_dock::Dock>();

/// The window manager: the stage policy of each output, placed in transactions, and the stage strip, whose thumbnails
/// are holes claiming the strip's windows (`cupertino-windowmanager`).
pub static WINDOWMANAGER: Entry = Entry::of::<cupertino_windowmanager::WindowManager>();

/// The portal's file choosers and app choosers, `org.cupertino.Dialogs1` (`cupertino-shell-dialogs`).
pub static DIALOGS: Entry = Entry::of::<cupertino_shell_dialogs::Dialogs>();

/// The components this build of the host runs. A component the server lists that is not here is answered
/// `unavailable` (`absent`), and the server runs it the way the rest of its configuration says.
pub static COMPONENTS: [&Entry; 11] = [
    &OSD,
    &NOTIFICATIONS,
    &CONTROLCENTER,
    &MENUBAR,
    &LAUNCHER,
    &AGENT,
    &DESKTOP,
    &OVERVIEW,
    &DOCK,
    &WINDOWMANAGER,
    &DIALOGS,
];
