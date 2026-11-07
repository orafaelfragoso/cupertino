//! Values the analysis did not recover are approximate and
//! say so; the README lists them.

/// A straight-alpha sRGB colour, red, green, blue and alpha from 0 to 1.
pub type Rgba = [f32; 4];

// ---- floating thumbnail ------------------------------------------------------------------------------------------

/// Time the thumbnail stays before it commits the capture (5.0 s).
pub const THUMBNAIL_EXPIRATION: f64 = 5.0;
/// Dismiss animation without a swipe (0.5 s, ease-in-ease-out).
pub const THUMBNAIL_DISMISS: f64 = 0.5;
/// Shortest swipe-dismiss animation and the factor of `|travel / velocity|
/// (`min(0.5, max(0.08, 0.015 * |2w / v|))).
pub const SWIPE_DISMISS_MIN: f64 = 0.08;
pub const SWIPE_DISMISS_FACTOR: f64 = 0.015;
/// Thumbnail size as a fraction of the first screen (0.1).
pub const THUMBNAIL_SCALE: f64 = 0.1;
/// The short side never gets below this fraction of the long one.
pub const THUMBNAIL_MIN_ASPECT: f64 = 0.25;
/// Distance from the screen edges (margin 16 on the default right side, y = 16 from the bottom).
pub const THUMBNAIL_MARGIN: f64 = 16.0;
/// Corner radius of the background.
pub const THUMBNAIL_RADIUS: f64 = 4.0;
/// Border of the image (0.5 pt). Its colour is approximate: [`THUMBNAIL_BORDER_COLOR`].
pub const THUMBNAIL_BORDER: f64 = 0.5;
pub const THUMBNAIL_BORDER_COLOR: Rgba = [0.0, 0.0, 0.0, 0.25];
/// Room around the thumbnail for its shadow (section 6: nine-slice outset 56 pt;
/// not in the register's measurements).
pub const THUMBNAIL_SHADOW_OUTSET: f64 = 56.0;
/// Peak alpha of the shadow asset (42/255). Its falloff is drawn as the toolkit's drop shadow with a blur of
/// [`THUMBNAIL_SHADOW_BLUR`] (approximate: the asset's profile is not known).
pub const THUMBNAIL_SHADOW_ALPHA: f32 = 42.0 / 255.0;
pub const THUMBNAIL_SHADOW_BLUR: f64 = 12.0;
/// Pointer travel on both axes under which a drag is ignored (5.0).
pub const DRAG_SLOP: f64 = 5.0;
/// Horizontal drag past which a release swipes the thumbnail away (+-30.0).
pub const SWIPE_THRESHOLD: f64 = 30.0;

// ---- selection overlay -------------------------------------------------------------------------------------------

/// Dim over the screen outside the selection of the interactive rectangle (black at 0.5).
pub const DIM: Rgba = [0.0, 0.0, 0.0, 0.5];
/// Border of the interactive selection: a black line under a white dashed one (1.0 pt,
/// dash [4, 4], round join).
pub const BORDER_WIDTH: f64 = 1.0;
pub const BORDER_DASH: [f32; 2] = [4.0, 4.0];
/// Resize handles: 16 x 16 at (-8, -8) from their anchor point, gray 0.5 with a white 1.0 stroke.
/// The square is the handle's hit area; the drawn shape is a dot in it (`render`, approximate).
pub const HANDLE_SIZE: f64 = 16.0;
pub const HANDLE_FILL: Rgba = [0.5, 0.5, 0.5, 1.0];
pub const HANDLE_STROKE: f64 = 1.0;
/// Fill of the selection rectangle of the direct shortcut (colour; role: the simple
/// tint window).
pub const SIMPLE_TINT: Rgba = [0.53, 0.53, 0.53, 0.25];
/// Window-pick highlight: the accent fallback (rgb) at an alpha the analysis did not reach
/// (approximate, section 4).
pub const PICK_HIGHLIGHT: Rgba = [0.5294, 0.7255, 0.9059, 0.35];

/// Dimension label: 11 pt text on a backdrop with corner radius 4, at least 26 pt high, 13 pt edge inset.
pub const LABEL_FONT_SIZE: f64 = 11.0;
pub const LABEL_RADIUS: f64 = 4.0;
pub const LABEL_MIN_HEIGHT: f64 = 26.0;
pub const LABEL_EDGE_INSET: f64 = 13.0;
/// Label colours and its offset from the pointer (approximate: the backdrop recipe and the placement are unknown).
pub const LABEL_BACKDROP: Rgba = [0.12, 0.12, 0.12, 0.8];
pub const LABEL_TEXT: Rgba = [1.0, 1.0, 1.0, 1.0];
pub const LABEL_OFFSET: f64 = 12.0;

/// Crosshair, in pixels of its 1x asset: 32 x 32 with the centre at (15, 15), arms spanning 3 to
/// 26, a black core at alpha 191/255 with a 1 px white halo at alpha 48/255.
pub const CROSSHAIR_CENTRE: f64 = 15.0;
pub const CROSSHAIR_ARM: (f64, f64) = (3.0, 26.0);
pub const CROSSHAIR_CORE: Rgba = [0.0, 0.0, 0.0, 191.0 / 255.0];
pub const CROSSHAIR_HALO: Rgba = [1.0, 1.0, 1.0, 48.0 / 255.0];
/// Half the gap at the centre of the arms (approximate: records a gap but not its size).
pub const CROSSHAIR_GAP: f64 = 3.0;

// ---- toolbar -----------------------------------------------------------------------------------------------------

/// The toolbar: its window is its background, 560 x 48 points with corners of radius 8, drawn on the toolkit's
/// heads-up material.
pub const TOOLBAR_SIZE: (f64, f64) = (560.0, 48.0);
pub const TOOLBAR_RADIUS: f64 = 8.0;
/// Distance of the toolbar from the bottom of the visible frame: 12 points to the window of the design, whose
/// background sits 28 points above that window's bottom edge.
pub const TOOLBAR_BOTTOM: i32 = 40;
/// Space between the toolbar's edges and its controls, and between the controls (approximate).
pub const TOOLBAR_PADDING: f64 = 8.0;
pub const TOOLBAR_SPACING: f64 = 8.0;
/// Timer choices of the Options menu, seconds.
pub const TIMER_CHOICES: [u32; 3] = [0, 5, 10];

// ---- keyboard ----------------------------------------------------------------------------------------------------

/// Share of the output's width and height of the selection an arrow key spans when there is none (approximate).
pub const KEYBOARD_SELECTION: f64 = 0.5;
/// Points an arrow key moves or resizes the selection with Shift held; a point without (approximate).
pub const KEYBOARD_STEP_LARGE: f64 = 10.0;

// ---- names -------------------------------------------------------------------------------------------------------

/// First word of a screenshot's file name (pattern `<Screenshot|Screen Recording> <date> at <time>).
pub const FILE_PREFIX: &str = "Screenshot";
