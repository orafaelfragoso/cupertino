//! The settings application: the pane table and the sidebar
//! order ([`pane`], [`sidebar`]), navigation history ([`history`]), search ([`search`]) and the panes ([`panes`]). The
//! `cupertino-settings` process draws the window with CupertinoKit (`shell`).

pub mod about;
pub mod about_app;
pub mod agent;
pub mod bluez;
pub mod clipboard;
pub mod controls;
pub mod desktop_control;
pub mod geom;
pub mod hardware;
pub mod history;
pub mod hotplug;
pub mod inflight;
pub mod instance;
pub mod live;
pub mod mapping;
pub mod menu_bar;
pub mod meter;
pub mod netinfo;
pub use cupertino_radios::nm;
pub mod pages;
pub mod pane;
pub mod pane_extras;
pub mod pane_more;
pub mod panes;
pub mod screen_sharing;
pub mod search;
pub mod services;
pub mod settings;
pub mod sheet;
pub mod shell;
pub mod sidebar;
pub mod system;
pub mod watch;

pub use geom::{Point, Rect, Size};
pub use pane::PaneId;
