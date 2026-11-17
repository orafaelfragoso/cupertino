//! Menu bar model for the Cupertino compositor (no Smithay dependency).
//!
//! * [`layout`]: bar slots (bold app-name menu, menus, status items, control panel, clock) and
//!   pull-down popups, with text measurement behind the [`layout::TextMeasure`] trait.
//! * [`controller`]: the interaction state machine (press-drag-release, click-sticky menus, hover
//!   switching, keyboard navigation, type-ahead, submenus with a safe triangle).
//! * [`fallback`]: synthetic app and Window menus for apps that export none.
//! * [`merge`]: an exporter's menus merged with the bar's App, Window and Help menus (roles, rebinding, order).
//! * [`search`]: the Help menu's search over the items of the bar and the Help menu that shows its results.
//! * [`clock`]: the clock text and its ticks; its options are `menubar.conf`, read and written by
//!   `cupertino_defaults::menubar`.
//! * [`extras`]: the bar's own status items (Wi-Fi, battery, Bluetooth, Focus) and their menus, and the system menu.
//! * [`recent`]: what Recent Items of the system menu lists (the launcher's applications, the recently-used documents)
//!   and its Clear Menu time.
//! * [`registered`]: the menus registered on the session bus and which belongs to the active window.
//! * [`source`]: window / app identity → menu source (KDE appmenu service + path, GTK bus name and
//!   object paths).
//! * [`tone`]: the colours of the bar and its menus, and the tone and scrim of the bar's text over the wallpaper.
//! * [`dbus`]: DBusMenu client, StatusNotifier watcher / item client,
//!   GMenuModel reader.
//! * the logic of the `cupertino-menubar` process (one bar per output as a client
//!   of the server): [`feed`] (what the bars show, from the server's groups and the bus),
//!   [`routing`] (keys and controller effects to actions), [`popups`] (placement of the menus'
//!   popups), [`view`] (the bar and its menus as CupertinoKit windows), [`desktop`] (application names),
//!   [`worker`] (the session bus thread), [`follow`] (a DBusMenu exporter's tree kept current), [`extras_bus`] (the system services behind the extras) and [`x11`] (the X
//!   server of the X11 bridge, for menus X11 programs register).
//!
//! the bar structure and the interaction model are as designed behaviour of the
//! design; the selection highlight corner radius (4 pt,
//! continuous) is as designed; the reveal and hide delays of the auto-hidden bar are unknown
//! (configurable placeholders); every metric, timer and the safe-triangle rule are approximate and
//! configurable. The D-Bus interfaces are open Canonical, KDE and GNOME interfaces.

pub mod clock;
pub mod controller;
pub mod dbus;
pub mod extras;
pub mod fallback;
pub mod geom;
pub mod item;
pub mod layout;
pub mod merge;
pub mod recent;
pub mod registered;
pub mod search;
pub mod source;
pub mod tone;
pub mod wakeup;

pub mod desktop;
pub mod extras_bus;
pub mod feed;
pub mod follow;
pub mod popups;
pub mod routing;
pub mod view;
pub mod worker;
pub mod x11;
