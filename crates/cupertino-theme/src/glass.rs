//! Glass material recipes: every value of the background pass (blur, face colour, refraction, bleed, highlight and
//! shadows) and the foreground colour matrix, for a [`GlassMode`] (appearance, style and accessibility options) and the
//! size of the shape it covers.
//!
//! A recipe is plain data. `cupertino-glass` turns it into GPU uniforms and holds the CPU model of each stage;
//! the README of that crate describes what every value does.
//!
//! * [`GlassStyle::Regular`] is the legibility style for controls, bars and panels; [`GlassStyle::Clear`] lets more of
//!   the backdrop through, for controls over media; [`GlassStyle::LargePanel`] is the regular glass of large floating
//!   panels, with a stronger tone compression and a drop shadow of its own.
//! * Size: several values follow the short side of the shape ([`GlassParams::new`]), so a small control bends and
//!   bleeds less than a large panel.
//! * Accessibility selects recipes of its own, it does not turn the glass off: Reduce Transparency trades refraction for
//!   a wide blur and a dense fill, Increase Contrast keeps the optics with darker or lighter colours, both together take
//!   the optics of the first and the colours of the second, and Reduce Motion flattens the refraction and widens the
//!   blur.

/// The scale the backdrop of a glass container is captured at, relative to the output's pixels, unless its recipe
/// says otherwise ([`GlassParams::capture_scale`]).
pub const GLASS_BACKDROP_SCALE: f32 = 0.5;
/// The capture scale of a notification card's glass.
pub const CARD_CAPTURE_SCALE: f32 = 0.25;
/// The capture scale of a track's glass: the output's own.
pub const TRACK_CAPTURE_SCALE: f32 = 1.0;

/// Most blurred copies of the backdrop one container mixes: one per distinct blur of its shapes (each shape's Gaussian
/// follows its size), up to this many; a shape whose blur lies between two of them mixes the two.
pub const BLUR_LEVELS: usize = 4;

/// The Gaussian deviation of a glass blur, in points per point of its radius scaled by the first level's opacity: a
/// regular control (radius 5 at opacity 0.4) blurs with a deviation of 4 points, a large regular panel (opacity 0.8)
/// with 8, clear glass of radius 10 with 20.
pub const BLUR_SIGMA_PER_RADIUS: f32 = 2.0;
/// The Gaussian deviation of the blur fill, in points per point of its radius.
pub const FILL_SIGMA_PER_RADIUS: f32 = 2.0;
/// The widest deviation a glass blur takes, in points: past it the face reads as the average colour around it anyway,
/// and the backdrop it would read grows with it.
pub const BLUR_SIGMA_MAX: f32 = 100.0;

/// The short side, in points, below which a glass control counts as small: its foreground follows what is behind it
/// (light text over a dark backdrop, dark over a light one), while larger bars and panels keep their appearance.
pub const SMALL_CONTROL_SIDE: f32 = 64.0;

/// A 4×5 colour matrix, row-major: `out.r = m[0]·r + m[1]·g + m[2]·b + m[3]·a + m[4]`, and so on for g, b and a.
pub type ColorMatrix = [f32; 20];

/// The two glass styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassStyle {
    /// For legibility: controls, bars, menus and panels with text on them.
    #[default]
    Regular,
    /// Lets the backdrop through with little tone change, for controls over photos and video.
    Clear,
    /// Regular glass for large floating panels (the launcher): the same optics with a blur twice as wide and a stronger
    /// tone compression (a lower white point, a lifted black and a little less saturation), so a big surface softens
    /// the scene behind it, and a broad drop shadow of its own.
    LargePanel,
    /// The glass of a floating bar of icons (the dock): a narrow blur, a little more colour, a thin bending edge and a
    /// thin line of light along its top and bottom edges, over a face that lifts black a little and dims white a little.
    Bar,
    /// The glass of a dense floating surface with text on it (a menu, a popover): regular glass over a face pressed to
    /// a mid grey veil, so any backdrop reads as a quiet smoky pane under light text.
    Dense,
    /// The glass of the tiles of a panel of modules (the control panel): the clear face, which keeps the scene's colour,
    /// a wide blur and a strong bending edge, light in the light appearance and dark in the dark one.
    Module,
    /// The glass of a notification card: the clear face over a wide blur and the blur fill, a bending edge and a faint
    /// ring shadow inside its outline, read from a backdrop captured at a quarter of the output's scale.
    Card,
    /// The glass of a level's track (the control panel's sliders): no blur, the backdrop read at the output's scale, a
    /// slight lift of black, a bending edge that follows the track's height and a ring shadow inside its outline.
    Track,
    /// The glass of a small popover over a floating bar (a tile's name, a tile's menu): the backdrop under a wide soft
    /// blur fill only, a strong bending edge and a thin line of light along its top and bottom edges, the same at every
    /// size.
    Popover,
}

/// Everything besides the shape's size that selects a recipe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlassMode {
    pub dark: bool,
    pub style: GlassStyle,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
    /// How much denser the person asked every material to be, in percent (0 leaves the recipe as it is).
    pub density: u8,
}

/// The blur of the backdrop and the blur fill laid over it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassBlur {
    /// Blur radius in points; with the first level's opacity it sets the Gaussian ([`GlassParams::blur_sigma`]).
    pub radius: f32,
    /// Opacity of the levels: the first scales the blur radius (a small control blurs less than a large panel of the
    /// same recipe); the others leave the blur as it is.
    pub level_opacity: [f32; BLUR_LEVELS],
    /// Blur radius in points of the fill blended over the blurred backdrop.
    pub fill_radius: f32,
    /// Opacity of the fill in darken blending: keeps the darker of the two, which drops thin bright detail.
    pub fill_darken: f32,
    /// Opacity of the fill in normal blending.
    pub fill_normal: f32,
    /// Opacity of the fill in lighten blending: keeps the lighter of the two, which drops thin dark detail.
    pub fill_lighten: f32,
}

/// The face colour: how the blurred backdrop is toned before the edge effects. Every step works on encoded values, in
/// this order: saturation, the luma compression of `max_luma`, the levels of `black` and `white`, then the fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassFace {
    /// Saturation about the Rec. 709 luma of the encoded colour (1 leaves the colour).
    pub saturation: f32,
    /// The white level: the face loses `(1 − white) · L` on every channel, `L` its luma after the compression, so a
    /// value under 1 dims bright faces more than dark ones (over 1 it brightens them).
    pub white: f32,
    /// The black level: the face gains `black · (1 − L)` on every channel, so dark faces are lifted most.
    pub black: f32,
    /// Largest encoded channel value of the face.
    pub clamp: f32,
    /// The luma compression: a colour `c` of luma `L` becomes `c − (1 − max_luma) · L · (0.75 c + 0.25 L)`, so a grey
    /// `x` becomes `x − (1 − max_luma) · x²` and white comes out at `max_luma`.
    pub max_luma: f32,
    /// Straight sRGB fill laid over the face.
    pub fill: [f32; 4],
}

/// Refraction at the inner edge of the shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassRefraction {
    /// Strength in points: a pixel at the edge reads the face [`REFRACTION_EDGE_SHARE`] of it along the outward normal,
    /// less deeper in the band ([`refraction_profile`]); negative values read it further inside, as a convex rim does.
    pub amount: f32,
    /// Width in points of the band inside the edge that bends.
    pub height: f32,
    /// Strength outside the edge (none at rest).
    pub outer_amount: f32,
}

/// The backdrop's colour bleeding into the inner edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassBleed {
    /// Strength in percent at the edge.
    pub amount: f32,
    /// Width in points of the band it fades over.
    pub height: f32,
    /// Encoded level that white maps to.
    pub white: f32,
    /// Encoded level that black maps to.
    pub black: f32,
    pub saturation: f32,
    /// Darken blending (light glass) instead of lighten blending (dark glass).
    pub darken: bool,
    /// How much of the bleed shows: none at rest.
    pub opacity: f32,
}

/// The rim along the edge, and the soft band of light some surfaces lay under their top and bottom edges.
///
/// The rim shifts every channel of the face by `amount · color_bias` (a negative bias darkens), scaled by the face's
/// brightness and by the direction of the edge, strongest where the edge runs diagonally, in a band
/// [`RIM_DEPTH_PER_HEIGHT`] times `height` deep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassHighlight {
    /// Strength of the rim.
    pub amount: f32,
    /// Direction the soft band's light comes from, in radians, counter-clockwise from the right with y up (π/2 is the
    /// top).
    pub angle: f32,
    /// Angular width of each arc the soft band lights, in radians; an arc fades to nothing three quarters of it from its
    /// end of the axis.
    pub spread: f32,
    /// The rim's depth in points, as a share of [`RIM_DEPTH_PER_HEIGHT`].
    pub height: f32,
    /// Unused by the rim; kept with the recipe.
    pub offset: f32,
    /// The colour shift of the rim per unit of strength: negative darkens. The soft band's far end of the axis is lit at
    /// `1 + color_bias` of its near end.
    pub color_bias: f32,
    /// Exponent on the soft band's arc falloff.
    pub curvature: f32,
    /// A soft band of light along the arcs: its strength (0 for none), its half-width and where its middle lies
    /// relative to the edge, in points.
    pub glow_amount: f32,
    pub glow_height: f32,
    pub glow_offset: f32,
    /// How strongly the soft band lights the far end of its axis, as a share of the near end.
    pub glow_fill: f32,
    /// A second, wider and fainter band of the same light under the soft band: its half-width, the angular width of its
    /// arcs and its strength as multiples of the soft band's (all 0 for none).
    pub diffuse: GlowDiffuse,
    /// Colour of the soft band's light, from the face colour under it.
    pub matrix: ColorMatrix,
}

/// The wider, fainter band under the soft band of [`GlassHighlight`], in multiples of the soft band's values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlowDiffuse {
    /// Half-width per unit of the soft band's half-width.
    pub height_scale: f32,
    /// Angular width of its arcs per unit of the soft band's.
    pub spread_scale: f32,
    /// Strength per unit of the soft band's strength.
    pub amount_scale: f32,
}

impl GlowDiffuse {
    /// No second band.
    pub const NONE: Self = Self {
        height_scale: 0.0,
        spread_scale: 0.0,
        amount_scale: 0.0,
    };
}

/// How deep the rim reaches into the shape, per point of its `height`.
pub const RIM_DEPTH_PER_HEIGHT: f32 = 1.7;
/// How strongly the rim shows by the luma of the face under it `(luma, response)`; linear between: nothing on black,
/// most a little above the middle.
pub const RIM_BRIGHTNESS_RESPONSE: [(f32, f32); 5] = [
    (0.0, 0.0),
    (0.25, 0.63),
    (0.5, 1.0),
    (0.75, 1.13),
    (1.0, 1.0),
];
/// The share of the rim along straight edges (horizontal and vertical); the rest grows with `sin²(2φ)` to the full rim
/// where the edge runs diagonally.
pub const RIM_STRAIGHT_SHARE: f32 = 0.15;

/// The rim's response to a face of luma `luma` ([`RIM_BRIGHTNESS_RESPONSE`]).
pub fn rim_brightness(luma: f32) -> f32 {
    by_side(&RIM_BRIGHTNESS_RESPONSE, luma.clamp(0.0, 1.0))
}

