//! Server-side window decorations, renderer-agnostic.
//!
//! * [`metrics`]: titlebar styles (toolbar-less / unified), button kinds, sizes and interaction
//!   settings, each with its evidence label.
//! * [`layout`]: titlebar, content, button and title geometry in logical points.
//! * [`hit`]: hit-testing (buttons, titlebar drag, resize zones outside and inside the geometry,
//!   double-click action, button-group hover).
//! * [`buttons`]: window control button states, colours and the authored vector glyphs.
//! * [`palette`]: colours resolved from the `cupertino-theme` tokens (with a built-in copy).
//! * [`text`]: title text with the fonts of the asset directory and the tracking table of the theme.
//! * [`field`]: the inside of a single-line text field (selection band, text, composition underline, caret), clipped
//!   and scrolled, for the apps' fields.
//! * [`edge`]: the edge of a window or a menu: a fine dark outline outside the shape and a faint light rim inside.
//! * [`shadow`]: window shadow parameters from the verified table (all 32 rows live in
//!   `cupertino_theme::window_shadow`), GPU parameters and a nine-slice cached CPU rasteriser.
//! * [`render`]: [`Decorations`], the façade that lays out, hit-tests and rasterises the titlebar
//!   and rim into premultiplied RGBA [`tiny_skia::Pixmap`]s that a compositor can upload as
//!   textures.
//! * [`preview`]: a composed window image (shadow, content, titlebar, rim) for samples and tests.
//!
//! window corner radii 12 pt / 10 pt, the shadow table and the
//! `ceil(σ·2.8) radius rule, button diameter 12 pt in a 16 pt art box,
//! the outline and inner rim of the edge. Titlebar
//! heights, button spacing, resize zones, glyph proportions and the button colours are
//! approximate and configurable.

pub mod buttons;
pub mod chrome;
pub mod edge;
pub mod field;
pub use cupertino_glyph as glyph;
pub mod hit;
pub mod layout;
pub mod metrics;
pub mod palette;
pub mod preview;
pub mod render;
pub mod shadow;
pub mod shape;
pub mod text;

pub use buttons::{ButtonState, ButtonVisual};
pub use chrome::{
    button_presence, Accessory, AccessoryEdge, ButtonPlacement, Capabilities, ChromeLayout,
    ChromeStyle, Direction, FrameKind, Presence, TitleArrangement, ToolbarStyle,
};
pub use edge::{Edge, EdgeKind, Hairline, OutlineFalloff};
pub use hit::{resize_hit, Hit, ResizeEdge};
pub use layout::{ButtonLayout, FrameLayout};
pub use metrics::{
    ButtonKind, DecorationMetrics, DoubleClickAction, TitleAlignment, TitlebarStyle,
    COMPACT_BUTTON_LEADING, COMPACT_TOOLBAR_ROW_HEIGHT, TITLED_BAR_HEIGHT, TOOLBAR_ROW_HEIGHT,
    WINDOW_BUTTON_DIAMETER, WINDOW_BUTTON_DIAMETER_SMALL, WINDOW_BUTTON_PITCH,
    WINDOW_CORNER_RADIUS,
};
pub use palette::Palette;
pub use render::{DecorationConfig, DecorationState, Decorations};
pub use shadow::{Margins, ShadowGpuParams, ShadowImage, ShadowParams, ShadowRasterizer};
