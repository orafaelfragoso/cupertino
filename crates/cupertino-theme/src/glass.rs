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

/// The scale the backdrop of a glass container is captured at, relative to the output's pixels.
pub const GLASS_BACKDROP_SCALE: f32 = 0.5;

/// Number of blur levels of the multiscale backdrop. Level `i` is blurred with `blur.radius · 2^i` points.
pub const BLUR_LEVELS: usize = 4;

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
    /// The glass of a floating bar of icons (the Dock): the clear glass optics (a wide bending edge, a bright rim along
    /// the top and bottom) over a face that barely changes the scene, only a small lift of white, so the wallpaper stays
    /// readable through it.
    Bar,
    /// The glass of a dense floating surface with text on it (a menu, a popover, a notification card): regular glass over
    /// a face pressed to a mid grey veil, so any backdrop reads as a quiet smoky pane under light text.
    Dense,
    /// The glass of the tiles of a panel of modules (the control panel): regular optics over a face that lets the
    /// scene through darker, in both appearances, so white labels read over a bright sky.
    Module,
}

/// Everything besides the shape's size that selects a recipe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlassMode {
    pub dark: bool,
    pub style: GlassStyle,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
}

/// Blur of the multiscale backdrop and the blur fill laid over it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassBlur {
    /// Blur radius of the first level in points; level `i` doubles it `i` times.
    pub radius: f32,
    /// Opacity of each level laid over the wider ones, widest first: the result starts as level 3 and each narrower
    /// level is mixed in at its opacity.
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

/// The face colour: how the blurred backdrop is toned before the edge effects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassFace {
    /// Saturation about Rec. 709 luminance (1 leaves the colour).
    pub saturation: f32,
    /// Gain on linear light.
    pub white: f32,
    /// Lift of the encoded value towards white: `v + black · (1 − v)`.
    pub black: f32,
    /// Largest linear channel value after the gain; it also scales `max_luma`.
    pub clamp: f32,
    /// Largest linear luminance the face lets through, as a fraction of `clamp`; brighter backdrops are scaled down
    /// to it with their hue kept.
    pub max_luma: f32,
    /// Straight sRGB fill laid over the face.
    pub fill: [f32; 4],
}

/// Refraction at the inner edge of the shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassRefraction {
    /// Strength; negative values pull the image of what lies past the edge inward, as a convex rim does.
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
}

/// The rim light along the axis of the light: the key along the edge facing the light and the fill, the same light a
/// little weaker, along the opposite edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassHighlight {
    /// Strength of the rim light.
    pub amount: f32,
    /// Direction the light comes from, in radians, counter-clockwise from the right with y up (π/2 is the top).
    pub angle: f32,
    /// Angular width of each lit arc in radians; an arc fades to nothing three quarters of it from its end of the axis.
    pub spread: f32,
    /// Half-width of the rim band in points.
    pub height: f32,
    /// Where the band's centre lies relative to the edge, in points (negative is inside).
    pub offset: f32,
    /// Strength of the fill side: the key light is scaled by `1 + color_bias` there.
    pub color_bias: f32,
    /// Exponent on the lit arc's falloff.
    pub curvature: f32,
    /// A second, soft band of the same light along the same arcs: its strength (0 for none), its half-width and where
    /// its middle lies relative to the edge, in points.
    pub glow_amount: f32,
    pub glow_height: f32,
    pub glow_offset: f32,
    /// Colour of the light added to the rim, from the face colour under it.
    pub matrix: ColorMatrix,
}

/// The drop shadow and the ring shadow around the shape. Both are drawn at their opacity, which is 0 at rest.
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
    /// Blur radius in points of the ring shadow.
    pub ring_blur: f32,
    /// Width in points of the ring.
    pub ring_stroke: f32,
    /// Offset in points of the ring below the shape.
    pub ring_offset: f32,
    pub ring_opacity: f32,
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
/// Opacity of the two narrow blur levels of a regular shape by short side `(side, opacity)`; linear between, flat
/// outside: small shapes keep more of the sharper levels out, large ones let them in.
const NARROW_LEVEL_OPACITY_BY_SIDE: [(f32, f32); 3] = [(64.0, 0.4), (100.0, 0.55), (240.0, 0.8)];
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