/// The rim's response to an edge whose outward normal points at `angle` radians: [`RIM_STRAIGHT_SHARE`] on straight
/// edges, all of it on diagonal ones.
pub fn rim_direction(angle: f32) -> f32 {
    let s = (2.0 * angle).sin();
    RIM_STRAIGHT_SHARE + (1.0 - RIM_STRAIGHT_SHARE) * s * s
}

/// The drop shadow around the shape and the ring shadow inside it. Both are drawn at their opacity, which is 0 at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassShadow {
    /// Offset in points (y down).
    pub offset: [f32; 2],
    /// Strength in percent of the drop shadow.
    pub amount: f32,
    /// Blur radius in points of the drop shadow.
    pub height: f32,
    /// Straight sRGB colour of the drop shadow.
    pub fill: [f32; 4],
    pub saturation: f32,
    pub opacity: f32,
    /// Gaussian deviation in points of the ring shadow: a stroke along the inside of the outline that darkens the face.
    pub ring_blur: f32,
    /// Width in points of the ring.
    pub ring_stroke: f32,
    /// How far up the ring is moved, in points.
    pub ring_offset: f32,
    /// How much the ring darkens the face: none at rest.
    pub ring_opacity: f32,
    /// A hairline just outside the shape, in the drop shadow's colour: its opacity (0 for none) and its width in points.
    pub outline_opacity: f32,
    pub outline_width: f32,
}

/// The complete background recipe of one glass shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassParams {
    pub blur: GlassBlur,
    pub face: GlassFace,
    pub refraction: GlassRefraction,
    pub bleed: GlassBleed,
    pub highlight: GlassHighlight,
    pub shadow: GlassShadow,
    /// The scale the backdrop is captured at, relative to the output's pixels, before it is blurred: a coarser capture
    /// softens fine detail on its own, a full one keeps it sharp where nothing blurs it.
    pub capture_scale: f32,
}

// ---- size rules -------------------------------------------------------------------------------------------------

/// Largest refraction strength of any shape.
const REFRACTION_AMOUNT_MAX: f32 = 60.0;
/// Refraction strength per point of short side, regular style.
const REFRACTION_PER_POINT_REGULAR: f32 = 0.5;
/// Refraction strength per point of short side, clear style.
const REFRACTION_PER_POINT_CLEAR: f32 = 0.65;
/// Widest refraction band in points.
const REFRACTION_HEIGHT_MAX: f32 = 20.0;
/// Refraction band width per point of short side, regular style.
const REFRACTION_HEIGHT_PER_POINT_REGULAR: f32 = 0.25;
/// Refraction band width per point of short side, clear style.
const REFRACTION_HEIGHT_PER_POINT_CLEAR: f32 = 0.36;
/// Bleed strength and width per point of short side (regular style).
const BLEED_PER_POINT: f32 = 0.35;
/// Short side past which the bleed stops growing.
const BLEED_SIDE_MAX: f32 = 240.0;
/// Opacity of the first blur level of a regular shape by short side `(side, opacity)`, which scales its blur radius;
/// linear between, flat outside: a small control blurs with 0.4 of the radius, a panel 160 points high or more with 0.8.
const NARROW_LEVEL_OPACITY_BY_SIDE: [(f32, f32); 2] = [(64.0, 0.4), (160.0, 0.8)];
/// Largest face luminance of dark regular glass by short side `(side, max luma)`; linear between, flat outside.
const DARK_MAX_LUMA_BY_SIDE: [(f32, f32); 5] = [
    (60.0, 0.6),
    (80.0, 0.5),
    (100.0, 0.375),
    (120.0, 0.35),
    (240.0, 0.358),
];
/// Blur radius of the smallest clear shapes, in points.
const CLEAR_BLUR_MIN: f32 = 5.0;
/// Blur radius of the largest clear shapes.
const CLEAR_BLUR_MAX: f32 = 10.0;
/// Short side where the clear blur starts to grow.
const CLEAR_BLUR_SIDE_MIN: f32 = 48.0;
/// Blur gained per point of short side.
const CLEAR_BLUR_PER_POINT: f32 = 5.0 / 72.0;
/// Clear drop-shadow strength per point of short side, and the side it stops growing at.
const CLEAR_SHADOW_AMOUNT_PER_POINT: f32 = 0.625;
const CLEAR_SHADOW_AMOUNT_SIDE_MAX: f32 = 120.0;
/// Clear drop-shadow blur per point of short side, and the side it stops growing at.
const CLEAR_SHADOW_HEIGHT_PER_POINT: f32 = 0.4;
const CLEAR_SHADOW_HEIGHT_SIDE_MAX: f32 = 240.0;

/// The share of what is still see-through that the densest setting fills with the face's own colour.
const DENSITY_FILL_SHARE: f32 = 0.3;
/// The colour of a fill that has none of its own (a clear face) once it is made denser: dark and light.
const DENSITY_NEUTRAL: [f32; 3] = [0.12, 0.12, 0.12];
const DENSITY_NEUTRAL_LIGHT: [f32; 3] = [0.95, 0.95, 0.95];

/// `fill` made denser by `amount` (0 to 1): its opacity grows towards [`DENSITY_FILL_SHARE`] of what was see-through.
fn denser_fill(fill: [f32; 4], dark: bool, amount: f32) -> [f32; 4] {
    let colour = if fill[3] < 0.02 {
        if dark {
            DENSITY_NEUTRAL
        } else {
            DENSITY_NEUTRAL_LIGHT
        }
    } else {
        [fill[0], fill[1], fill[2]]
    };
    let alpha = fill[3] + (1.0 - fill[3]) * DENSITY_FILL_SHARE * amount;
    [colour[0], colour[1], colour[2], alpha]
}

// ---- accessibility ----------------------------------------------------------------------------------------------

/// Blur radius of regular glass with Reduce Transparency, in points.
const REDUCED_TRANSPARENCY_BLUR: f32 = 40.625;
/// Blur radius of clear glass with Reduce Transparency.
const REDUCED_TRANSPARENCY_BLUR_CLEAR: f32 = 200.0;
/// Level opacities with Reduce Transparency: the two narrow levels win.
const REDUCED_TRANSPARENCY_LEVELS: [f32; BLUR_LEVELS] = [1.0, 1.0, 0.123_076_92, 0.123_076_92];
/// Smallest blur radius with Reduce Motion.
const REDUCED_MOTION_BLUR: f32 = 8.0;
/// Level opacities with Reduce Motion.
const REDUCED_MOTION_LEVELS: [f32; BLUR_LEVELS] = [1.0, 1.0, 0.625, 0.625];

// ---- colour matrices --------------------------------------------------------------------------------------------

/// Key light colour of regular glass: a little more saturated and lifted than the face.
const HIGHLIGHT_MATRIX_REGULAR: ColorMatrix = [
    1.1202, -0.1894, -0.019, 0.0, 0.1471, //
    -0.0563, 0.9871, -0.0191, 0.0, 0.1471, //
    -0.0563, -0.1893, 1.1574, 0.0, 0.1471, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// Key light colour of clear glass: brighter, keeping the hue.
const HIGHLIGHT_MATRIX_CLEAR: ColorMatrix = [
    1.3192, -0.0478, -0.0047, 0.0, 0.2, //
    -0.0142, 1.2856, -0.0048, 0.0, 0.2, //
    -0.0142, -0.0476, 1.3285, 0.0, 0.2, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// Foreground on dark glass: the face under the glyph lifted almost to white, so a label keeps a trace of the colour
/// behind it.
const FOREGROUND_MATRIX_DARK: ColorMatrix = [
    1.0, 0.0, 0.0, 0.0, 0.95, //
    0.0, 1.0, 0.0, 0.0, 0.95, //
    0.0, 0.0, 1.0, 0.0, 0.95, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// Foreground on light glass: the face under the glyph pushed down to black.
const FOREGROUND_MATRIX_LIGHT: ColorMatrix = [
    1.0, 0.0, 0.0, 0.0, -1.0, //
    0.0, 1.0, 0.0, 0.0, -1.0, //
    0.0, 0.0, 1.0, 0.0, -1.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// Light from the top.
const HIGHLIGHT_ANGLE: f32 = std::f32::consts::FRAC_PI_2;

fn regular_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: 1.3,
            white: 1.125,
            black: 0.08,
            clamp: 1.308_310_9,
            max_luma: 0.375,
            fill: [0.0; 4],
        }
    } else {
        GlassFace {
            saturation: REGULAR_SATURATION_LIGHT,
            white: REGULAR_WHITE_LIGHT,
            black: REGULAR_BLACK_LIGHT,
            clamp: 1.0,
            max_luma: 0.94,
            fill: [1.0, 1.0, 1.0, REGULAR_FILL_LIGHT],
        }
    }
}

/// The light regular face: a little more colour, black lifted by 0.4 of what is left and white dimmed a little, under a
/// white veil, so a dark backdrop comes through as a light grey of it (black comes out at 130 of 255, white at 240).
const REGULAR_SATURATION_LIGHT: f32 = 1.2;
const REGULAR_WHITE_LIGHT: f32 = 0.96;
const REGULAR_BLACK_LIGHT: f32 = 0.40;
const REGULAR_FILL_LIGHT: f32 = 0.184;

fn clear_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: 1.0,
            white: 0.8,
            black: 0.05,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.05],
        }
    } else {
        GlassFace {
            saturation: 1.0,
            white: 0.95,
            black: 0.2,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.1],
        }
    }
}

/// The face of bar glass: a little more colour, black lifted and white dimmed a little. Light: black 0.15, white 0.9.
/// Dark: black 0.05, white 0.92, the luma compressed to 0.75 and a faint black veil.
fn bar_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: BAR_SATURATION,
            white: 0.92,
            black: 0.05,
            clamp: 1.0,
            max_luma: 0.75,
            fill: [0.0, 0.0, 0.0, BAR_VEIL_DARK],
        }
    } else {
        GlassFace {
            saturation: BAR_SATURATION,
            white: 0.9,
            black: 0.15,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.0],
        }
    }
}

/// The face of popover glass. Light: the regular light face with white a little past 1. Dark: black lifted by an
/// eighth, white past 1 and the luma compressed to 0.6, with more colour.
fn popover_face(dark: bool) -> GlassFace {
    let regular = regular_face(dark);
    if dark {
        GlassFace {
            black: POPOVER_BLACK_DARK,
            max_luma: POPOVER_MAX_LUMA_DARK,
            ..regular
        }
    } else {
        GlassFace {
            white: POPOVER_WHITE_LIGHT,
            ..regular
        }
    }
}

/// The side the popover's regular glass is worked out at before its own values replace what follows the size.
const POPOVER_SIDE: f32 = 26.0;
/// The popover's blur: its radius, and the opacity of each level, the first 0, so only the blur fill softens it.
const POPOVER_BLUR: f32 = 10.0;
const POPOVER_LEVELS: [f32; BLUR_LEVELS] = [0.0, 0.0, 0.5, 1.0];
/// The popover's blur fill: its radius, its opacity in darken (dark) or lighten (light) mode, and in normal mode.
const POPOVER_FILL_RADIUS: f32 = 16.0;
const POPOVER_FILL_OVER: f32 = 0.882;
const POPOVER_FILL_NORMAL: f32 = 0.46;
/// The popover's refraction: strength and band width in points.
const POPOVER_REFRACTION: f32 = -24.0;
const POPOVER_REFRACTION_HEIGHT: f32 = 5.0;
/// The popover's bleed band (shown at no opacity at rest).
const POPOVER_BLEED: f32 = 16.8;
/// The faint ring shadow inside the popover's outline.
const POPOVER_RING_OPACITY: f32 = 0.06;
/// The dark popover face's black lift and luma compression, and the light face's white point.
const POPOVER_BLACK_DARK: f32 = 0.125;
const POPOVER_MAX_LUMA_DARK: f32 = 0.6;
const POPOVER_WHITE_LIGHT: f32 = 1.03;

