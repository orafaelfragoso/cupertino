//! The settings application without a toolkit: the pane table and the sidebar
//! order ([`pane`], [`sidebar`]), navigation history ([`history`]), the keyboard ([`keys`]), search ([`search`]), the
//! window layout ([`window`]) and its painter ([`paint`]). The `cupertino-settings` process (feature `app`) puts the
//! window on the server.

pub mod about;
#[cfg(feature = "app")]
pub mod agent;
#[cfg(feature = "app")]
pub mod bluez;
pub mod clipboard;
pub mod controls;
pub mod form;
pub mod geom;
pub mod hardware;
pub mod history;
#[cfg(feature = "app")]
pub mod inflight;
#[cfg(feature = "app")]
pub mod instance;
pub mod keys;
pub mod live;
pub mod mapping;
pub mod menu;
pub mod menu_bar;
#[cfg(feature = "app")]
pub mod nm;
pub mod paint;
pub mod pane;
pub mod panes;
pub mod search;
#[cfg(feature = "app")]
pub mod services;
pub mod settings;
pub mod sheet;
#[cfg(feature = "app")]
pub mod shell;
pub mod sidebar;
pub mod window;

pub use geom::{Point, Rect, Size};
pub use pane::PaneId;
