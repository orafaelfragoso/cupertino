//! The dock, a component of the shell host ([`Dock`], [`component`]): the bar of applications, folders and the trash
//! along an edge of one output, the strips that move it to another, the context menus, the Force Quit dialog and the
//! application switcher's panel.
//!
//! The model is pure logic (no compositor dependency): tiles, layout, magnification, bounce animations, hit-testing,
//! minimize targets, drag-reorder and auto-hide ([`dock`]), the window feed that drives the tiles ([`session`]), the
//! application switcher's panel ([`switcher`]), the context menus ([`menu`]), the hot-corner and gesture actions
//! ([`actions`]), the press and removal effect around a tile drag ([`tile_drag`]), the settings file ([`conf`]) and
//! desktop entries ([`desktop`]). The dock as a CupertinoKit application is [`kit`] with its windows ([`view`],
//! [`switcher_kit`]); [`component`] runs it on the shell's roles, with the icons it loads ([`icons`]).
//!
//! The tile layout, margins, section order and drag behaviour are approximate or unknown and configurable.

#![forbid(unsafe_code)]

pub mod actions;
pub mod autohide;
pub mod badge;
pub mod bounce;
pub mod component;
pub mod conf;
pub mod config;
pub mod desktop;
pub mod dock;
pub mod ease;
pub mod fan;
pub mod force_quit;
pub mod icons;
pub mod kit;
pub mod magnify;
pub mod menu;
pub mod minimized;
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
pub use component::Dock;
pub use config::{
    default_tile_size, sanitize_large_size, sanitize_tile_size, BounceConfig, BounceModel,
    DockConfig, Hop, LiftFormula, MinimizeEffect, Orientation,
};
pub use dock::{DockLayout, DragOutcome, PlacedTile, TileHit};
pub use ease::{cosine_ease, EasedValue, COSINE_EASE_BEZIER};
pub use magnify::{
    base_length, fish_ramp_ms, fish_ramp_out_ms, fisheye_edge_offset, fisheye_real_scale, layout,
    layout_items, profile, CosineProfile, FisheyeProfile, LayoutItem, MagnificationProfile,
    TileFrame, FISH_SIZE_TILES,
};
pub use tiles::{Tile, TileId, TileKind, TileLifecycle};