/// The saturation of the bar's face in both appearances.
const BAR_SATURATION: f32 = 1.25;
/// The black veil over the dark bar's face.
const BAR_VEIL_DARK: f32 = 0.02;
/// The bar's blur radius in points, at full opacity of the first level.
const BAR_BLUR: f32 = 2.761;
/// The bar's refraction: strength and band width in points, the same at every size.
const BAR_REFRACTION: f32 = -8.64;
const BAR_REFRACTION_HEIGHT: f32 = 3.84;
/// How strongly a soft band lights the far end of its axis, as a share of the near end, unless a surface says
/// otherwise: on clear glass and on regular glass (and the styles built on it).
const GLOW_FILL: f32 = 0.75;
const GLOW_FILL_REGULAR: f32 = 0.8125;
/// The bar's rim: strength and colour shift per unit of strength.
const BAR_RIM: f32 = 0.4;
const BAR_RIM_BIAS: f32 = -0.25;
/// The bar's line of light along its top and bottom edges: strength, half-width and middle (points from the edge, inside
/// for a negative value), the arcs' angular width, and the bottom's strength as a share of the top's.
const BAR_GLOW: f32 = 0.5;
const BAR_GLOW_HEIGHT: f32 = 0.6;
const BAR_GLOW_OFFSET: f32 = -0.25;
const BAR_GLOW_SPREAD: f32 = 80.0 * std::f32::consts::PI / 180.0;
const BAR_GLOW_FILL: f32 = 1.0;
/// The faint glow under the bar's line of light: eight times as wide, on arcs 0.65 as wide, at 0.15 of its strength.
const BAR_GLOW_DIFFUSE: GlowDiffuse = GlowDiffuse {
    height_scale: 8.0,
    spread_scale: 0.65,
    amount_scale: 0.15,
};
/// The dark hairline just outside the bar's outline, which shows at its ends: its opacity and width in points.
const BAR_OUTLINE_OPACITY: f32 = 0.45;
const BAR_OUTLINE_WIDTH: f32 = 0.5;
/// The bar's drop shadow (shown at no opacity at rest): strength in percent and blur in points.
const BAR_SHADOW_AMOUNT: f32 = 30.0;
const BAR_SHADOW_HEIGHT: f32 = 19.2;
/// The colour of the bar's soft light from the face under it: a little more saturated and lifted than the face, in the
/// light and the dark appearance.
const BAR_GLOW_MATRIX_LIGHT: ColorMatrix = [
    1.2625, -0.2385, -0.024, 0.0, 0.2, //
    -0.0708, 1.0949, -0.0241, 0.0, 0.2, //
    -0.0709, -0.2383, 1.3093, 0.0, 0.2, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
const BAR_GLOW_MATRIX_DARK: ColorMatrix = [
    1.0937, -0.3577, -0.036, 0.0, 0.25, //
    -0.1063, 0.8424, -0.0361, 0.0, 0.25, //
    -0.1064, -0.3575, 1.1639, 0.0, 0.25, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// The face of dense glass. Dark: a bright backdrop comes out near a mid grey (about 125 of 255) and a dark one is
/// lifted a little, with the colours kept, under a faint grey veil. Light: the regular light face.
fn dense_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: DENSE_SATURATION_DARK,
            white: DENSE_WHITE_DARK,
            black: DENSE_BLACK_DARK,
            clamp: 1.308_310_9,
            max_luma: DENSE_MAX_LUMA_DARK,
            fill: DENSE_FILL_DARK,
        }
    } else {
        regular_face(false)
    }
}

/// Saturation, white and black levels and luma compression of the dark dense face, and the straight sRGB veil laid over
/// it: white comes out at 114 of 255, black at 37, a mid grey at 105, and a strong red keeps its colour.
const DENSE_SATURATION_DARK: f32 = 2.04;
const DENSE_WHITE_DARK: f32 = 1.5;
const DENSE_BLACK_DARK: f32 = 0.039;
const DENSE_MAX_LUMA_DARK: f32 = 0.395;
const DENSE_FILL_DARK: [f32; 4] = [0.261, 0.261, 0.261, 0.482];

/// Module glass: the clear glass of its size with a strong bending edge and a darker rim. The strength is the same at
/// every size; the band is [`MODULE_REFRACTION_HEIGHT_PER_POINT`] of the short side, at most
/// [`MODULE_REFRACTION_HEIGHT_MAX`] points.
const MODULE_REFRACTION: f32 = -80.0;
const MODULE_REFRACTION_HEIGHT_PER_POINT: f32 = 0.225;
const MODULE_REFRACTION_HEIGHT_MAX: f32 = 18.0;
const MODULE_RIM_BIAS: f32 = -0.3;

/// Card glass: the blur radius at full opacity of the first level, the blur fill's radius and opacities (lighten in the
/// light appearance, darken in the dark one, and normal), the refraction, the rim and the ring shadow inside the
/// outline, which shows at rest.
const CARD_BLUR: f32 = 10.0;
const CARD_FILL_RADIUS: f32 = 8.0;
const CARD_FILL_BLEND: f32 = 0.8821;
const CARD_FILL_NORMAL: f32 = 0.4602;
const CARD_REFRACTION: f32 = -60.0;
const CARD_REFRACTION_HEIGHT: f32 = 20.0;
const CARD_RIM: f32 = 0.4;
const CARD_RIM_BIAS: f32 = -0.25;
/// The angular width of the card's light, light and dark appearance, in radians (96° and 75°).
const CARD_SPREAD_LIGHT: f32 = 1.676;
const CARD_SPREAD_DARK: f32 = 5.0 * std::f32::consts::PI / 12.0;
const CARD_RING_OPACITY: f32 = 0.06;

/// Track glass: the face lifts black by a tenth and leaves white; the refraction's strength and band follow the
/// track's height (its short side); the rim; the ring shadow inside the outline, darker in the dark appearance; and a
/// faint bluish shadow below it.
const TRACK_BLACK: f32 = 0.1;
const TRACK_REFRACTION_PER_POINT: f32 = 0.3;
const TRACK_REFRACTION_HEIGHT_PER_POINT: f32 = 0.18;
const TRACK_RIM: f32 = 0.5;
const TRACK_RIM_BIAS: f32 = -0.225;
const TRACK_RING_BLUR: f32 = 3.0;
const TRACK_RING_LIGHT: f32 = 0.15;
const TRACK_RING_DARK: f32 = 0.40;
const TRACK_SHADOW_OFFSET: f32 = 7.0;
const TRACK_SHADOW_AMOUNT: f32 = 6.0;
const TRACK_SHADOW_BLUR: f32 = 8.0;
const TRACK_SHADOW_OPACITY: f32 = 0.0018;
const TRACK_SHADOW_COLOR: [f32; 4] = [0.337, 0.445, 0.562, 1.0];

/// Face colours with Reduce Transparency.
fn reduced_transparency_face(dark: bool, style: GlassStyle) -> GlassFace {
    match (style, dark) {
        (GlassStyle::Clear, true) => GlassFace {
            saturation: 0.7,
            white: 0.0,
            black: 0.075,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [0.0, 0.0, 0.0, 0.25],
        },
        (_, true) => GlassFace {
            saturation: 0.7,
            white: 0.2,
            black: 0.2,
            clamp: 1.0,
            max_luma: 0.375,
            fill: [0.0, 0.0, 0.0, 0.6],
        },
        // light clear glass takes the light regular colours: the denser of the two choices
        (_, false) => GlassFace {
            saturation: 0.9,
            white: 1.03,
            black: 0.9,
            clamp: 1.069_610_1,
            max_luma: 0.94,
            fill: [1.0, 1.0, 1.0, 0.8],
        },
    }
}

/// Face colours with Increase Contrast; clear glass keeps its own.
fn increased_contrast_face(dark: bool, style: GlassStyle) -> Option<GlassFace> {
    match (style, dark) {
        (GlassStyle::Clear, _) => None,
        (_, true) => Some(GlassFace {
            saturation: 0.8,
            white: 0.3,
            black: 0.1,
            clamp: 1.0,
            max_luma: 0.375,
            fill: [0.12, 0.12, 0.12, 0.4],
        }),
        (_, false) => Some(GlassFace {
            saturation: 0.8,
            white: 0.96,
            black: 0.5,
            clamp: 1.0,
            max_luma: 0.94,
            fill: [1.0, 1.0, 1.0, 0.45],
        }),
    }
}

/// Face of the large panel. Dark: white behind it comes out a mid grey (about 125 of 255) and black a dark grey (about
/// 53), while strong colours keep much of their brightness, as their luma is compressed less; the saturation is a
/// little under the regular glass's. Light: the regular light face.
fn large_panel_face(dark: bool) -> GlassFace {
    let regular = regular_face(dark);
    if dark {
        GlassFace {
            saturation: LARGE_PANEL_SATURATION_DARK,
            white: LARGE_PANEL_WHITE_DARK,
            black: LARGE_PANEL_BLACK_DARK,
            max_luma: LARGE_PANEL_MAX_LUMA_DARK,
            ..regular
        }
    } else {
        regular
    }
}

/// The dark large panel's tone compression: saturation, white and black levels and luma compression. A bright spot in
/// the scene (a sunlit sky, a white page, the light centre of a picture) comes through as a light patch near the
/// panel's tint rather than a glare: white comes out near 125, black near 53.
pub const LARGE_PANEL_SATURATION_DARK: f32 = 0.93;
pub const LARGE_PANEL_WHITE_DARK: f32 = 0.94;
pub const LARGE_PANEL_BLACK_DARK: f32 = 0.164;
pub const LARGE_PANEL_MAX_LUMA_DARK: f32 = 0.415;
/// How much wider the large panel blurs than regular glass: its blur levels and its blur fill (10 and 16 points
/// rather than 5 and 8), so stripes, text and bright spots in the scene behind a launcher-sized sheet melt into broad
/// fields of colour instead of showing through as ripples.
pub const LARGE_PANEL_BLUR_SCALE: f32 = 2.0;
/// The large panel's drop shadow: how far below the panel it falls and its blur radius in points (its edge fades over
/// three radii), and its black's opacity in each appearance.
pub const LARGE_PANEL_SHADOW_OFFSET: f32 = 16.0;
pub const LARGE_PANEL_SHADOW_BLUR: f32 = 24.0;
pub const LARGE_PANEL_SHADOW_DARK: f32 = 0.5;
pub const LARGE_PANEL_SHADOW_LIGHT: f32 = 0.40;

/// Linear interpolation through `table` (sorted by side), flat outside it.
fn by_side(table: &[(f32, f32)], side: f32) -> f32 {
    let (first, last) = (table[0], table[table.len() - 1]);
    if side <= first.0 {
        return first.1;
    }
    if side >= last.0 {
        return last.1;
    }
    table
        .windows(2)
        .find(|w| side <= w[1].0)
        .map(|w| {
            let t = (side - w[0].0) / (w[1].0 - w[0].0);
            w[0].1 + (w[1].1 - w[0].1) * t
        })
        .unwrap_or(last.1)
}

