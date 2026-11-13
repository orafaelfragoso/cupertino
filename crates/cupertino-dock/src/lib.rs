//! Dock model: tiles, layout, magnification, bounce animations, hit-testing, minimize targets,
//! drag-reorder and auto-hide, the window feed that drives the tiles ([`session`]), the
//! application switcher's panel ([`switcher`]), the overview's
//! chrome and layout ([`overview_ui`]), the context menus ([`menu`]), the hot-corner and gesture
//! actions ([`actions`]), the press and removal effect around a tile drag ([`tile_drag`]), the settings file ([`conf`]) and desktop entries ([`desktop`]). Pure logic
//! (no compositor dependency). The dock as a CupertinoKit application is [`kit`] with its windows ([`view`],
//! [`switcher_kit`], [`overview_kit`]). The feature `app` adds the `cupertino-dock` process: those windows on layer
//! surfaces and popups as a client of the server, with the icons it loads ([`icons`]) and the D-Bus activation that
//! asks a resident application for a window ([`activate`]).
//!
//! The tile
//! layout, margins, section order and drag behaviour are approximate or unknown and configurable.

pub mod actions;
#[cfg(feature = "app")]
pub use cupertino_apps::activate;
pub mod autohide;
pub mod badge;
pub mod bounce;
pub mod conf;
pub mod config;
pub mod desktop;
pub mod dock;
pub mod ease;
pub mod fan;
pub mod force_quit;
#[cfg(feature = "app")]
pub mod icons;
pub mod kit;
pub mod magnify;
pub mod menu;
pub mod minimized;
pub mod overview_kit;
pub mod overview_ui;
pub mod session;
pub mod stack;
pub mod switcher;
pub mod switcher_kit;
pub mod tile_drag;
pub mod tiles;
pub mod view;
pub mod wm_kit;

pub use autohide::{autohide_hide_ms, autohide_show_ms, reveal_zone, AutoHide, HideState};
pub use bounce::{Bounce, BounceKind};
pub use config::{
    default_tile_size, sanitize_large_size, sanitize_tile_size, BounceConfig, BounceModel,
    DockConfig, Hop, LiftFormula, MinimizeEffect, Orientation,
};
pub use dock::{Dock, DockLayout, DragOutcome, PlacedTile, TileHit};
pub use ease::{cosine_ease, EasedValue, COSINE_EASE_BEZIER};
pub use magnify::{
    base_length, fish_ramp_ms, fish_ramp_out_ms, fisheye_edge_offset, fisheye_real_scale, layout,
    layout_items, profile, CosineProfile, FisheyeProfile, LayoutItem, MagnificationProfile,
    TileFrame, FISH_SIZE_TILES,
};
pub use tiles::{Tile, TileId, TileKind, TileLifecycle};
