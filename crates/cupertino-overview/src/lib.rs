//! Window overview model: layout, app overview, workspaces bar, selection, keyboard navigation and
//! the gesture-driven transition. Pure logic (no compositor dependency).
//!
//! The values (the transition's spring, the desktops bar's band, the packing's constants) are in
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