impl GlassParams {
    /// The recipe of a shape in `mode` whose shorter side is `short_side` points.
    pub fn new(mode: GlassMode, short_side: f32) -> Self {
        let s = if short_side.is_finite() {
            short_side.max(0.0)
        } else {
            0.0
        };
        let dark = mode.dark;
        let mut p = match mode.style {
            GlassStyle::Regular => Self::regular(dark, s),
            GlassStyle::Clear => Self::clear(dark, s),
            GlassStyle::LargePanel => Self::large_panel(dark, s),
            GlassStyle::Bar => Self::bar(dark, s),
            GlassStyle::Dense => Self::dense(dark, s),
            GlassStyle::Module => Self::module(dark, s),
            GlassStyle::Card => Self::card(dark, s),
            GlassStyle::Track => Self::track(dark, s),
            GlassStyle::Popover => Self::popover(dark),
        };
        if mode.increase_contrast {
            if let Some(face) = increased_contrast_face(dark, mode.style) {
                p.face = face;
            }
        }
        if mode.reduce_motion {
            p.refraction.height = 0.0;
            p.blur.radius = p.blur.radius.max(REDUCED_MOTION_BLUR);
            p.blur.level_opacity = REDUCED_MOTION_LEVELS;
        }
        if mode.reduce_transparency {
            p.refraction.amount = 0.0;
            p.refraction.height = 0.0;
            p.shadow.ring_blur = 0.0;
            p.shadow.ring_stroke = 0.0;
            p.shadow.ring_offset = 0.0;
            match mode.style {
                GlassStyle::Regular
                | GlassStyle::LargePanel
                | GlassStyle::Bar
                | GlassStyle::Dense
                | GlassStyle::Module
                | GlassStyle::Card
                | GlassStyle::Track
                | GlassStyle::Popover => {
                    // the large panel's wider blur gives way to regular glass's whole blur recipe
                    p.blur = Self::regular(dark, s).blur;
                    p.blur.radius = REDUCED_TRANSPARENCY_BLUR;
                    p.blur.level_opacity = REDUCED_TRANSPARENCY_LEVELS;
                }
                GlassStyle::Clear => p.blur.radius = REDUCED_TRANSPARENCY_BLUR_CLEAR,
            }
            // both options: these optics with the contrast colours (clear glass has none of its own)
            let contrast = mode
                .increase_contrast
                .then(|| increased_contrast_face(dark, mode.style))
                .flatten();
            p.face = contrast.unwrap_or_else(|| reduced_transparency_face(dark, mode.style));
        } else if mode.density > 0 {
            p.face.fill = denser_fill(p.face.fill, dark, f32::from(mode.density.min(100)) / 100.0);
        }
        p
    }

    fn regular(dark: bool, s: f32) -> Self {
        let narrow = by_side(&NARROW_LEVEL_OPACITY_BY_SIDE, s);
        let bleed = BLEED_PER_POINT * s.min(BLEED_SIDE_MAX);
        let mut face = regular_face(dark);
        if dark {
            face.max_luma = by_side(&DARK_MAX_LUMA_BY_SIDE, s);
        }
        Self {
            blur: GlassBlur {
                radius: 5.0,
                level_opacity: [narrow, narrow, 1.0, 1.0],
                fill_radius: 8.0,
                fill_darken: if dark { 0.882 } else { 0.0 },
                fill_normal: 0.46,
                fill_lighten: if dark { 0.0 } else { 0.882 },
            },
            face,
            refraction: GlassRefraction {
                amount: -(REFRACTION_PER_POINT_REGULAR * s).min(REFRACTION_AMOUNT_MAX),
                height: (REFRACTION_HEIGHT_PER_POINT_REGULAR * s).min(REFRACTION_HEIGHT_MAX),
                outer_amount: 0.0,
            },
            bleed: GlassBleed {
                amount: bleed,
                height: bleed,
                white: if dark { 0.5 } else { 1.0 },
                black: if dark { 0.125 } else { 0.9 },
                saturation: if dark { 1.0 } else { 1.2 },
                darken: !dark,
                opacity: 0.0,
            },
            highlight: GlassHighlight {
                amount: 0.5,
                angle: HIGHLIGHT_ANGLE,
                spread: 2.0 * std::f32::consts::FRAC_PI_3,
                height: 0.5,
                offset: -0.5,
                color_bias: -0.1875,
                curvature: 0.75,
                glow_amount: 0.0,
                glow_height: 0.0,
                glow_offset: 0.0,
                glow_fill: GLOW_FILL_REGULAR,
                diffuse: GlowDiffuse::NONE,
                matrix: HIGHLIGHT_MATRIX_REGULAR,
            },
            shadow: GlassShadow {
                offset: [0.0, 8.0],
                amount: 0.0,
                height: 0.0,
                fill: [0.0, 0.0, 0.0, 0.3],
                saturation: 1.0,
                opacity: 0.0,
                ring_blur: 5.0,
                ring_stroke: 4.0,
                ring_offset: 8.0,
                ring_opacity: 0.0,
                outline_opacity: 0.0,
                outline_width: 0.0,
            },
            capture_scale: GLASS_BACKDROP_SCALE,
        }
    }

    /// The large panel's recipe: the regular one of its size with the face compressed towards the middle (see
    /// [`large_panel_face`]) and the panel's drop shadow shown.
    fn large_panel(dark: bool, s: f32) -> Self {
        let mut p = Self::regular(dark, s);
        p.blur.radius *= LARGE_PANEL_BLUR_SCALE;
        p.blur.fill_radius *= LARGE_PANEL_BLUR_SCALE;
        p.face = large_panel_face(dark);
        p.shadow.offset = [0.0, LARGE_PANEL_SHADOW_OFFSET];
        p.shadow.amount = 100.0;
        p.shadow.height = LARGE_PANEL_SHADOW_BLUR;
        p.shadow.fill = [
            0.0,
            0.0,
            0.0,
            if dark {
                LARGE_PANEL_SHADOW_DARK
            } else {
                LARGE_PANEL_SHADOW_LIGHT
            },
        ];
        p.shadow.opacity = 1.0;
        p
    }

    /// The dense glass: the regular recipe of its size over the dense face ([`dense_face`]).
    fn dense(dark: bool, s: f32) -> Self {
        let mut p = Self::regular(dark, s);
        p.face = dense_face(dark);
        p
    }

    /// The module glass: the clear glass of its size with the module's refraction and rim.
    fn module(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        p.refraction.amount = MODULE_REFRACTION;
        p.refraction.height =
            (MODULE_REFRACTION_HEIGHT_PER_POINT * s).min(MODULE_REFRACTION_HEIGHT_MAX);
        p.highlight.color_bias = MODULE_RIM_BIAS;
        p
    }

    /// The card glass: the clear face over a wide blur and the regular glass's blur fill, with the card's refraction,
    /// rim and ring shadow, captured at [`CARD_CAPTURE_SCALE`].
    fn card(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        p.blur = GlassBlur {
            radius: CARD_BLUR,
            level_opacity: [1.0; BLUR_LEVELS],
            fill_radius: CARD_FILL_RADIUS,
            fill_darken: if dark { CARD_FILL_BLEND } else { 0.0 },
            fill_normal: CARD_FILL_NORMAL,
            fill_lighten: if dark { 0.0 } else { CARD_FILL_BLEND },
        };
        p.refraction.amount = CARD_REFRACTION;
        p.refraction.height = CARD_REFRACTION_HEIGHT;
        p.highlight.amount = CARD_RIM;
        p.highlight.color_bias = CARD_RIM_BIAS;
        p.highlight.spread = if dark {
            CARD_SPREAD_DARK
        } else {
            CARD_SPREAD_LIGHT
        };
        p.highlight.matrix = HIGHLIGHT_MATRIX_REGULAR;
        p.shadow = GlassShadow {
            amount: 0.0,
            height: 0.0,
            fill: [0.0, 0.0, 0.0, 0.3],
            saturation: 1.0,
            ring_blur: 5.0,
            ring_stroke: 4.0,
            ring_offset: 8.0,
            ring_opacity: CARD_RING_OPACITY,
            ..p.shadow
        };
        p.capture_scale = CARD_CAPTURE_SCALE;
        p
    }