/// The light regular face: a strong saturation under a pale veil, so a backdrop's colour comes through as a pastel of it
/// (a strong gain on the colour, then the black lift and the white fill that keep it light).
const REGULAR_SATURATION_LIGHT: f32 = 2.2;
const REGULAR_WHITE_LIGHT: f32 = 1.09;
const REGULAR_BLACK_LIGHT: f32 = 0.43;
const REGULAR_FILL_LIGHT: f32 = 0.215;

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

/// How strongly the rim light of bar glass shows along its top and bottom edges.
const BAR_HIGHLIGHT_AMOUNT: f32 = 0.6;
/// The white laid over bar glass in the dark appearance: a faint lift, so the bar is only just paler than the scene.
const BAR_LIFT_DARK: f32 = 0.03;
/// The white laid over bar glass in the light appearance: a little more, still clear enough to show the scene.
const BAR_LIFT_LIGHT: f32 = 0.12;

/// The face of bar glass: the blurred scene as it is, with a small lift of white.
fn bar_face(dark: bool) -> GlassFace {
    GlassFace {
        saturation: 1.0,
        white: 1.0,
        black: 0.0,
        clamp: 1.0,
        max_luma: 1.0,
        fill: [
            1.0,
            1.0,
            1.0,
            if dark { BAR_LIFT_DARK } else { BAR_LIFT_LIGHT },
        ],
    }
}

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

/// Saturation, gain on linear light, black lift and largest luminance (as a fraction of the clamp) of the dark dense
/// face, and the straight sRGB veil laid over it.
const DENSE_SATURATION_DARK: f32 = 1.62;
const DENSE_WHITE_DARK: f32 = 1.43;
const DENSE_BLACK_DARK: f32 = 0.054;
const DENSE_MAX_LUMA_DARK: f32 = 0.251;
const DENSE_FILL_DARK: [f32; 4] = [0.261, 0.261, 0.261, 0.482];

/// The face of module glass: a gain on linear light of about 0.6 with a little more colour, and a largest luminance
/// below the regular glass's, so a bright backdrop comes out a darker, more saturated tone of itself.
fn module_face() -> GlassFace {
    GlassFace {
        saturation: 1.32,
        white: 0.557,
        black: 0.034,
        clamp: 1.308_310_9,
        max_luma: 0.434,
        fill: [0.0; 4],
    }
}

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
/// 50), while strong colours keep much of their brightness, as their luminance is under the cap; the saturation is a
/// little under the regular glass's. Light: the regular light face.
fn large_panel_face(dark: bool) -> GlassFace {
    let regular = regular_face(dark);
    if dark {
        GlassFace {
            saturation: LARGE_PANEL_SATURATION_DARK,
            black: LARGE_PANEL_BLACK_DARK,
            max_luma: LARGE_PANEL_MAX_LUMA_DARK,
            ..regular
        }
    } else {
        regular
    }
}

