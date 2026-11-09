//! Typed access to the design tokens, material recipes and animation presets of the theme data
//! (`data/theme/{tokens,materials,animations}.json`).
//!
//! * [`Theme::load`] reads the files from `$CUPERTINO_SPEC_DIR`, defaulting to
//!   `../../data/theme` relative to this crate; every loader also has a `from_str`
//!   constructor.
//! * [`Appearance`] selects one of four catalogs (light/dark, normal/high contrast).
//! * [`tokens::DesignTokens`]: semantic colours (sRGB `f32` + alpha) by name.
//! * [`metrics`]: control alignment heights per size class and shape tokens, as data.
//! * [`control_sizes`]: each control family's height, fixed width, corners and label room at the five control sizes,
//!   with the normal and the compact metrics.
//! * [`typography::Typography`]: substitute font family, size presets, the tracking table with
//!   linear interpolation and line metrics.
//! * [`animation::AnimationPresets`]: springs, timing functions, momentum and rubber-band settings
//!   converted to `cupertino-animation` types.
//! * [`window_shadow`]: the verified window shadow parameter table (32 rows) and the window corner radii.
//! * [`recipe`]: material recipes (backdrop filter chain + fill layers) including opaque variants
//!   for reduce-transparency and the high-contrast catalogs.
//! * [`glass`]: glass recipes (background pass and foreground matrix) per appearance, style, accessibility option and
//!   shape size.

pub mod animation;
pub mod appearance;
pub mod color;
pub mod control_sizes;
pub mod error;
pub mod evidence;
pub mod glass;
pub mod metrics;
pub mod recipe;
pub mod theme;
pub mod tokens;
pub mod typography;
pub mod window_shadow;

pub use animation::{AnimationPresets, Easing};
pub use appearance::Appearance;
pub use color::{AccentColor, Color, SemanticColor};
pub use control_sizes::{control_geometry, ControlFamily, ControlGeometry, Corners};
pub use error::{Error, Result};
pub use evidence::Evidence;
pub use glass::{GlassMode, GlassParams, GlassStyle};
pub use metrics::{ControlSize, ShapeTokens, WindowShadowPreset};
pub use recipe::{
    Backdrop, Blend, Catalog, ColorOp, CornerCurve, Fill, Interaction, MaterialLibrary,
    MaterialOptions, Recipe, WindowState,
};
pub use theme::{spec_dir, Theme, SPEC_DIR_ENV};
pub use tokens::DesignTokens;
pub use typography::Typography;
pub use window_shadow::{
    rim_radius, sigma_to_radius_px, window_shadow, ShadowOption, WindowCornerSize,
    WindowShadowEntry, SHADOW_SIGMA_MAXIMUM, SIGMA_TO_RADIUS_FACTOR, WINDOW_CORNER_RADIUS_REGULAR,
    WINDOW_CORNER_RADIUS_SMALL, WINDOW_SHADOW_TABLE,
};
