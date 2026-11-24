//! The glass of the control centre's modules, the Dock bar's lines, the window sidebars' dense pane, the notification
//! cards' edges, the launcher's results panel and the cartouches' edge lines, kept under the glass variant names, and
//! the control centre's accessibility shifts laid over its face.

use super::clear::clear_base;
use super::*;

// ---- the control centre ----------------------------------------------------------------------------------------

/// The angular width of the arcs the module's line lights, in radians: the whole top and bottom, fading round the
/// corners.
const MODULE_LINE_SPREAD: f32 = std::f32::consts::FRAC_PI_2;
/// Module glass: the clear glass of its size with a strong bending edge and a darker rim. The strength is the same at
/// every size; the band is [`MODULE_REFRACTION_HEIGHT_PER_POINT`] of the short side, at most
/// [`MODULE_REFRACTION_HEIGHT_MAX`] points.
const MODULE_REFRACTION: f32 = -80.0;
const MODULE_REFRACTION_HEIGHT_PER_POINT: f32 = 0.18;
const MODULE_REFRACTION_HEIGHT_MAX: f32 = 25.0;
/// The module's blur radius by short side `(side, radius)`; linear between, flat outside.
const MODULE_BLUR_BY_SIDE: [(f32, f32); 2] = [(60.0, 5.0), (140.0, 10.0)];
const MODULE_RIM_BIAS: f32 = -0.3;
/// The module's face, the same in both appearances: a little more colour, black lifted to about 27 of 255 and white held
/// near 178, so every channel comes out at about `0.59 · c + 27` and a sunlit backdrop as a mid tone of its own colour.
pub const MODULE_SATURATION: f32 = 1.045;
pub const MODULE_WHITE: f32 = 0.685;
pub const MODULE_BLACK: f32 = 0.107;
/// The module's line of light just inside its top and bottom edges: its strength (the share of its colour added at its
/// middle), its half-width and where its middle lies inside the edge, in points, and the bottom's strength as a share
/// of the top's: a bright line about a point wide along the top and as bright along the bottom, which takes a
/// mid-toned face to near white and a dark warm one to a pale tint of it.
pub const MODULE_LINE: (f32, f32, f32) = (0.65, 0.45, -0.35);
pub const MODULE_LINE_BOTTOM: f32 = 1.0;
/// The soft glow the module's line of light spreads inwards under it: about a tenth of the line's light a point inside
/// the edge, fading out some five points in, along the same arcs.
pub const MODULE_LINE_DIFFUSE: GlowDiffuse = GlowDiffuse {
    height_scale: 6.0,
    spread_scale: 1.0,
    amount_scale: 0.2,
};
/// The exponent on the module line's arc falloff: under 1, so the line keeps most of its light round the corners until
/// the side's dark line takes over.
const MODULE_LINE_CURVATURE: f32 = 0.5;
/// The colour of the module's line of light from the face under it: the face plus nearly half white, so a mid tone of
/// a warm backdrop turns a pale tint of it and a light one white.
const MODULE_LINE_MATRIX: ColorMatrix = [
    1.0, 0.0, 0.0, 0.0, 0.47, //
    0.0, 1.0, 0.0, 0.0, 0.47, //
    0.0, 0.0, 1.0, 0.0, 0.47, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// The dark line just outside the module's outline, which marks its sides: its strength, the same over any backdrop,
/// and the angular width of the arcs facing left and right it keeps to (none along the top and the bottom, which carry
/// the line of light).
const MODULE_EDGE_LINE: f32 = 0.64;
const MODULE_EDGE_LINE_SIDES: f32 = std::f32::consts::FRAC_PI_2;

/// Subdued module glass: how much wider its blur is than the module's of the same size, and its face's white (light)
/// and black (dark).
const SUBDUED_MODULE_BLUR_SCALE: f32 = 2.0;
const SUBDUED_MODULE_WHITE_LIGHT: f32 = 0.88;
const SUBDUED_MODULE_BLACK_DARK: f32 = 0.0;

/// The control centre's shifts of its face with Increase Contrast: the light face's black lift and the dark face's
/// white gain (relative to the face at rest).
pub const CONTROL_CENTER_CONTRAST_BLACK_LIGHT: f32 = -0.2;
pub const CONTROL_CENTER_CONTRAST_WHITE_DARK: f32 = 0.55;
/// The dark face's luma cap with Increase Contrast: the white gain lifts the face over a dark backdrop, and the
/// cap keeps a bright one from carrying it towards white, so the module's light labels and symbols stay at about 4:1
/// over any backdrop rather than vanishing into a sunlit one.
pub const CONTROL_CENTER_CONTRAST_MAX_LUMA_DARK: f32 = 0.35;
/// The control centre with Reduce Transparency: the blur radius in points at full opacity, the shifts of the face's
/// black lift and white gain, light and dark (relative to the face at rest), and the dark face's saturation
/// (the clear glass's).
pub const CONTROL_CENTER_REDUCED_BLUR: f32 = 10.0;
pub const CONTROL_CENTER_REDUCED_BLACK_LIGHT: f32 = -0.25;
pub const CONTROL_CENTER_REDUCED_WHITE_LIGHT: f32 = -0.11;
pub const CONTROL_CENTER_REDUCED_BLACK_DARK: f32 = 0.05;
pub const CONTROL_CENTER_REDUCED_WHITE_DARK: f32 = 0.10;
const CONTROL_CENTER_REDUCED_SATURATION_DARK: f32 = 0.7;

/// The control centre's module glass, the same in both appearances at rest: the clear glass of its size with the
/// module's blur, face, refraction and rim, a line of light inside its top and bottom edges and a dark line outside its
/// sides; subdued, twice the blur and a quieter face. Increase Contrast and Reduce Transparency
/// shift its face.
pub(super) fn control_center(mode: GlassMode, s: f32) -> GlassParams {
    let dark = mode.dark;
    let mut p = clear_base(s);
    p.blur.radius = by_side(&MODULE_BLUR_BY_SIDE, s);
    p.face = GlassFace {
        saturation: MODULE_SATURATION,
        white: MODULE_WHITE,
        black: MODULE_BLACK,
        clamp: 1.0,
        max_luma: 1.0,
        fill: [0.0; 4],
    };
    p.refraction.amount = MODULE_REFRACTION;
    p.refraction.height =
        (MODULE_REFRACTION_HEIGHT_PER_POINT * s).min(MODULE_REFRACTION_HEIGHT_MAX);
    p.highlight.color_bias = MODULE_RIM_BIAS;
    let (amount, height, offset) = MODULE_LINE;
    p.highlight.glow_amount = amount;
    p.highlight.glow_height = height;
    p.highlight.glow_offset = offset;
    p.highlight.glow_fill = MODULE_LINE_BOTTOM;
    p.highlight.spread = MODULE_LINE_SPREAD;
    p.highlight.curvature = MODULE_LINE_CURVATURE;
    p.highlight.diffuse = MODULE_LINE_DIFFUSE;
    p.highlight.matrix = MODULE_LINE_MATRIX;
    p.shadow.edge_line = MODULE_EDGE_LINE;
    p.shadow.edge_line_sides = MODULE_EDGE_LINE_SIDES;
    p.shadow.edge_line_even = true;
    if mode.modifiers.subdued {
        p.blur.radius *= SUBDUED_MODULE_BLUR_SCALE;
        if dark {
            p.face.black = SUBDUED_MODULE_BLACK_DARK;
        } else {
            p.face.white = SUBDUED_MODULE_WHITE_LIGHT;
        }
    }
    let f = &mut p.face;
    if mode.increase_contrast {
        if dark {
            f.white += CONTROL_CENTER_CONTRAST_WHITE_DARK;
            f.max_luma = CONTROL_CENTER_CONTRAST_MAX_LUMA_DARK;
        } else {
            f.black += CONTROL_CENTER_CONTRAST_BLACK_LIGHT;
        }
    }
    if mode.reduce_transparency {
        if dark {
            f.black += CONTROL_CENTER_REDUCED_BLACK_DARK;
            f.white += CONTROL_CENTER_REDUCED_WHITE_DARK;
            f.saturation = CONTROL_CENTER_REDUCED_SATURATION_DARK;
        } else {
            f.black += CONTROL_CENTER_REDUCED_BLACK_LIGHT;
            f.white += CONTROL_CENTER_REDUCED_WHITE_LIGHT;
        }
        p.blur.radius = CONTROL_CENTER_REDUCED_BLUR;
        p.blur.level_opacity = [1.0; BLUR_LEVELS];
        p.refraction.amount = 0.0;
        p.refraction.height = 0.0;
    }
    p
}

// ---- the Dock --------------------------------------------------------------------------------------------------

/// The Dock bar's light along its outline, round its ends as they present at device pixels
/// (`dock/dock_dark_rest_67.883`, `dock_light_rest_328.340`): a bright line along the top and the bottom (the outermost
/// pixel about 89 levels over the backdrop, the next 52), at about three quarters of it 10° to 15° round each end and
/// half 25° round, a faint line at about a quarter of the top's light that runs on down the curve of the end and is
/// gone where the end runs straight down, and under the top and the bottom a soft lift that fades over four points.
///
/// The faint line round the end and the soft lift are the soft band of light: its strength, half-width and middle in
/// points (inside for a negative value), on arcs reaching the middle of each end with an exponent on their falloff that
/// keeps them nearly flat until close to it; the light bar's is a little weaker, as its face is lighter
/// ([`DOCK_BAR_LINE_LIGHT`]).
pub const DOCK_BAR_LINE: (f32, f32, f32) = (0.27, 0.6, -0.25);
pub const DOCK_BAR_LINE_LIGHT: f32 = 0.235;
const DOCK_BAR_LINE_SPREAD: f32 = 120.0 * std::f32::consts::PI / 180.0;
const DOCK_BAR_LINE_CURVATURE: f32 = 0.25;
const DOCK_BAR_LINE_BOTTOM: f32 = 1.0;
/// The soft lift under the top and the bottom: five times as wide as the faint line, on arcs half as wide (gone 45°
/// round each end), at 0.6 of its strength: about 16 levels a point inside the edge, 9 at two points, 3 at four.
const DOCK_BAR_LINE_DIFFUSE: GlowDiffuse = GlowDiffuse {
    height_scale: 5.0,
    spread_scale: 0.5,
    amount_scale: 0.6,
};
/// The bright line along the top and the bottom: white light from straight above and, as strong, from below, at this
/// strength over a band this many points deep, on arcs of this half-width.
pub const DOCK_BAR_PEAK: f32 = 0.175;
const DOCK_BAR_PEAK_DEPTH: f32 = 1.4;
const DOCK_BAR_PEAK_SPREAD: f32 = 32.0 * std::f32::consts::PI / 180.0;
/// The dark line one device pixel wide just outside the bar's ends: black at this alpha over any backdrop, on arcs this
/// wide facing left and right, so it is darkest across the middle of each end, about a third as dark 25° round it,
/// gone 40° round and along the top and the bottom, which carry the line of light (about 0.6 over the backdrop at the
/// middle of an end, in both appearances).
pub const DOCK_BAR_EDGE_LINE: f32 = 0.6;
const DOCK_BAR_EDGE_LINE_SIDES: f32 = 55.0 * std::f32::consts::PI / 180.0;
/// The colour of the bar's line of light from the face under it: a little more saturated and lifted than the face, in
/// the light and the dark appearance.
const DOCK_BAR_LINE_MATRIX_LIGHT: ColorMatrix = [
    1.2625, -0.2385, -0.024, 0.0, 0.2, //
    -0.0708, 1.0949, -0.0241, 0.0, 0.2, //
    -0.0709, -0.2383, 1.3093, 0.0, 0.2, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
const DOCK_BAR_LINE_MATRIX_DARK: ColorMatrix = [
    1.0937, -0.3577, -0.036, 0.0, 0.25, //
    -0.1063, 0.8424, -0.0361, 0.0, 0.25, //
    -0.1064, -0.3575, 1.1639, 0.0, 0.25, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// The Dock bar's lines over its recipe `p` (the Dock variant on the clear base, whose blur, face, rim and bends inside
/// and outside the edge it keeps): a thin line of light along its top and bottom edges, equally bright and running
/// faintly down the curve of each end, over a soft lift of white, and a dark line just outside its ends, which the
/// recipe's inputs alone leave out.
pub(super) fn dock_bar(p: &mut GlassParams, dark: bool) {
    let (amount, height, offset) = DOCK_BAR_LINE;
    p.highlight = GlassHighlight {
        spread: DOCK_BAR_LINE_SPREAD,
        curvature: DOCK_BAR_LINE_CURVATURE,
        glow_amount: if dark { amount } else { DOCK_BAR_LINE_LIGHT },
        glow_height: height,
        glow_offset: offset,
        glow_fill: DOCK_BAR_LINE_BOTTOM,
        diffuse: DOCK_BAR_LINE_DIFFUSE,
        matrix: if dark {
            DOCK_BAR_LINE_MATRIX_DARK
        } else {
            DOCK_BAR_LINE_MATRIX_LIGHT
        },
        ..p.highlight
    };
    p.specular = GlassSpecular {
        amount: DOCK_BAR_PEAK,
        spread: DOCK_BAR_PEAK_SPREAD,
        height: DOCK_BAR_PEAK_DEPTH,
        angle: std::f32::consts::FRAC_PI_2,
        fill: 1.0,
        diffuse: GlowDiffuse::NONE,
    };
    p.shadow.edge_line = DOCK_BAR_EDGE_LINE;
    p.shadow.edge_line_sides = DOCK_BAR_EDGE_LINE_SIDES;
    p.shadow.edge_line_even = true;
}

// ---- the window sidebars -----------------------------------------------------------------------------------------

/// A window sidebar's blur: its radius in points at full opacity of every level, and the blur fill's radius, its
/// lighten (light) or darken (dark) opacity and its normal opacity.
const WINDOW_SIDEBAR_BLUR: f32 = 5.0;
const WINDOW_SIDEBAR_FILL_RADIUS: f32 = 8.0;
const WINDOW_SIDEBAR_FILL_BLEND: f32 = 0.882;
const WINDOW_SIDEBAR_FILL_NORMAL: f32 = 0.48;
/// A window sidebar's bend inside its edge: strength (inwards) and band in points.
const WINDOW_SIDEBAR_REFRACTION: f32 = -24.0;
const WINDOW_SIDEBAR_REFRACTION_HEIGHT: f32 = 12.0;
/// How much of the backdrop's departure from grey a window sidebar keeps, in both appearances.
pub const WINDOW_SIDEBAR_SATURATION: f32 = 0.1;
/// The dark window sidebar's black and white levels: a grey `x` comes out at about `0.034 · x + 34` of 255.
pub const WINDOW_SIDEBAR_BLACK_DARK: f32 = 0.1325;
pub const WINDOW_SIDEBAR_WHITE_DARK: f32 = 0.1665;
/// The light window sidebar's black and white levels and the warm light grey veil over them: a grey `x` comes out at
/// about `0.02 · x + 229` of 255.
pub const WINDOW_SIDEBAR_BLACK_LIGHT: f32 = 0.9;
pub const WINDOW_SIDEBAR_WHITE_LIGHT: f32 = 0.94;
pub const WINDOW_SIDEBAR_VEIL_LIGHT: [f32; 4] = [0.9094, 0.8969, 0.8827, 0.5];

/// A window's sidebar or inspector over its planar recipe `p`, as it presents: a dense pane that lets only a faint tint
/// of what lies behind the window through (black near 34 and white near 42 of 255 in the dark appearance, near 229 and
/// 234 under a warm grey veil in the light one), with the sidebar's blur, blur fill and bend, and no edge bleed, which
/// would bring the scene outside the window into the pane's edge.
pub(super) fn window_sidebar(p: &mut GlassParams, dark: bool) {
    p.blur = GlassBlur {
        radius: WINDOW_SIDEBAR_BLUR,
        level_opacity: [1.0; BLUR_LEVELS],
        fill_radius: WINDOW_SIDEBAR_FILL_RADIUS,
        fill_darken: if dark { WINDOW_SIDEBAR_FILL_BLEND } else { 0.0 },
        fill_normal: WINDOW_SIDEBAR_FILL_NORMAL,
        fill_lighten: if dark { 0.0 } else { WINDOW_SIDEBAR_FILL_BLEND },
    };
    p.face = if dark {
        GlassFace {
            saturation: WINDOW_SIDEBAR_SATURATION,
            white: WINDOW_SIDEBAR_WHITE_DARK,
            black: WINDOW_SIDEBAR_BLACK_DARK,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [0.0; 4],
        }
    } else {
        GlassFace {
            saturation: WINDOW_SIDEBAR_SATURATION,
            white: WINDOW_SIDEBAR_WHITE_LIGHT,
            black: WINDOW_SIDEBAR_BLACK_LIGHT,
            clamp: 1.0,
            max_luma: 1.0,
            fill: WINDOW_SIDEBAR_VEIL_LIGHT,
        }
    };
    p.refraction.amount = WINDOW_SIDEBAR_REFRACTION;
    p.refraction.height = WINDOW_SIDEBAR_REFRACTION_HEIGHT;
    p.bleed.opacity = 0.0;
}

// ---- the notification centre -------------------------------------------------------------------------------------

/// The notification card's edge line in the light appearance, all round.
const CARD_EDGE_LINE_LIGHT: f32 = 0.19;
/// The dark card's edge line, as the module's: a dark line just outside its sides only, its strength over any
/// backdrop, on arcs as wide as the module's.
const CARD_EDGE_LINE_DARK: f32 = 0.62;

/// The notification centre's cards over the regular base `p`: the card's edge lines, read from a backdrop captured at
/// [`CARD_CAPTURE_SCALE`].
pub(super) fn notification_center(p: &mut GlassParams, dark: bool) {
    let sh = &mut p.shadow;
    if dark {
        sh.edge_line = CARD_EDGE_LINE_DARK;
        sh.edge_line_sides = MODULE_EDGE_LINE_SIDES;
        sh.edge_line_even = true;
    } else {
        sh.edge_line = CARD_EDGE_LINE_LIGHT;
    }
    p.capture_scale = CARD_CAPTURE_SCALE;
}

// ---- the cartouches ------------------------------------------------------------------------------------------------

/// The edge line of the minimum cartouche (the dock's labels) in the light appearance; the dark one has none.
const CARTOUCHE_EDGE_LINE_LIGHT: f32 = 0.175;
/// The edge line of larger cartouches (the dock's stack grid), light and dark.
const CARTOUCHE_EDGE_LINE_WIDE_LIGHT: f32 = 0.31;
const CARTOUCHE_EDGE_LINE_WIDE_DARK: f32 = 0.54;

/// The edge line of a cartouche resolved at `size`.
pub(super) fn cartouche_edge_line(size: GlassSize, dark: bool) -> f32 {
    match (size, dark) {
        (GlassSize::Minimum, false) => CARTOUCHE_EDGE_LINE_LIGHT,
        (GlassSize::Minimum, true) => 0.0,
        (_, false) => CARTOUCHE_EDGE_LINE_WIDE_LIGHT,
        (_, true) => CARTOUCHE_EDGE_LINE_WIDE_DARK,
    }
}

// ---- the launcher's results panel ----------------------------------------------------------------------------------

/// The launcher's results panel over the regular base `p`, whose bend, edge bleed, rim, ring and key and fill light it
/// keeps: a blur twice as wide, its own face (unless Increase Contrast gave it the contrast face) and its drop shadow
/// shown.
pub(super) fn search_results(p: &mut GlassParams, dark: bool, contrast: bool) {
    p.blur.radius *= LARGE_PANEL_BLUR_SCALE;
    p.blur.fill_radius *= LARGE_PANEL_BLUR_SCALE;
    if !contrast {
        p.face = large_panel_face(dark, p.face);
    }
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
}

/// The launcher's results panel with Reduce Transparency, in the dark appearance without Increase Contrast: a near-black
/// sheet with only a faint trace of the colours behind it.
pub(super) fn search_results_reduced(p: &mut GlassParams) {
    p.face.saturation = LARGE_PANEL_REDUCED_SATURATION_DARK;
}

/// Face of the large panel. Dark: white behind it comes out a mid grey (about 125 of 255) and black a dark grey (about
/// 53), while strong colours keep much of their brightness, as their luma is compressed less; the saturation is a
/// little under the regular glass's. Light: a bright sheet that keeps the scene's colour: every channel comes out at
/// about `0.61 · c + 124` of 255 after a moderate saturation, so black comes through as a light grey near 124, a dark warm
/// scene as a light tint of its own hue, and anything from a mid tone up is near white.
fn large_panel_face(dark: bool, regular: GlassFace) -> GlassFace {
    if dark {
        GlassFace {
            saturation: LARGE_PANEL_SATURATION_DARK,
            white: LARGE_PANEL_WHITE_DARK,
            black: LARGE_PANEL_BLACK_DARK,
            max_luma: LARGE_PANEL_MAX_LUMA_DARK,
            fill: [0.0; 4],
            ..regular
        }
    } else {
        GlassFace {
            saturation: LARGE_PANEL_SATURATION_LIGHT,
            white: LARGE_PANEL_WHITE_LIGHT,
            black: LARGE_PANEL_BLACK_LIGHT,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, LARGE_PANEL_FILL_LIGHT],
        }
    }
}

/// The light large panel's face: saturation, white and black levels (no luma compression) and the white veil over it.
/// A grey `x` comes out at `(1 − fill) · (b + (w − b) · x) + fill`: 0.61 per unit of the scene above 0.486.
pub const LARGE_PANEL_SATURATION_LIGHT: f32 = 1.4;
pub const LARGE_PANEL_WHITE_LIGHT: f32 = 1.118;
pub const LARGE_PANEL_BLACK_LIGHT: f32 = 0.37;
pub const LARGE_PANEL_FILL_LIGHT: f32 = 0.184;
/// The dark large panel's saturation with Reduce Transparency: the near-black sheet keeps a tenth or so of the
/// backdrop's colour (its departure from grey), so a strong colour behind shows only as a faint tint.
pub const LARGE_PANEL_REDUCED_SATURATION_DARK: f32 = 0.3;
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