/// The dark large panel's tone compression: saturation, black lift and largest face luminance (as a fraction of the
/// face's `clamp`). The white point sits well below the face's own, so a bright spot in the scene (a sunlit sky, a white
/// page, the light centre of a picture) comes through as a light patch near the panel's tint rather than a glare: white
/// comes out near 125.
pub const LARGE_PANEL_SATURATION_DARK: f32 = 1.15;
pub const LARGE_PANEL_BLACK_DARK: f32 = 0.15;
pub const LARGE_PANEL_MAX_LUMA_DARK: f32 = 0.102;
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
            GlassStyle::Module => Self::module(s),
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
                | GlassStyle::Module => {
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
            },
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

    /// The module glass: the dark regular recipe of its size over the module face ([`module_face`]), in both
    /// appearances.
    fn module(s: f32) -> Self {
        let mut p = Self::regular(true, s);
        p.face = module_face();
        p
    }

    /// The bar glass: clear glass with the bar face ([`bar_face`]) and a brighter rim.
    fn bar(dark: bool, s: f32) -> Self {
        let mut p = Self::clear(dark, s);
        p.face = bar_face(dark);
        p.highlight.amount = BAR_HIGHLIGHT_AMOUNT;
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
            },
        }
    }

    /// Blur radius in points of each level of the multiscale backdrop.
    pub fn level_radii(&self) -> [f32; BLUR_LEVELS] {
        std::array::from_fn(|i| self.blur.radius * (1u32 << i) as f32)
    }

    /// How much each level contributes once they are laid over each other (widest first, each narrower one mixed in
    /// at its opacity). The weights sum to 1.
    pub fn level_weights(&self) -> [f32; BLUR_LEVELS] {
        let o = self.blur.level_opacity.map(|v| v.clamp(0.0, 1.0));
        let mut w = [0.0; BLUR_LEVELS];
        let mut rest = 1.0;
        for i in 0..BLUR_LEVELS {
            let take = if i == BLUR_LEVELS - 1 { 1.0 } else { o[i] };
            w[i] = rest * take;
            rest *= 1.0 - take;
        }
        w
    }

    /// How far past the shape the backdrop is read, in points: the widest blur level that contributes, the blur fill
    /// and the refraction.
    pub fn reach(&self) -> f32 {
        let radii = self.level_radii();
        let widest = self
            .level_weights()
            .iter()
            .zip(radii)
            .filter(|(w, _)| **w > LEVEL_WEIGHT_EPSILON)
            .map(|(_, r)| r)
            .fold(0.0f32, f32::max);
        let fill = if self.blur.fill_darken + self.blur.fill_normal + self.blur.fill_lighten > 0.0 {
            self.blur.fill_radius
        } else {
            0.0
        };
        // a Gaussian of radius r is read three radii out; refraction pulls in from up to its band past the edge
        let read =
            3.0 * widest.max(fill) + self.refraction.amount.abs() * REFRACTION_REACH_PER_AMOUNT;
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

/// A blur level weighing less than this is not computed.
pub const LEVEL_WEIGHT_EPSILON: f32 = 1e-3;

/// How far past the edge, in points per unit of refraction strength, the refraction reads the backdrop.
pub const REFRACTION_REACH_PER_AMOUNT: f32 = 0.25;

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
/// How strongly a secondary label is drawn on glass.
pub const VIBRANT_SECONDARY: f32 = 0.75;
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
            (0.9, 0.5, 0.0)
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
            (1.2, 0.96, 0.4, 1.0)
        );
        assert_eq!(p.face.fill, [1.0, 1.0, 1.0, 0.2]);
        assert_eq!((p.blur.fill_darken, p.blur.fill_lighten), (0.0, 0.9));
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
            (80.0, -40.0, 20.0, 28.0, 0.466_67, 0.5),
            (100.0, -50.0, 20.0, 35.0, 0.55, 0.375),
            (120.0, -60.0, 20.0, 42.0, 0.633_33, 0.35),
            (140.0, -60.0, 20.0, 49.0, 0.716_67, 0.35),
            (180.0, -60.0, 20.0, 49.0, 0.716_67, 0.35),
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
            (180.0, -60.0, 20.0, 10.0, 75.0, 56.0),
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
                (panel.face.white, panel.face.clamp, panel.face.fill),
                (regular.face.white, regular.face.clamp, regular.face.fill)
            );
            // a lower white point, a lifted black and a little less saturation
            assert!(panel.face.max_luma < regular.face.max_luma, "{dark}");
            assert!(panel.face.black > regular.face.black, "{dark}");
            assert!(panel.face.saturation < regular.face.saturation, "{dark}");
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
            assert_eq!(panel.shadow_reach(), 16.0 + 1.5 * 28.0);
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
    fn level_weights_sum_to_one_and_drop_unused_levels() {
        let p = GlassParams::new(mode(true, GlassStyle::Regular), 100.0);
        let w = p.level_weights();
        assert!(close(w.iter().sum::<f32>(), 1.0));
        assert!(close(w[0], 0.55) && close(w[1], 0.45 * 0.55) && close(w[2], 0.45 * 0.45));
        assert_eq!(w[3], 0.0, "level 2 is opaque, so level 3 never shows");
        let rt = with(true, GlassStyle::Regular, true, false, false).level_weights();
        assert_eq!(rt, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(p.level_radii(), [5.0, 10.0, 20.0, 40.0]);
        // reach: three radii of the widest level used (20) plus the refraction's pull
        assert!(close(p.reach(), 60.0 + 50.0 * REFRACTION_REACH_PER_AMOUNT));
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
