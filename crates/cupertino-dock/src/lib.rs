//! Dock model: tiles, layout, magnification, bounce animations, hit-testing, minimize targets,
//! drag-reorder and auto-hide, the window feed that drives the tiles ([`session`]), the
//! application switcher's panel ([`switcher`]), the application grid ([`app_grid`]), the overview's
//! chrome and layout ([`overview_ui`]), the context menus ([`menu`]), the hot-corner and gesture
//! actions ([`actions`]), the press and removal effect around a tile drag ([`tile_drag`]), the settings file ([`conf`]) and desktop entries ([`desktop`]). Pure logic
//! (no compositor dependency). The feature `app` adds the `cupertino-dock` process: the bar, the
//! switcher panel, the app grid, the overview and the menus on layer surfaces and popups as a
//! client of the server.
//!
//! The tile
//! layout, margins, section order and drag behaviour are approximate or unknown and configurable.

pub mod actions;
pub mod app_grid;
pub mod autohide;
pub mod bounce;
pub mod conf;
pub mod config;
pub mod desktop;
pub mod dock;
pub mod ease;
pub mod magnify;
pub mod menu;
pub mod overview_ui;
#[cfg(feature = "app")]
pub mod render;
pub mod session;
pub mod switcher;
pub mod tile_drag;
pub mod tiles;

pub use autohide::{autohide_hide_ms, autohide_show_ms, reveal_zone, AutoHide, HideState};
pub use bounce::{Bounce, BounceKind};
pub use config::{
    default_tile_size, sanitize_large_size, sanitize_tile_size, BounceConfig, BounceModel, DockConfig, Hop, LiftFormula, MinimizeEffect,
    Orientation,
};
pub use dock::{Dock, DockLayout, DragOutcome, PlacedTile, TileHit};
pub use ease::{cosine_ease, EasedValue, COSINE_EASE_BEZIER};
pub use magnify::{
    base_length, fish_ramp_ms, fish_ramp_out_ms, fisheye_edge_offset, fisheye_real_scale, layout, layout_items, profile, CosineProfile,
    FisheyeProfile, LayoutItem, MagnificationProfile, TileFrame, FISH_SIZE_TILES,
};
pub use tiles::{Tile, TileId, TileKind, TileLifecycle};
