//! File manager: the core without a toolkit, and with the `app` feature the `cupertino-files`
//! window process.
//!
//! * [`access`]: what the window tells assistive technology;
//! * [`model`]: asynchronous directory model (batched enumeration, change watching, deltas);
//! * [`natural`], [`sort`], [`group`]: collation, ordering and sectioning of names;
//! * [`settings`]: per-folder view state and its store;
//! * [`selection`], [`rename`], [`typeselect`], [`spring`]: interaction logic;
//! * [`trash`], [`thumbnail`], [`tags`], [`mime`]: freedesktop-compliant services; [`open`]: default
//!   applications and the Open With list (`mimeapps.list`);
//! * [`layout`]: icon, list, column and gallery layout engines with the measured metrics of
//!   [`metrics`]; [`preview`]: quick-preview transition geometry;
//! * [`render`]: CPU rendering of the icon and list views;
//! * [`places`], [`recents`], [`history`], [`browser`]: the sidebar's standard places, the Recents folder, back / forward history and
//!   the state of one browser window; [`window`]: its layout, hit testing and drawing; [`sidebar_keys`]: the sidebar's
//!   keyboard navigation;
//! * `udisks` (feature `app`): removable media on the system bus; `notice` (feature `app`): failures as notifications.

pub mod access;
pub mod browser;
pub mod context_menu;
pub mod entry;
pub mod geom;
pub mod group;
pub mod history;
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
pub mod render;
pub mod selection;
pub mod settings;
pub mod sidebar_keys;
pub mod sort;
pub mod spring;
pub mod tags;
pub use cupertino_thumbnails as thumbnail;
pub mod trash;
pub mod typeselect;
#[cfg(feature = "app")]
pub mod udisks;
pub mod window;

pub use browser::{Activation, Browser};
pub use entry::{Entry, Kind};
pub use geom::{Direction, Point, Rect, Size};
pub use model::{DirectoryModel, ModelDelta, ModelOptions};
pub use selection::{Modifiers, Selection};
pub use settings::{FolderKey, SettingsStore, ViewSettings, ViewStyle};
pub use sort::{SortKey, SortSpec};
