//! Window overview model: layout, app overview, workspaces bar, selection, keyboard navigation and
//! the gesture-driven transition. Pure logic (no compositor dependency).
//!
//! the transition duration (0.25 s, 2.0 s in slow motion) and the 90 pt workspaces bar
//! band, and the constants of the window packing ([`layout`]); the packing's
//! structure is and every other number is approximate and configurable through
//! [`OverviewConfig`].

pub mod bar;
pub mod config;
pub mod layout;
pub mod overview;
pub mod transition;

pub use bar::{BarConfig, BarHit, BarLayout, BarSlot, WorkspaceBar};
pub use config::{AppOverviewConfig, HighlightConfig, OverviewConfig, TitleFade};
pub use layout::{layout, layout_row, pack, reading_order, LayoutParams, Packing, Rect};
pub use overview::{
    Highlight, Key, Overview, OverviewAction, OverviewMode, OverviewWindow, PlacedWindow,
};
pub use transition::Transition;
