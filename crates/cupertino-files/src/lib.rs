//! File manager: the core without a toolkit, and with the `app` feature the `cupertino-files`
//! window process on CupertinoKit ([`kit`]).
//!
//! * [`model`]: asynchronous directory model (batched enumeration, change watching, deltas);
//! * [`natural`], [`sort`], [`group`]: collation and ordering of names, and the date buckets dates read in;
//! * [`settings`]: per-folder view state and its store;
//! * [`selection`], [`rename`], [`typeselect`]: interaction logic;
//! * [`trash`], [`thumbnail`], [`tags`], [`mime`]: freedesktop-compliant services; [`thumbnail_service`]: the
//!   thumbnails of the items in view, made on worker threads; [`open`]: default
//!   applications and the Open With list (`mimeapps.list`);
//! * [`layout`]: icon and list layout engines with the measured metrics of [`metrics`];
//! * [`format`]: how sizes and dates read;
//! * [`places`], [`recents`], [`history`], [`browser`]: the sidebar's standard places, the Recents folder, back / forward history and
//!   the state of one browser window; [`window`]: its size limits, view modes, path bar and status bar text;
//! * [`context_menu`], [`menu_spec`], [`menu_bar`]: the rows of the context menus and the menus of the menu bar;
//! * [`desktop`]: where the desktop folder's items sit on the desktop, and what keeps their places;
//! * [`search_field`]: the toolbar search field's width;
//! * [`columns`]: the column view's chain of folders, their widths and scrolling; [`preview`]: what a preview of a
//!   file draws, its bounded text and the preview window's motion; [`tab_strip`]: a window's tabs;
//! * `udisks` (feature `app`): removable media on the system bus; `notice` (feature `app`): failures as notifications;
//!   `instance` (feature `app`): one Files process per session, serving `org.freedesktop.Application`.

pub mod browser;
pub mod columns;
pub mod context_menu;
pub mod desktop;
pub mod entry;
pub mod format;
pub mod geom;
pub mod group;
pub mod history;
#[cfg(feature = "app")]
pub mod instance;
pub mod item_art;
#[cfg(feature = "app")]
pub mod kit;
pub mod layout;
pub mod menu_bar;
pub mod menu_spec;
pub mod metrics;
pub mod mime;
pub mod model;
pub mod name_edit;
pub mod natural;
#[cfg(feature = "app")]
pub mod notice;
pub mod open;
pub mod operations;
pub mod places;
pub mod preview;
pub mod recents;
pub mod rename;
pub mod search_field;
pub mod selection;
pub mod settings;
pub mod sort;
pub mod spring;
pub mod tab_strip;
pub mod tags;
pub use cupertino_thumbnails as thumbnail;
pub mod thumbnail_service;
pub use cupertino_trash as trash;
pub mod typeselect;
#[cfg(feature = "app")]
pub mod udisks;
pub mod window;
pub mod window_prefs;

pub use browser::{Activation, Browser};
pub use entry::{Entry, Kind};
pub use geom::{Direction, Point, Rect, Size};
pub use model::{DirectoryModel, ModelDelta, ModelOptions};
pub use selection::{Modifiers, Selection};
pub use settings::{FolderKey, SettingsStore, ViewSettings, ViewStyle};
pub use sort::{SortKey, SortSpec};
