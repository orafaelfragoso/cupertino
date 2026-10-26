//! Menu bar model for the Cupertino compositor (no Smithay dependency).
//!
//! * [`layout`]: bar slots (bold app-name menu, menus, status items, control panel, clock) and
//!   pull-down popups, with text measurement behind the [`layout::TextMeasure`] trait.
//! * [`controller`]: the interaction state machine (press-drag-release, click-sticky menus, hover
//!   switching, keyboard navigation, type-ahead, submenus with a safe triangle).
//! * [`fallback`]: synthetic app and Window menus for apps that export none.
//! * [`clock`]: the clock text and its ticks; its options are `menubar.conf`, read and written by
//!   `cupertino_defaults::menubar`.
//! * [`extras`]: the bar's own status items (Wi-Fi, battery, Bluetooth, Focus) and their menus.
//! * [`source`]: window / app identity → menu source (KDE appmenu service + path, GTK bus name and
//!   object paths).
//! * `dbus` (feature `dbus`, on by default): DBusMenu client, StatusNotifier watcher / item client,
//!   GMenuModel reader.
//! * feature `app`, the logic of the `cupertino-menubar` process (one bar per output as a client
//!   of the server): [`feed`] (what the bars show, from the server's groups and the bus),
//!   [`routing`] (keys and controller effects to actions), [`popups`] (placement of the popup
//!   surfaces), [`raster`] (text and drawing), [`desktop`] (application names), [`worker`] (the session bus
//!   thread) and [`extras_bus`] (the system services behind the extras).
//!
//! the bar structure and the interaction model are as designed behaviour of the
//! design; the selection highlight corner radius (4 pt,
//! continuous) is as designed; the reveal and hide delays of the auto-hidden bar are unknown
//! (configurable placeholders); every metric, timer and the safe-triangle rule are approximate and
//! configurable. The D-Bus interfaces are open Canonical, KDE and GNOME interfaces.

pub mod clock;
pub mod controller;
#[cfg(feature = "dbus")]
pub mod dbus;
pub mod extras;
pub mod fallback;
pub mod geom;
pub mod item;
pub mod layout;
pub mod source;

#[cfg(feature = "app")]
pub mod desktop;
#[cfg(feature = "app")]
pub mod extras_bus;
#[cfg(feature = "app")]
pub mod feed;
#[cfg(feature = "app")]
pub mod popups;
#[cfg(feature = "app")]
pub mod raster;
#[cfg(feature = "app")]
pub mod routing;
#[cfg(feature = "app")]
pub mod worker;
