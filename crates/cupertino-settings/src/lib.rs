//! The settings application: the pane table and the sidebar
//! order ([`pane`], [`sidebar`]), navigation history ([`history`]), search ([`search`]) and the panes ([`panes`]). The
//! `cupertino-settings` process (feature `app`) draws the window with CupertinoKit (`shell`).

pub mod about;
#[cfg(feature = "app")]
pub mod about_app;
#[cfg(feature = "app")]
pub mod agent;
#[cfg(feature = "app")]
pub mod bluez;
pub mod clipboard;
pub mod controls;
pub mod desktop_control;
pub mod geom;
pub mod hardware;
pub mod history;
#[cfg(feature = "app")]
pub mod inflight;
#[cfg(feature = "app")]
pub mod instance;
pub mod live;
pub mod mapping;
pub mod menu_bar;
#[cfg(feature = "app")]
pub mod nm;
#[cfg(feature = "app")]
pub mod output_config;
pub mod pages;
pub mod pane;
pub mod panes;
pub mod screen_sharing;
pub mod search;
#[cfg(feature = "app")]
pub mod services;
pub mod settings;
pub mod sheet;
#[cfg(feature = "app")]
pub mod shell;
pub mod sidebar;

pub use geom::{Point, Rect, Size};
pub use pane::PaneId;