    /// The track glass: no blur, read at [`TRACK_CAPTURE_SCALE`], a refraction that follows the track's height, and
    /// a ring shadow inside the outline.
    fn track(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        p.blur = GlassBlur {
            radius: 0.0,
            level_opacity: [0.0; BLUR_LEVELS],
            fill_radius: 0.0,
            fill_darken: 0.0,
            fill_normal: 0.0,
            fill_lighten: 0.0,
        };
        p.face = GlassFace {
            saturation: 1.0,
            white: 1.0,
            black: TRACK_BLACK,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.0],
        };
        p.refraction.amount = -TRACK_REFRACTION_PER_POINT * s;
        p.refraction.height = TRACK_REFRACTION_HEIGHT_PER_POINT * s;
        p.highlight = GlassHighlight {
            amount: TRACK_RIM,
            spread: 2.0 * std::f32::consts::FRAC_PI_3,
            height: 1.0,
            offset: -1.0,
            color_bias: TRACK_RIM_BIAS,
            matrix: HIGHLIGHT_MATRIX_REGULAR,
            ..p.highlight
        };
        p.shadow = GlassShadow {
            offset: [0.0, TRACK_SHADOW_OFFSET],
            amount: TRACK_SHADOW_AMOUNT,
            height: TRACK_SHADOW_BLUR,
            fill: TRACK_SHADOW_COLOR,
            saturation: 1.0,
            opacity: TRACK_SHADOW_OPACITY,
            ring_blur: TRACK_RING_BLUR,
            ring_stroke: 4.0,
            ring_offset: 8.0,
            ring_opacity: if dark {
                TRACK_RING_DARK
            } else {
                TRACK_RING_LIGHT
            },
            outline_opacity: 0.0,
            outline_width: 0.0,
        };
        p.capture_scale = TRACK_CAPTURE_SCALE;
        p
    }

    /// The popover glass: no blur of its own, only the wide soft blur fill, the popover's face, a strong bending edge,
    /// the rim and the thin line of light along the top and bottom edges over a wide faint glow; a faint ring shadow
    /// inside its outline. Nothing in it follows the size.
    fn popover(dark: bool) -> Self {
        let mut p = Self::regular(dark, POPOVER_SIDE);
        p.blur = GlassBlur {
            radius: POPOVER_BLUR,
            level_opacity: POPOVER_LEVELS,
            fill_radius: POPOVER_FILL_RADIUS,
            fill_darken: if dark { POPOVER_FILL_OVER } else { 0.0 },
            fill_normal: POPOVER_FILL_NORMAL,
            fill_lighten: if dark { 0.0 } else { POPOVER_FILL_OVER },
        };
        p.face = popover_face(dark);
        p.refraction = GlassRefraction {
            amount: POPOVER_REFRACTION,
            height: POPOVER_REFRACTION_HEIGHT,
            outer_amount: 0.0,
        };
        p.bleed.amount = POPOVER_BLEED;
        p.bleed.height = POPOVER_BLEED;
        p.highlight = GlassHighlight {
            amount: BAR_RIM,
            spread: BAR_GLOW_SPREAD,
            color_bias: BAR_RIM_BIAS,
            glow_amount: BAR_GLOW,
            glow_height: BAR_GLOW_HEIGHT,
            glow_offset: BAR_GLOW_OFFSET,
            glow_fill: BAR_GLOW_FILL,
            diffuse: BAR_GLOW_DIFFUSE,
            ..p.highlight
        };
        p.shadow.ring_opacity = POPOVER_RING_OPACITY;
        p
    }

    /// The bar glass: clear glass with the bar's narrow blur, face, thin refraction, rim and a thin line of light along
    /// its top and bottom edges, equally bright, over a wide faint glow.
    fn bar(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        p.blur.radius = BAR_BLUR;
        p.face = bar_face(dark);
        p.refraction.amount = BAR_REFRACTION;
        p.refraction.height = BAR_REFRACTION_HEIGHT;
        p.highlight = GlassHighlight {
            amount: BAR_RIM,
            angle: HIGHLIGHT_ANGLE,
            spread: BAR_GLOW_SPREAD,
            color_bias: BAR_RIM_BIAS,
            glow_amount: BAR_GLOW,
            glow_height: BAR_GLOW_HEIGHT,
            glow_offset: BAR_GLOW_OFFSET,
            glow_fill: BAR_GLOW_FILL,
            diffuse: BAR_GLOW_DIFFUSE,
            matrix: if dark {
                BAR_GLOW_MATRIX_DARK
            } else {
                BAR_GLOW_MATRIX_LIGHT
            },
            ..p.highlight
        };
        p.shadow.amount = BAR_SHADOW_AMOUNT;
        p.shadow.height = BAR_SHADOW_HEIGHT;
        p.shadow.outline_opacity = BAR_OUTLINE_OPACITY;
        p.shadow.outline_width = BAR_OUTLINE_WIDTH;
        p
    }

    fn clear(dark: bool, s: f32) -> Self {
        Self {
            blur: GlassBlur {
                radius: (CLEAR_BLUR_MIN + (s - CLEAR_BLUR_SIDE_MIN) * CLEAR_BLUR_PER_POINT)
                    .clamp(CLEAR_BLUR_MIN, CLEAR_BLUR_MAX),
                level_opacity: [1.0; BLUR_LEVELS],
                fill_radius: 0.0,
                fill_darken: 0.0,
                fill_normal: 0.0,
                fill_lighten: 0.0,
            },
            face: clear_face(dark),
            refraction: GlassRefraction {
                amount: -(REFRACTION_PER_POINT_CLEAR * s).min(REFRACTION_AMOUNT_MAX),
                height: (REFRACTION_HEIGHT_PER_POINT_CLEAR * s).min(REFRACTION_HEIGHT_MAX),
                outer_amount: 0.0,
            },
            bleed: GlassBleed {
                amount: 0.0,
                height: 0.0,
                white: 1.0,
                black: 0.75,
                saturation: 1.2,
                darken: true,
                opacity: 0.0,
            },
            highlight: GlassHighlight {
                amount: 0.4,
                angle: HIGHLIGHT_ANGLE,
                spread: 5.0 * std::f32::consts::PI / 12.0,
                height: 0.5,
                offset: -0.5,
                color_bias: -0.25,
                curvature: 0.75,
                glow_amount: 0.0,
                glow_height: 0.0,
                glow_offset: 0.0,
                glow_fill: GLOW_FILL,
                diffuse: GlowDiffuse::NONE,
                matrix: HIGHLIGHT_MATRIX_CLEAR,
            },
            shadow: GlassShadow {
                offset: [0.0, 8.0],
                amount: CLEAR_SHADOW_AMOUNT_PER_POINT * s.min(CLEAR_SHADOW_AMOUNT_SIDE_MAX),
                height: CLEAR_SHADOW_HEIGHT_PER_POINT * s.min(CLEAR_SHADOW_HEIGHT_SIDE_MAX),
                fill: [0.0, 0.0, 0.0, 0.1],
                saturation: 1.2,
                opacity: 0.0,
                ring_blur: 0.0,
                ring_stroke: 0.0,
                ring_offset: 0.0,
                ring_opacity: 0.0,
                outline_opacity: 0.0,
                outline_width: 0.0,
            },
            capture_scale: GLASS_BACKDROP_SCALE,
        }
    }

    /// The Gaussian deviation of the backdrop's blur, in points: the radius scaled by the first level's opacity, twice
    /// over, at most [`BLUR_SIGMA_MAX`].
    pub fn blur_sigma(&self) -> f32 {
        (BLUR_SIGMA_PER_RADIUS * self.blur.radius * self.blur.level_opacity[0].clamp(0.0, 1.0))
            .clamp(0.0, BLUR_SIGMA_MAX)
    }

    /// Whether the blur fill shows.
    pub fn fill_shows(&self) -> bool {
        self.blur.fill_darken + self.blur.fill_normal + self.blur.fill_lighten > 0.0
    }

    /// The Gaussian deviation of the blur fill, in points, at most [`BLUR_SIGMA_MAX`].
    pub fn fill_sigma(&self) -> f32 {
        (FILL_SIGMA_PER_RADIUS * self.blur.fill_radius).clamp(0.0, BLUR_SIGMA_MAX)
    }

    /// How far past the shape the backdrop is read, in points: three deviations of the widest blur it reads (the
    /// backdrop's, or the blur fill's when it shows), or the reach of a shown drop shadow.
    pub fn reach(&self) -> f32 {
        let fill = if self.fill_shows() {
            self.fill_sigma()
        } else {
            0.0
        };
        // a Gaussian is read three deviations out; the refraction reads the face further inside the shape, never past
        // its edge
        let read = 3.0 * self.blur_sigma().max(fill);
        read.max(self.shadow_reach())
    }

    /// How far past the shape a shown drop shadow reaches, in points: its offset and half its smooth edge of three
    /// blur radii; 0 when it does not show.
    pub fn shadow_reach(&self) -> f32 {
        let s = &self.shadow;
        if s.opacity * s.amount * s.fill[3] <= 0.0 {
            return 0.0;
        }
        s.offset[0].abs().max(s.offset[1].abs()) + 1.5 * s.height
    }
}

/// The refraction's offset at the edge, as a share of its strength.
pub const REFRACTION_EDGE_SHARE: f32 = 0.9;
/// How fast the refraction's offset decays across its band: `e^(−REFRACTION_DECAY · t)`.
pub const REFRACTION_DECAY: f32 = 4.2;

/// The refraction's offset `t` of its band deep (0 at the edge, 1 at the band's inner side), as a share of its strength:
/// `REFRACTION_EDGE_SHARE · e^(−REFRACTION_DECAY · t) · (1 − t⁴)`, nothing from the band's inner side on. A pixel reads
/// the face `strength · refraction_profile(t)` points along the edge's outward normal: a negative strength reads it
/// further inside, so the edge magnifies what lies just inside it.
pub fn refraction_profile(t: f32) -> f32 {
    if !(0.0..1.0).contains(&t) {
        return if t < 0.0 { REFRACTION_EDGE_SHARE } else { 0.0 };
    }
    let t2 = t * t;
    REFRACTION_EDGE_SHARE * (-REFRACTION_DECAY * t).exp() * (1.0 - t2 * t2)
}

/// The foreground colour matrix of text and symbols on glass of `dark` appearance. It is applied to the face colour
/// under the glyph, and the glyph's coverage and alpha mix the result over that face.
pub fn foreground_matrix(dark: bool) -> ColorMatrix {
    if dark {
        FOREGROUND_MATRIX_DARK
    } else {
        FOREGROUND_MATRIX_LIGHT
    }
}

/// Whether a glass shape whose short side is `short_side` points picks its foreground from what is behind it.
pub fn adapts_foreground(short_side: f32) -> bool {
    short_side < SMALL_CONTROL_SIDE
}

/// What a small glass shape's face looks like, which picks its foreground ([`FaceTone::matrix`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaceTone {
    /// A dark face: light text and symbols.
    Dark,
    /// A light face: dark text and symbols.
    Light,
}

impl FaceTone {
    /// The foreground matrix of a face of this tone: the dark appearance's over a dark face, the light one's over a
    /// light face.
    pub fn matrix(self) -> ColorMatrix {
        foreground_matrix(self == FaceTone::Dark)
    }
}

/// The relative luminance of a face where white and black text read equally well (both about 4.6:1): above it a small
/// shape's face counts as light.
pub const FACE_TONE_THRESHOLD: f32 = 0.179;
/// How far past [`FACE_TONE_THRESHOLD`] the luminance must go before the tone changes, so that a backdrop moving about
/// the threshold (video under a control) does not make the text flicker.
pub const FACE_TONE_HYSTERESIS: f32 = 0.03;

/// The tone of a face of relative luminance `luminance`, which had tone `previous` (`None` when it was not measured
/// yet). The tone changes only once the luminance is [`FACE_TONE_HYSTERESIS`] past the threshold on the other side.
pub fn face_tone(luminance: f32, previous: Option<FaceTone>) -> FaceTone {
    let light_from = match previous {
        None => FACE_TONE_THRESHOLD,
        Some(FaceTone::Dark) => FACE_TONE_THRESHOLD + FACE_TONE_HYSTERESIS,
        Some(FaceTone::Light) => FACE_TONE_THRESHOLD - FACE_TONE_HYSTERESIS,
    };
    if luminance > light_from {
        FaceTone::Light
    } else {
        FaceTone::Dark
    }
}

/// The foreground matrix of text and symbols on a glass shape of `dark` appearance whose face was measured as `tone`:
/// the appearance's matrix until it was measured, or when the shape does not adapt ([`adapts_foreground`]).
pub fn foreground_matrix_for(dark: bool, tone: Option<FaceTone>) -> ColorMatrix {
    tone.map_or_else(|| foreground_matrix(dark), FaceTone::matrix)
}

