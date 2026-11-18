//! File manager: the core, and with the `app` feature the `cupertino-files` window process on CupertinoKit
//! ([`kit`]). The items, their order, types and tags, the icon layout, opening, renaming and the file operations are
//! Kit's file family (`cupertino_kit::files`), and so are a folder's listing, its places, the items' icons and their
//! thumbnails, and the notices.
//!
//! * [`group`]: the date buckets dates read in;
//! * [`settings`]: per-folder view state and its store;
//! * [`selection`], [`typeselect`]: interaction logic;
//! * [`trash`]: freedesktop-compliant trash;
//! * [`layout`]: the list layout engine with the measured metrics of [`metrics`];
//! * [`format`]: how sizes and dates read;
//! * [`history`], [`browser`]: back / forward history and the state of one browser window (the sidebar's places are
//!   the family's, `cupertino_kit::files::places`); [`window`]: its size limits, view modes, path bar and status bar text;
//! * [`context_menu`], [`menu_spec`], [`menu_bar`]: the rows of the context menus and the menus of the menu bar;
//! * [`desktop`]: where the desktop folder's items sit on the desktop, and what keeps their places;
//! * [`search_field`]: the toolbar search field's width;
//! * [`columns`]: the column view's chain of folders, their widths and scrolling; [`preview`]: what a preview of a
//!   file draws, its bounded text and the preview window's motion; [`tab_strip`]: a window's tabs;
//! * `udisks` (feature `app`): removable media on the system bus; failures as notifications are the family's
//!   (`cupertino_kit::files::notice`); `instance` (feature `app`): one Files process per session, serving `org.freedesktop.Application`.

pub mod browser;
pub mod columns;
pub mod context_menu;
pub mod desktop;
pub mod format;
pub mod group;
pub mod history;
#[cfg(feature = "app")]
pub mod instance;
#[cfg(feature = "app")]
pub mod kit;
pub mod layout;
pub mod menu_bar;
pub mod menu_spec;
pub mod metrics;
pub mod preview;
pub mod search_field;
pub mod selection;
pub mod settings;
pub mod spring;
pub mod tab_strip;
pub use cupertino_trash as trash;
pub mod typeselect;
#[cfg(feature = "app")]
pub mod udisks;
pub mod window;
pub mod window_prefs;

pub use browser::{Activation, Browser};
pub use selection::{Modifiers, Selection};
pub use settings::{FolderKey, SettingsStore, ViewSettings, ViewStyle};