/// The relative luminance of an encoded sRGB colour, each channel from 0 to 1.
pub fn relative_luminance(rgb: [f32; 3]) -> f32 {
    let lin = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// The contrast ratio of two encoded sRGB colours, from 1 to 21.
pub fn contrast_ratio(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `m · [rgb, a]`, unclamped.
pub fn apply_matrix(m: &ColorMatrix, c: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|r| {
        let row = &m[r * 5..r * 5 + 5];
        row[0] * c[0] + row[1] * c[1] + row[2] * c[2] + row[3] * c[3] + row[4]
    })
}

/// How strongly a primary label (text, a control's text, a header) is drawn on glass: the face behind it is busier
/// than a window's background, so labels on glass are drawn more opaque than on a plain window.
pub const VIBRANT_PRIMARY: f32 = 1.0;
/// The shift a secondary label's matrix gives every channel of the face under it: down on light glass, up on dark
/// glass (a primary label's goes all the way to black or near white).
pub const SECONDARY_SHIFT_LIGHT: f32 = -0.4;
pub const SECONDARY_SHIFT_DARK: f32 = 0.3;
/// The face a label drawn as a plain colour is taken to lie over, as the window does not know the face: the regular
/// glass's face over a mid grey, in the light and the dark appearance.
pub const LABEL_FACE_LIGHT: f32 = 0.73;
pub const LABEL_FACE_DARK: f32 = 0.43;

/// How strongly a secondary label is drawn on glass whose foreground is the `dark` side's: the opacity at which the
/// side's foreground (black or white) shifts the label face by [`SECONDARY_SHIFT_LIGHT`] or [`SECONDARY_SHIFT_DARK`].
pub fn vibrant_secondary(dark: bool) -> f32 {
    if dark {
        SECONDARY_SHIFT_DARK / (1.0 - LABEL_FACE_DARK)
    } else {
        -SECONDARY_SHIFT_LIGHT / LABEL_FACE_LIGHT
    }
}
/// How strongly a text field's placeholder is drawn on glass.
pub const VIBRANT_PLACEHOLDER: f32 = 0.7;

/// The face the foreground matrix is applied to when the window does not know the face: a mid grey. Both matrices
/// carry any face but the darkest to white or to black, so the exact face hardly matters.
const NOMINAL_FACE: f32 = 0.5;

/// The colour of text and symbols on glass of the `dark` appearance whose face was measured as `tone`
/// ([`foreground_matrix_for`]), as an encoded sRGB colour the window draws at a label's opacity.
pub fn foreground_rgb(dark: bool, tone: Option<FaceTone>) -> [f32; 3] {
    let out = apply_matrix(
        &foreground_matrix_for(dark, tone),
        [NOMINAL_FACE, NOMINAL_FACE, NOMINAL_FACE, 1.0],
    );
    [
        out[0].clamp(0.0, 1.0),
        out[1].clamp(0.0, 1.0),
        out[2].clamp(0.0, 1.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(dark: bool, style: GlassStyle) -> GlassMode {
        GlassMode {
            dark,
            style,
            ..GlassMode::default()
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn dark_regular_wide_shape() {
        // 150 × 100 point shape: short side 100
        let p = GlassParams::new(mode(true, GlassStyle::Regular), 100.0);
        assert_eq!(p.blur.radius, 5.0);
        assert_eq!(p.blur.fill_radius, 8.0);
        assert!(close(p.blur.level_opacity[0], 0.55) && close(p.blur.level_opacity[1], 0.55));
        assert_eq!(&p.blur.level_opacity[2..], &[1.0, 1.0]);
        assert_eq!(
            (p.blur.fill_darken, p.blur.fill_normal, p.blur.fill_lighten),
            (0.882, 0.46, 0.0)
        );
        assert_eq!(
            (p.face.saturation, p.face.white, p.face.black),
            (1.3, 1.125, 0.08)
        );
        assert!(close(p.face.clamp, 1.308_310_9) && close(p.face.max_luma, 0.375));
        assert_eq!(p.face.fill, [0.0; 4]);
        assert_eq!(
            (
                p.refraction.amount,
                p.refraction.height,
                p.refraction.outer_amount
            ),
            (-50.0, 20.0, 0.0)
        );
        assert_eq!(
            (p.bleed.amount, p.bleed.height, p.bleed.white, p.bleed.black),
            (35.0, 35.0, 0.5, 0.125)
        );
        let h = p.highlight;
        assert_eq!(
            (h.amount, h.height, h.offset, h.color_bias, h.curvature),
            (0.5, 0.5, -0.5, -0.1875, 0.75)
        );
        assert!(close(h.angle, std::f32::consts::FRAC_PI_2) && close(h.spread, 2.094_395));
        let s = p.shadow;
        assert_eq!(
            (s.offset, s.ring_blur, s.ring_stroke, s.ring_offset),
            ([0.0, 8.0], 5.0, 4.0, 8.0)
        );
        assert_eq!((s.opacity, s.ring_opacity), (0.0, 0.0), "no shadow at rest");
    }

    #[test]
    fn light_regular_differs_in_tone_and_blend_only() {
        let p = GlassParams::new(mode(false, GlassStyle::Regular), 100.0);
        assert_eq!(
            (p.face.saturation, p.face.white, p.face.black, p.face.clamp),
            (1.2, 0.96, 0.40, 1.0)
        );
        assert_eq!(p.face.max_luma, 0.94);
        assert_eq!(p.face.fill, [1.0, 1.0, 1.0, 0.184]);
        assert_eq!((p.blur.fill_darken, p.blur.fill_lighten), (0.0, 0.882));
        assert!(p.bleed.darken && p.bleed.saturation == 1.2);
        let dark = GlassParams::new(mode(true, GlassStyle::Regular), 100.0);
        assert_eq!(p.refraction, dark.refraction);
        assert_eq!(p.highlight, dark.highlight);
    }

    #[test]
    fn regular_size_rule_follows_the_short_side() {
        // (short side, refraction, height, bleed, narrow level opacity, dark max luma)
        let rows = [
            (20.0, -10.0, 5.0, 7.0, 0.4, 0.6),
            (40.0, -20.0, 10.0, 14.0, 0.4, 0.6),
            (60.0, -30.0, 15.0, 21.0, 0.4, 0.6),
            (80.0, -40.0, 20.0, 28.0, 0.466_666_7, 0.5),
            (100.0, -50.0, 20.0, 35.0, 0.55, 0.375),
            (120.0, -60.0, 20.0, 42.0, 0.633_333_3, 0.35),
            (140.0, -60.0, 20.0, 49.0, 0.716_666_7, 0.351_333_3),
            (180.0, -60.0, 20.0, 63.0, 0.8, 0.354),
        ];
        for (s, amount, height, bleed, narrow, luma) in rows {
            let p = GlassParams::new(mode(true, GlassStyle::Regular), s);
            assert!(
                close(p.refraction.amount, amount),
                "{s}: {}",
                p.refraction.amount
            );
            assert!(close(p.refraction.height, height), "{s}");
            assert!(
                close(p.bleed.amount, bleed) && close(p.bleed.height, bleed),
                "{s}"
            );
            assert!(
                close(p.blur.level_opacity[0], narrow),
                "{s}: {:?}",
                p.blur.level_opacity
            );
            assert!(close(p.face.max_luma, luma), "{s}");
            assert_eq!(p.blur.radius, 5.0);
            assert_eq!(p.highlight.amount, 0.5);
        }
    }

    #[test]
    fn clear_size_rule_follows_the_short_side() {
        // (short side, refraction, height, blur, shadow amount, shadow height)
        let rows = [
            (20.0, -13.0, 7.2, 5.0, 12.5, 8.0),
            (40.0, -26.0, 14.4, 5.0, 25.0, 16.0),
            (60.0, -39.0, 20.0, 5.833_333, 37.5, 24.0),
            (80.0, -52.0, 20.0, 7.222_222, 50.0, 32.0),
            (100.0, -60.0, 20.0, 8.611_111, 62.5, 40.0),
            (120.0, -60.0, 20.0, 10.0, 75.0, 48.0),
            (140.0, -60.0, 20.0, 10.0, 75.0, 56.0),
            (180.0, -60.0, 20.0, 10.0, 75.0, 72.0),
        ];
        for (s, amount, height, blur, shadow, shadow_height) in rows {
            let p = GlassParams::new(mode(true, GlassStyle::Clear), s);
            assert!(
                close(p.refraction.amount, amount),
                "{s}: {}",
                p.refraction.amount
            );
            assert!(close(p.refraction.height, height), "{s}");
            assert!(close(p.blur.radius, blur), "{s}: {}", p.blur.radius);
            assert!(
                close(p.shadow.amount, shadow) && close(p.shadow.height, shadow_height),
                "{s}"
            );
            assert_eq!(p.highlight.amount, 0.4);
            assert_eq!(p.blur.level_opacity, [1.0; 4]);
        }
    }

    #[test]
    fn clear_style_colours() {
        let dark = GlassParams::new(mode(true, GlassStyle::Clear), 100.0);
        assert_eq!(
            (dark.face.saturation, dark.face.white, dark.face.black),
            (1.0, 0.8, 0.05)
        );
        assert_eq!(dark.face.fill, [1.0, 1.0, 1.0, 0.05]);
        assert_eq!(
            (dark.highlight.color_bias, dark.shadow.fill[3]),
            (-0.25, 0.1)
        );
        assert_eq!((dark.bleed.amount, dark.blur.fill_normal), (0.0, 0.0));
        let light = GlassParams::new(mode(false, GlassStyle::Clear), 100.0);
        assert_eq!(
            (light.face.white, light.face.black, light.face.fill[3]),
            (0.95, 0.2, 0.1)
        );
    }

    fn with(dark: bool, style: GlassStyle, rt: bool, ic: bool, rm: bool) -> GlassParams {
        GlassParams::new(
            GlassMode {
                dark,
                style,
                reduce_transparency: rt,
                increase_contrast: ic,
                reduce_motion: rm,
                density: 0,
            },
            100.0,
        )
    }

    #[test]
    fn a_large_panel_is_regular_glass_with_a_compressed_face_and_its_own_shadow() {
        for dark in [true, false] {
            let regular = with(dark, GlassStyle::Regular, false, false, false);
            let panel = with(dark, GlassStyle::LargePanel, false, false, false);
            // the optics are the regular glass's, with the blur twice as wide
            assert_eq!(
                (panel.refraction, panel.bleed, panel.highlight),
                (regular.refraction, regular.bleed, regular.highlight)
            );
            assert_eq!(
                (panel.blur.radius, panel.blur.fill_radius),
                (10.0, 16.0),
                "{dark}"
            );
            assert_eq!(
                (
                    panel.blur.level_opacity,
                    panel.blur.fill_darken,
                    panel.blur.fill_normal,
                    panel.blur.fill_lighten
                ),
                (
                    regular.blur.level_opacity,
                    regular.blur.fill_darken,
                    regular.blur.fill_normal,
                    regular.blur.fill_lighten
                )
            );
            assert_eq!(
                (panel.face.clamp, panel.face.fill),
                (regular.face.clamp, regular.face.fill)
            );
            // dark: a lower white level, a lifted black and a little less saturation; light: the regular face
            if dark {
                assert!(panel.face.white < regular.face.white);
                assert!(panel.face.black > regular.face.black);
                assert!(panel.face.saturation < regular.face.saturation);
            } else {
                assert_eq!(panel.face, regular.face);
            }
            // the shadow shows, below the panel; regular glass casts none at rest
            assert_eq!(regular.shadow_reach(), 0.0);
            assert_eq!(panel.shadow.offset, [0.0, LARGE_PANEL_SHADOW_OFFSET]);
            assert_eq!(panel.shadow.height, LARGE_PANEL_SHADOW_BLUR);
            let alpha = if dark {
                LARGE_PANEL_SHADOW_DARK
            } else {
                LARGE_PANEL_SHADOW_LIGHT
            };
            assert_eq!(
                (
                    panel.shadow.opacity,
                    panel.shadow.amount,
                    panel.shadow.fill[3]
                ),
                (1.0, 100.0, alpha)
            );
            assert_eq!(
                panel.shadow_reach(),
                LARGE_PANEL_SHADOW_OFFSET + 1.5 * LARGE_PANEL_SHADOW_BLUR
            );
            assert!(panel.reach() >= panel.shadow_reach());
        }
    }

    #[test]
    fn a_large_panel_follows_the_per_mode_recipes_of_regular_glass() {
        for dark in [true, false] {
            for (rt, ic, rm) in [
                (true, false, false),
                (false, true, false),
                (true, true, false),
                (false, false, true),
            ] {
                let regular = with(dark, GlassStyle::Regular, rt, ic, rm);
                let panel = with(dark, GlassStyle::LargePanel, rt, ic, rm);
                let what = format!("dark {dark}, rt {rt}, ic {ic}, rm {rm}");
                if rt {
                    // Reduce Transparency's wide blur is the same for both
                    assert_eq!(panel.blur, regular.blur, "{what}");
                } else {
                    // otherwise the panel keeps its blur twice as wide, Reduce Motion's floor included
                    assert_eq!(
                        panel.blur.radius,
                        regular.blur.radius.max(5.0 * LARGE_PANEL_BLUR_SCALE),
                        "{what}"
                    );
                    assert_eq!(
                        panel.blur.level_opacity, regular.blur.level_opacity,
                        "{what}"
                    );
                }
                assert_eq!(panel.refraction, regular.refraction, "{what}");
                if rt || ic {
                    // the accessibility recipes replace the face whole, the large panel's included
                    assert_eq!(panel.face, regular.face, "{what}");
                } else {
                    assert_eq!(panel.face, large_panel_face(dark), "{what}");
                }
                // the panel keeps its shadow; Reduce Transparency drops the ring as it does for regular glass
                assert_eq!(panel.shadow.opacity, 1.0, "{what}");
                assert_eq!(
                    panel.shadow.ring_stroke, regular.shadow.ring_stroke,
                    "{what}"
                );
            }
        }
    }

    #[test]
    fn reduce_transparency_is_a_recipe_not_blur_off() {
        let dark = with(true, GlassStyle::Regular, true, false, false);
        assert_eq!((dark.refraction.amount, dark.refraction.height), (0.0, 0.0));
        assert_eq!(dark.blur.radius, 40.625);
        assert!(close(dark.blur.level_opacity[2], 0.123_076_9));
        assert_eq!(&dark.blur.level_opacity[..2], &[1.0, 1.0]);
        assert_eq!(dark.face.fill, [0.0, 0.0, 0.0, 0.6]);
        assert_eq!(
            (dark.face.saturation, dark.face.white, dark.face.black),
            (0.7, 0.2, 0.2)
        );
        assert_eq!((dark.shadow.ring_blur, dark.shadow.ring_stroke), (0.0, 0.0));
        let light = with(false, GlassStyle::Regular, true, false, false);
        assert_eq!(light.face.fill, [1.0, 1.0, 1.0, 0.8]);
        assert_eq!(light.face.saturation, 0.9);
        let clear = with(true, GlassStyle::Clear, true, false, false);
        assert_eq!(clear.blur.radius, 200.0);
        assert_eq!(clear.face.fill, [0.0, 0.0, 0.0, 0.25]);
    }

    #[test]
    fn increase_contrast_keeps_the_optics() {
        let base = with(true, GlassStyle::Regular, false, false, false);
        let dark = with(true, GlassStyle::Regular, false, true, false);
        assert_eq!((dark.refraction, dark.blur), (base.refraction, base.blur));
        assert_eq!(dark.face.fill, [0.12, 0.12, 0.12, 0.4]);
        assert_eq!(
            (dark.face.saturation, dark.face.white, dark.face.black),
            (0.8, 0.3, 0.1)
        );
        let light = with(false, GlassStyle::Regular, false, true, false);
        assert_eq!(light.face.fill, [1.0, 1.0, 1.0, 0.45]);
        assert_eq!(light.face.saturation, 0.8);
        let clear = with(true, GlassStyle::Clear, false, true, false);
        assert_eq!(clear, with(true, GlassStyle::Clear, false, false, false));
    }

    #[test]
    fn both_options_take_transparency_optics_and_contrast_colours() {
        let both = with(true, GlassStyle::Regular, true, true, false);
        let rt = with(true, GlassStyle::Regular, true, false, false);
        let ic = with(true, GlassStyle::Regular, false, true, false);
        assert_eq!((both.blur, both.refraction), (rt.blur, rt.refraction));
        assert_eq!(both.face, ic.face);
    }

    #[test]
    fn reduce_motion_flattens_refraction_and_widens_blur() {
        let p = with(true, GlassStyle::Regular, false, false, true);
        assert_eq!((p.refraction.amount, p.refraction.height), (-50.0, 0.0));
        assert_eq!(p.blur.radius, 8.0);
        assert_eq!(p.blur.level_opacity, [1.0, 1.0, 0.625, 0.625]);
    }

    #[test]
    fn the_blur_is_one_gaussian_twice_the_radius_scaled_by_the_first_level() {
        let sigma = |style, dark, side| GlassParams::new(mode(dark, style), side).blur_sigma();
        for dark in [true, false] {
            // regular glass: radius 5 at opacity 0.4 for controls up to 64 points, 0.8 from 160 points
            for side in [29.0, 36.0, 44.0, 60.0] {
                assert!(close(sigma(GlassStyle::Regular, dark, side), 4.0), "{side}");
            }
            assert!(close(
                sigma(GlassStyle::Regular, dark, 90.0),
                2.0 * 5.0 * 0.508_333_3
            ));
            assert!(close(sigma(GlassStyle::Regular, dark, 160.0), 8.0));
            assert!(close(sigma(GlassStyle::Regular, dark, 400.0), 8.0));
            // clear glass at radius 10 and every level opaque
            assert!(close(sigma(GlassStyle::Clear, dark, 160.0), 20.0));
            // the large panel blurs twice as wide as regular glass of its size
            assert!(close(sigma(GlassStyle::LargePanel, dark, 400.0), 16.0));
        }
        let p = GlassParams::new(mode(true, GlassStyle::Regular), 100.0);
        // the blur fill: twice its radius of 8 points
        assert!(close(p.fill_sigma(), 16.0));
        // reach: three deviations of the widest blur read (the fill's 16); the refraction reads inside the shape
        assert!(close(p.reach(), 48.0));
        // a deviation no wider than the cap however large the recipe's radius
        let rt_clear = with(true, GlassStyle::Clear, true, false, false);
        assert_eq!(rt_clear.blur_sigma(), BLUR_SIGMA_MAX);
        // Reduce Transparency: radius 40.625 at full opacity
        let rt = with(true, GlassStyle::Regular, true, false, false);
        assert!(close(rt.blur_sigma(), 81.25));
    }

    #[test]
    fn foreground_matrices_lift_or_drop_the_face() {
        let grey = [0.3, 0.3, 0.3, 1.0];
        let on_dark = apply_matrix(&foreground_matrix(true), grey);
        assert!(close(on_dark[0], 1.25) && on_dark[3] == 1.0);
        let on_light = apply_matrix(&foreground_matrix(false), grey);
        assert!(close(on_light[0], -0.7));
        assert!(adapts_foreground(40.0) && !adapts_foreground(64.0));
    }

    #[test]
    fn the_threshold_is_where_white_and_black_text_read_alike() {
        let grey = |l: f32| {
            // the encoded grey of relative luminance l
            let e = if l <= 0.0031308 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            [e; 3]
        };
        let face = grey(FACE_TONE_THRESHOLD);
        let (white, black) = (
            contrast_ratio([1.0; 3], face),
            contrast_ratio([0.0; 3], face),
        );
        assert!(
            (white - black).abs() < 0.05 && white > 4.5,
            "{white} {black}"
        );
        // the side each tone picks reads better than the other one over the faces it is picked for
        for l in [0.02, 0.1, 0.25, 0.6, 0.95] {
            let rgb = |m: ColorMatrix| {
                let o = apply_matrix(&m, [grey(l)[0], grey(l)[1], grey(l)[2], 1.0]);
                [o[0], o[1], o[2]].map(|v| v.clamp(0.0, 1.0))
            };
            let picked = face_tone(l, None);
            let other = match picked {
                FaceTone::Dark => FaceTone::Light,
                FaceTone::Light => FaceTone::Dark,
            };
            assert!(
                contrast_ratio(rgb(picked.matrix()), grey(l))
                    >= contrast_ratio(rgb(other.matrix()), grey(l)),
                "{l}"
            );
        }
    }

    #[test]
    fn the_tone_changes_only_past_the_hysteresis() {
        let t = FACE_TONE_THRESHOLD;
        let h = FACE_TONE_HYSTERESIS;
        assert_eq!(face_tone(t - 0.01, None), FaceTone::Dark);
        assert_eq!(face_tone(t + 0.01, None), FaceTone::Light);
        // a dark face that brightens a little past the threshold stays dark, and only flips past the band
        assert_eq!(face_tone(t + h * 0.5, Some(FaceTone::Dark)), FaceTone::Dark);
        assert_eq!(
            face_tone(t + h * 1.5, Some(FaceTone::Dark)),
            FaceTone::Light
        );
        assert_eq!(
            face_tone(t - h * 0.5, Some(FaceTone::Light)),
            FaceTone::Light
        );
        assert_eq!(
            face_tone(t - h * 1.5, Some(FaceTone::Light)),
            FaceTone::Dark
        );
        // a luminance wobbling inside the band never changes the tone, from either side
        for start in [FaceTone::Dark, FaceTone::Light] {
            let mut tone = start;
            for i in 0..100 {
                let wobble = (i as f32 * 0.7).sin() * h * 0.9;
                tone = face_tone(t + wobble, Some(tone));
                assert_eq!(tone, start);
            }
        }
    }

    #[test]
    fn unmeasured_and_large_shapes_keep_the_appearances_matrix() {
        assert_eq!(foreground_matrix_for(true, None), foreground_matrix(true));
        assert_eq!(foreground_matrix_for(false, None), foreground_matrix(false));
        // a small dark shape over a light face takes the light appearance's matrix, and the other way round
        assert_eq!(
            foreground_matrix_for(true, Some(FaceTone::Light)),
            foreground_matrix(false)
        );
        assert_eq!(
            foreground_matrix_for(false, Some(FaceTone::Dark)),
            foreground_matrix(true)
        );
    }

    #[test]
    fn module_tiles_are_clear_glass_with_a_strong_bending_edge() {
        // (short side, blur radius, refraction band): the clear blur rule and 0.225 of the side, at most 18 points
        for (side, blur, band) in [
            (170.0, 10.0, 18.0),
            (69.0, 6.458_333, 15.525),
            (40.0, 5.0, 9.0),
        ] {
            for dark in [false, true] {
                let p = GlassParams::new(mode(dark, GlassStyle::Module), side);
                let clear = GlassParams::new(mode(dark, GlassStyle::Clear), side);
                assert_eq!(p.face, clear_face(dark), "{side} {dark}");
                assert!(close(p.blur.radius, blur), "{side}: {}", p.blur.radius);
                assert_eq!(p.blur.level_opacity, [1.0; BLUR_LEVELS]);
                assert!(!p.fill_shows(), "no blur fill");
                assert_eq!(p.refraction.amount, -80.0);
                assert!(close(p.refraction.height, band), "{side}");
                let h = p.highlight;
                assert_eq!((h.amount, h.color_bias, h.height), (0.4, -0.3, 0.5));
                assert!(close(h.spread, 75f32.to_radians()));
                assert_eq!(h.matrix, HIGHLIGHT_MATRIX_CLEAR);
                assert_eq!((p.shadow, p.bleed), (clear.shadow, clear.bleed));
                assert_eq!(p.capture_scale, GLASS_BACKDROP_SCALE);
            }
        }
        // light: black lifted by 0.2, white dimmed to 0.95 under a white veil of 0.1; dark: 0.05, 0.8 and 0.05
        let light = clear_face(false);
        assert_eq!((light.black, light.white, light.fill[3]), (0.2, 0.95, 0.1));
        let dark = clear_face(true);
        assert_eq!((dark.black, dark.white, dark.fill[3]), (0.05, 0.8, 0.05));
    }

    #[test]
    fn a_card_is_clear_glass_over_a_wide_blur_and_the_blur_fill_read_at_a_quarter_scale() {
        for dark in [false, true] {
            for side in [56.0, 120.0] {
                let p = GlassParams::new(mode(dark, GlassStyle::Card), side);
                assert_eq!(p.face, clear_face(dark));
                // radius 10 at full opacity: a deviation of 20 points; the fill of radius 8
                assert!(close(p.blur_sigma(), 20.0) && close(p.fill_sigma(), 16.0));
                let b = p.blur;
                if dark {
                    assert_eq!((b.fill_darken, b.fill_lighten), (0.8821, 0.0));
                } else {
                    assert_eq!((b.fill_darken, b.fill_lighten), (0.0, 0.8821));
                }
                assert_eq!(b.fill_normal, 0.4602);
                assert_eq!((p.refraction.amount, p.refraction.height), (-60.0, 20.0));
                let h = p.highlight;
                assert_eq!(
                    (h.amount, h.color_bias, h.matrix),
                    (0.4, -0.25, HIGHLIGHT_MATRIX_REGULAR)
                );
                let spread = if dark { 75f32 } else { 96.0 };
                assert!((h.spread - spread.to_radians()).abs() < 1e-3, "{dark}");
                let s = p.shadow;
                assert_eq!(
                    (s.ring_opacity, s.ring_blur, s.ring_stroke, s.ring_offset),
                    (0.06, 5.0, 4.0, 8.0),
                    "the ring shows at rest"
                );
                assert_eq!(p.shadow_reach(), 0.0, "no drop shadow");
                assert_eq!(p.bleed.opacity, 0.0);
                assert_eq!(p.capture_scale, 0.25);
            }
        }
    }

    #[test]
    fn a_track_reads_the_backdrop_unblurred_and_bends_by_its_height() {
        for dark in [false, true] {
            let p = GlassParams::new(mode(dark, GlassStyle::Track), 24.0);
            assert_eq!((p.blur_sigma(), p.fill_shows()), (0.0, false));
            assert_eq!(p.capture_scale, 1.0);
            assert!(close(p.refraction.amount, -7.2) && close(p.refraction.height, 4.32));
            let f = p.face;
            assert_eq!(
                (f.saturation, f.black, f.white, f.max_luma, f.fill[3]),
                (1.0, 0.1, 1.0, 1.0, 0.0)
            );
            let h = p.highlight;
            assert_eq!(
                (h.amount, h.color_bias, h.height, h.offset),
                (0.5, -0.225, 1.0, -1.0)
            );
            let s = p.shadow;
            assert_eq!((s.ring_blur, s.ring_stroke, s.ring_offset), (3.0, 4.0, 8.0));
            assert_eq!(s.ring_opacity, if dark { 0.40 } else { 0.15 });
            // a faint bluish shadow below it
            assert_eq!(
                (s.offset, s.amount, s.height, s.opacity),
                ([0.0, 7.0], 6.0, 8.0, 0.0018)
            );
            assert!(s.fill[2] > s.fill[1] && s.fill[1] > s.fill[0]);
            // a taller track bends further
            let tall = GlassParams::new(mode(dark, GlassStyle::Track), 48.0);
            assert!(close(tall.refraction.amount, -14.4) && close(tall.refraction.height, 8.64));
        }
    }

    #[test]
    fn regular_and_clear_glass_light_the_far_end_of_their_soft_band_at_one_plus_their_rims_shift() {
        for style in [
            GlassStyle::Regular,
            GlassStyle::Dense,
            GlassStyle::LargePanel,
            GlassStyle::Clear,
        ] {
            for dark in [false, true] {
                let h = GlassParams::new(mode(dark, style), 40.0).highlight;
                assert_eq!(h.glow_fill, 1.0 + h.color_bias, "{style:?} dark={dark}");
            }
        }
    }

    #[test]
    fn a_popover_softens_only_through_its_blur_fill_and_lights_its_top_and_bottom_edges_at_every_size(
    ) {
        for dark in [false, true] {
            let small = GlassParams::new(mode(dark, GlassStyle::Popover), 26.0);
            let large = GlassParams::new(mode(dark, GlassStyle::Popover), 120.0);
            assert_eq!(small, large, "the same at every size");
            let p = small;
            assert_eq!(p.blur_sigma(), 0.0, "no blur of its own");
            assert_eq!(p.blur.fill_radius, 16.0);
            let over = if dark {
                p.blur.fill_darken
            } else {
                p.blur.fill_lighten
            };
            assert_eq!((over, p.blur.fill_normal), (0.882, 0.46));
            assert_eq!((p.refraction.amount, p.refraction.height), (-24.0, 5.0));
            let h = p.highlight;
            assert_eq!(
                (h.angle, h.glow_amount, h.glow_fill),
                (std::f32::consts::FRAC_PI_2, 0.5, 1.0)
            );
            assert_eq!((h.amount, h.color_bias), (0.4, -0.25));
            assert_eq!(h.diffuse.height_scale, 8.0);
            assert_eq!(p.shadow.ring_opacity, 0.06);
            assert_eq!(p.shadow.opacity, 0.0, "no drop shadow");
        }
        let light = GlassParams::new(mode(false, GlassStyle::Popover), 26.0).face;
        assert_eq!(
            (light.black, light.white, light.saturation, light.fill[3]),
            (0.4, 1.03, 1.2, REGULAR_FILL_LIGHT)
        );
        let dark = GlassParams::new(mode(true, GlassStyle::Popover), 26.0).face;
        assert_eq!(
            (dark.black, dark.white, dark.saturation, dark.max_luma),
            (0.125, 1.125, 1.3, 0.6)
        );
    }

    #[test]
    fn the_bar_blurs_narrowly_lifts_its_colour_and_lights_its_top_and_bottom_edges() {
        let light = GlassParams::new(mode(false, GlassStyle::Bar), 70.0);
        let dark = GlassParams::new(mode(true, GlassStyle::Bar), 70.0);
        for p in [light, dark] {
            assert!(close(p.blur_sigma(), 2.0 * 2.761));
            assert!(!p.fill_shows());
            assert_eq!((p.refraction.amount, p.refraction.height), (-8.64, 3.84));
            let h = p.highlight;
            assert_eq!((h.amount, h.color_bias), (0.4, -0.25));
            // the line of light along the top and the bottom, both alike, over a wide faint glow
            assert_eq!(
                (h.angle, h.glow_amount, h.glow_height, h.glow_fill),
                (std::f32::consts::FRAC_PI_2, 0.5, 0.6, 1.0)
            );
            // a dark hairline just outside the outline
            assert_eq!(
                (p.shadow.outline_opacity, p.shadow.outline_width),
                (0.45, 0.5)
            );
            assert_eq!(
                (
                    h.diffuse.height_scale,
                    h.diffuse.spread_scale,
                    h.diffuse.amount_scale
                ),
                (8.0, 0.65, 0.15)
            );
            assert!(close(h.spread, 80f32.to_radians()));
            assert_eq!(
                (p.shadow.amount, p.shadow.height, p.shadow.opacity),
                (30.0, 19.2, 0.0)
            );
            assert_eq!(p.capture_scale, GLASS_BACKDROP_SCALE);
        }
        let f = light.face;
        assert_eq!(
            (f.saturation, f.black, f.white, f.max_luma, f.fill[3]),
            (1.25, 0.15, 0.9, 1.0, 0.0)
        );
        let f = dark.face;
        assert_eq!(
            (f.saturation, f.black, f.white, f.max_luma),
            (1.25, 0.05, 0.92, 0.75)
        );
        assert_eq!(f.fill, [0.0, 0.0, 0.0, 0.02]);
        assert_eq!(light.highlight.matrix[4], 0.2);
        assert_eq!(dark.highlight.matrix[4], 0.25);
        // the same at every size
        assert_eq!(
            GlassParams::new(mode(false, GlassStyle::Bar), 120.0).refraction,
            light.refraction
        );
    }

    #[test]
    fn the_new_surfaces_keep_their_accessibility_recipes() {
        for style in [
            GlassStyle::Module,
            GlassStyle::Card,
            GlassStyle::Track,
            GlassStyle::Bar,
        ] {
            for dark in [false, true] {
                let base = with(dark, style, false, false, false);
                let rt = with(dark, style, true, false, false);
                let ic = with(dark, style, false, true, false);
                let what = format!("{style:?} dark {dark}");
                // Reduce Transparency: no bending, the wide blur and a dense face
                assert_eq!(
                    (rt.refraction.amount, rt.refraction.height),
                    (0.0, 0.0),
                    "{what}"
                );
                assert_eq!(rt.blur.radius, REDUCED_TRANSPARENCY_BLUR, "{what}");
                assert_eq!(rt.face, reduced_transparency_face(dark, style), "{what}");
                assert!(rt.face.fill[3] >= 0.6, "{what}");
                assert_eq!(rt.shadow.ring_stroke, 0.0, "{what}");
                // Increase Contrast: the optics kept, the contrast colours
                assert_eq!(
                    (ic.refraction, ic.blur),
                    (base.refraction, base.blur),
                    "{what}"
                );
                assert_eq!(
                    Some(ic.face),
                    increased_contrast_face(dark, style),
                    "{what}"
                );
                assert_ne!(ic.face, base.face, "{what}");
            }
        }
    }

    #[test]
    fn every_style_but_the_card_and_the_track_is_captured_at_half_scale() {
        for style in [
            GlassStyle::Regular,
            GlassStyle::Clear,
            GlassStyle::LargePanel,
            GlassStyle::Bar,
            GlassStyle::Dense,
            GlassStyle::Module,
        ] {
            assert_eq!(
                GlassParams::new(mode(false, style), 60.0).capture_scale,
                GLASS_BACKDROP_SCALE
            );
        }
        assert_eq!(
            GlassParams::new(mode(true, GlassStyle::Card), 56.0).capture_scale,
            CARD_CAPTURE_SCALE
        );
        assert_eq!(
            GlassParams::new(mode(true, GlassStyle::Track), 24.0).capture_scale,
            TRACK_CAPTURE_SCALE
        );
    }

    #[test]
    fn the_refraction_decays_from_nine_tenths_at_the_edge_to_nothing_at_the_band_s_inner_side() {
        assert!(close(refraction_profile(0.0), 0.9));
        // e^(−4.2 t) · (1 − t⁴) at a quarter, a half and three quarters of the band
        assert!(close(
            refraction_profile(0.25),
            0.9 * (-1.05f32).exp() * (1.0 - 0.003_906_25)
        ));
        assert!(close(
            refraction_profile(0.5),
            0.9 * (-2.1f32).exp() * 0.9375
        ));
        assert!(close(
            refraction_profile(0.75),
            0.9 * (-3.15f32).exp() * (1.0 - 0.316_406_25)
        ));
        assert_eq!(refraction_profile(1.0), 0.0);
        assert_eq!(refraction_profile(3.0), 0.0);
        // it falls all the way across the band, and is continuous where it ends
        let mut last = refraction_profile(0.0);
        for i in 1..=100 {
            let v = refraction_profile(i as f32 / 100.0);
            assert!(v < last, "{i}");
            last = v;
        }
        assert!(refraction_profile(0.999) < 1e-3);
    }

    #[test]
    fn bad_sizes_are_treated_as_empty() {
        assert_eq!(
            GlassParams::new(GlassMode::default(), f32::NAN),
            GlassParams::new(GlassMode::default(), 0.0)
        );
        assert_eq!(
            GlassParams::new(GlassMode::default(), -5.0)
                .refraction
                .amount,
            0.0
        );
    }
}
