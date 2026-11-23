//! The regular glass and the variants built on it: their recorded values, the variant stages laid over the base, the
//! inactive branch and the window shadows.

use super::fitted;
use super::*;

// ---- the regular base (recorded) -------------------------------------------------------------------------------

/// The regular blur radius in points (the filter's half of the recipe's 10), scaled by the first level's opacity.
const REGULAR_BLUR: f32 = 5.0;
/// Opacity of the first blur level of regular glass by short side `(side, opacity)`, which scales its blur radius;
/// linear between, flat outside: a control blurs with 0.4 of the radius, a panel 160 points high or more with 0.8.
const NARROW_LEVEL_OPACITY_BY_SIDE: [(f32, f32); 2] = [(64.0, 0.4), (160.0, 0.8)];
/// The regular blur fill: its radius in points (the filter's half of the recipe's 16), its lighten (light) or darken
/// (dark) opacity and its normal opacity.
const REGULAR_FILL_RADIUS: f32 = 8.0;
const REGULAR_FILL_BLEND: f32 = 0.9;
const REGULAR_FILL_NORMAL: f32 = 0.5;
/// The light regular face: black lift, white gain, saturation, luma cap and the white normal fill's opacity.
const REGULAR_BLACK_LIGHT: f32 = 0.4;
const REGULAR_WHITE_LIGHT: f32 = 1.03;
const REGULAR_SATURATION_LIGHT: f32 = 1.2;
const REGULAR_MAX_LUMA_LIGHT: f32 = 0.94;
const REGULAR_FILL_LIGHT: f32 = 0.2;
/// The dark regular face: black lift, white gain and saturation; no normal fill.
const REGULAR_BLACK_DARK: f32 = 0.125;
const REGULAR_WHITE_DARK: f32 = 1.125;
const REGULAR_SATURATION_DARK: f32 = 1.3;
/// The dark regular face's luma cap by short side `(side, max luma)`; linear between, flat outside.
const DARK_MAX_LUMA_BY_SIDE: [(f32, f32); 2] = [(64.0, 0.6), (104.0, 0.35)];
/// The largest encoded channel of a face: the unit range.
pub(super) const FACE_CLAMP: f32 = 1.0;
/// The inner refraction per point of short side (negative: inwards), and its band per point of short side, each at
/// most the cap below.
const REFRACTION_PER_POINT_REGULAR: f32 = 0.5;
const REFRACTION_HEIGHT_PER_POINT_REGULAR: f32 = 0.25;
const REFRACTION_AMOUNT_MAX: f32 = 60.0;
const REFRACTION_HEIGHT_MAX: f32 = 20.0;
/// The regular edge bleed's band, reach and blur as fractions of the short side, and its opacity.
const BLEED_NORMALIZED: f32 = 0.35;
const BLEED_OPACITY: f32 = 0.2;
/// The widest blur of the edge bleed, in points: the blur fill's deviation, so a shape past about 45.7 points (where
/// its fraction of the side reaches it) reads the copy its blur fill makes instead of blurring one of its own, while a
/// control keeps its own (a 36-point capsule blurs its bleed by 12.6 points).
pub const BLEED_BLUR_MAX: f32 = FILL_SIGMA_PER_RADIUS * REGULAR_FILL_RADIUS;
/// The farthest outside its edge the edge bleed reads, in points, as far as its widest blur: the bleed of any shape
/// reads at most this and three of those deviations (64 points) around it, where a 300-point panel read 420.
pub const BLEED_AMOUNT_MAX: f32 = 16.0;
/// The edge bleed's colour: its black lift and saturation in the light appearance (white kept, darken blending), and
/// its black lift and white in the dark one (saturation kept, normal blending).
const BLEED_BLACK_LIGHT: f32 = 0.9;
const BLEED_SATURATION_LIGHT: f32 = 1.2;
const BLEED_BLACK_DARK: f32 = 0.125;
const BLEED_WHITE_DARK: f32 = 0.5;
/// The rim: strength, colour shift per unit of it and depth in points.
const REGULAR_RIM: f32 = 0.5;
const REGULAR_RIM_BIAS: f32 = -0.25;
const REGULAR_RIM_HEIGHT: f32 = 0.5;
/// The ring shadow inside the outline: its Gaussian deviation, stroke and downward offset in points, and its opacity.
const REGULAR_RING_BLUR: f32 = 5.0;
const REGULAR_RING_STROKE: f32 = 4.0;
const REGULAR_RING_OFFSET: f32 = 8.0;
const REGULAR_RING_OPACITY: f32 = 0.06;
/// The key and fill light: strength, depth in points of its band, the fill's strength per unit of the key's, and the
/// half-width of each arc in the light and the dark appearance (96° and 75°).
const SPECULAR_AMOUNT: f32 = 0.4;
const SPECULAR_HEIGHT: f32 = 0.75;
const SPECULAR_FILL: f32 = 1.0;
const SPECULAR_SPREAD_LIGHT: f32 = 96.0 * std::f32::consts::PI / 180.0;
const SPECULAR_SPREAD_DARK: f32 = 75.0 * std::f32::consts::PI / 180.0;
/// The diffuse glow under the key and fill light: eight times as deep, on arcs 0.65 as wide, at 0.15 of its strength.
const SPECULAR_DIFFUSE: GlowDiffuse = GlowDiffuse {
    height_scale: 8.0,
    spread_scale: 0.65,
    amount_scale: 0.15,
};
/// The edge line of regular glass in the light and the dark appearance ([`GlassShadow::edge_line`]; fitted to
/// captures).
const REGULAR_EDGE_LINE_LIGHT: f32 = 0.2;
const REGULAR_EDGE_LINE_DARK: f32 = 0.22;
/// The colour of the soft band's light from the face under it, when an interaction state lights it: a little more
/// saturated and lifted than the face (fitted to captures).
pub(super) const HIGHLIGHT_MATRIX_REGULAR: ColorMatrix = [
    1.1202, -0.1894, -0.019, 0.0, 0.1471, //
    -0.0563, 0.9871, -0.0191, 0.0, 0.1471, //
    -0.0563, -0.1893, 1.1574, 0.0, 0.1471, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// How strongly the soft band lights the far end of its axis on regular glass, as a share of the near end (fitted to
/// captures).
const GLOW_FILL_REGULAR: f32 = 0.8125;

/// The key and fill light of regular glass in the dark appearance or the light one.
pub(super) fn regular_specular(dark: bool) -> GlassSpecular {
    GlassSpecular {
        amount: SPECULAR_AMOUNT,
        spread: if dark {
            SPECULAR_SPREAD_DARK
        } else {
            SPECULAR_SPREAD_LIGHT
        },
        height: SPECULAR_HEIGHT,
        angle: HIGHLIGHT_ANGLE,
        fill: SPECULAR_FILL,
        diffuse: SPECULAR_DIFFUSE,
    }
}

/// The regular face of a shape resolved at short side `s`.
fn regular_face(dark: bool, s: f32) -> GlassFace {
    if dark {
        GlassFace {
            saturation: REGULAR_SATURATION_DARK,
            white: REGULAR_WHITE_DARK,
            black: REGULAR_BLACK_DARK,
            clamp: FACE_CLAMP,
            max_luma: by_side(&DARK_MAX_LUMA_BY_SIDE, s),
            fill: [0.0; 4],
        }
    } else {
        GlassFace {
            saturation: REGULAR_SATURATION_LIGHT,
            white: REGULAR_WHITE_LIGHT,
            black: REGULAR_BLACK_LIGHT,
            clamp: FACE_CLAMP,
            max_luma: REGULAR_MAX_LUMA_LIGHT,
            fill: [1.0, 1.0, 1.0, REGULAR_FILL_LIGHT],
        }
    }
}

/// The regular glass resolved at short side `s`: every stage the regular recipe records.
pub(super) fn regular_base(dark: bool, s: f32) -> GlassParams {
    let narrow = by_side(&NARROW_LEVEL_OPACITY_BY_SIDE, s);
    let mut bleed = GlassBleed {
        amount: 0.0,
        height: 0.0,
        blur: 0.0,
        white: if dark { BLEED_WHITE_DARK } else { 1.0 },
        black: if dark {
            BLEED_BLACK_DARK
        } else {
            BLEED_BLACK_LIGHT
        },
        saturation: if dark { 1.0 } else { BLEED_SATURATION_LIGHT },
        darken: !dark,
        opacity: BLEED_OPACITY,
    }
    .normalized(
        s,
        BLEED_NORMALIZED,
        BLEED_NORMALIZED,
        BLEED_NORMALIZED,
        BLEED_BLUR_MAX,
    );
    bleed.amount = bleed.amount.min(BLEED_AMOUNT_MAX);
    GlassParams {
        blur: GlassBlur {
            radius: REGULAR_BLUR,
            level_opacity: [narrow, narrow, 1.0, 1.0],
            fill_radius: REGULAR_FILL_RADIUS,
            fill_darken: if dark { REGULAR_FILL_BLEND } else { 0.0 },
            fill_normal: REGULAR_FILL_NORMAL,
            fill_lighten: if dark { 0.0 } else { REGULAR_FILL_BLEND },
        },
        face: regular_face(dark, s),
        refraction: GlassRefraction {
            amount: -(REFRACTION_PER_POINT_REGULAR * s).min(REFRACTION_AMOUNT_MAX),
            height: (REFRACTION_HEIGHT_PER_POINT_REGULAR * s).min(REFRACTION_HEIGHT_MAX),
            outer_amount: 0.0,
            outer_height: 0.0,
            outer_opacity: 0.0,
        },
        bleed,
        highlight: GlassHighlight {
            amount: REGULAR_RIM,
            angle: HIGHLIGHT_ANGLE,
            spread: 2.0 * std::f32::consts::FRAC_PI_3,
            height: REGULAR_RIM_HEIGHT,
            offset: -0.5,
            color_bias: REGULAR_RIM_BIAS,
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
            ring_blur: REGULAR_RING_BLUR,
            ring_stroke: REGULAR_RING_STROKE,
            ring_offset: REGULAR_RING_OFFSET,
            ring_opacity: REGULAR_RING_OPACITY,
            outline_opacity: 0.0,
            outline_width: 0.0,
            edge_line: if dark {
                REGULAR_EDGE_LINE_DARK
            } else {
                REGULAR_EDGE_LINE_LIGHT
            },
            edge_line_sides: 0.0,
            edge_line_even: false,
        },
        capture_scale: GLASS_BACKDROP_SCALE,
        veil: [0.0; 2],
        specular: regular_specular(dark),
        border: GlassBorder::NONE,
        tint: [0.0; 4],
    }
}

// ---- Increase Contrast (recorded) --------------------------------------------------------------------------------

/// The regular face with Increase Contrast: black lift, saturation and the normal fill's opacity (white light, black
/// dark). The dark black lift is the recipe's floor for the dark face under the setting (inferred).
const CONTRAST_BLACK_LIGHT: f32 = 0.5;
const CONTRAST_BLACK_DARK: f32 = 0.2;
const CONTRAST_SATURATION: f32 = 0.8;
const CONTRAST_FILL: f32 = 0.45;

/// `face` with the regular glass's Increase Contrast stage.
pub(super) fn contrast_face(face: &mut GlassFace, dark: bool) {
    face.saturation = CONTRAST_SATURATION;
    if dark {
        face.black = CONTRAST_BLACK_DARK;
        face.fill = [0.0, 0.0, 0.0, CONTRAST_FILL];
    } else {
        face.black = CONTRAST_BLACK_LIGHT;
        face.fill = [1.0, 1.0, 1.0, CONTRAST_FILL];
    }
}

// ---- the variants' stages ----------------------------------------------------------------------------------------

/// The light sidebar face's luma cap (recorded; the dark one keeps the regular map).
pub const SIDEBAR_MAX_LUMA_LIGHT: f32 = 0.85;
/// The planar stage's edge bleed saturation (recorded).
pub const PLANAR_BLEED_SATURATION: f32 = 1.5;
/// The tab's normal fill: white at this opacity in both appearances; and how much the light tab lowers the face's
/// black and white (recorded).
pub const TAB_FILL: f32 = 0.15;
pub const TAB_LIGHT_REDUCTION: f32 = 0.05;
/// The cartouche's fixed inner bend: its strength (inwards) and band in points; and its white normal fill in the light
/// appearance (recorded).
pub const CARTOUCHE_REFRACTION: f32 = -5.0;
pub const CARTOUCHE_REFRACTION_HEIGHT: f32 = 5.0;
pub const CARTOUCHE_FILL_LIGHT: f32 = 0.1;
/// A toolbar item's first blur level (a fifth of the radius at every size), its fixed bend in points and its control
/// border (recorded).
pub const TOOLBAR_LEVEL_OPACITY: f32 = 0.2;
pub const TOOLBAR_REFRACTION: f32 = -18.0;
pub const TOOLBAR_REFRACTION_HEIGHT: f32 = 9.0;
pub const TOOLBAR_BORDER: GlassBorder = GlassBorder {
    opacity: 0.125,
    width: 1.0,
};
/// The scale a menu's backdrop is captured at (fitted to captures).
const MENU_CAPTURE_SCALE: f32 = 0.25;

/// The variant's own stages over the regular base `p` resolved at `s`, in the dark appearance or the light one, with
/// Increase Contrast (`contrast`) applied to the base face already.
pub(super) fn regular_variant(
    p: &mut GlassParams,
    variant: GlassVariant,
    modifiers: GlassModifiers,
    dark: bool,
    contrast: bool,
) {
    match variant {
        GlassVariant::Sidebar => {
            if !dark {
                p.face.max_luma = SIDEBAR_MAX_LUMA_LIGHT;
            }
        }
        GlassVariant::AbuttedSidebar | GlassVariant::Inspector => planar(p, dark),
        GlassVariant::Tab => {
            if !dark {
                p.face.black -= TAB_LIGHT_REDUCTION;
                p.face.white -= TAB_LIGHT_REDUCTION;
            }
            p.face.fill = [1.0, 1.0, 1.0, TAB_FILL];
        }
        GlassVariant::CartouchePopover => {
            p.refraction.amount = CARTOUCHE_REFRACTION;
            p.refraction.height = CARTOUCHE_REFRACTION_HEIGHT;
            if !dark && !contrast {
                p.face.fill = [1.0, 1.0, 1.0, CARTOUCHE_FILL_LIGHT];
            }
            p.shadow.edge_line = fitted::cartouche_edge_line(modifiers.size, dark);
        }
        GlassVariant::ToolbarItem => {
            p.blur.level_opacity[0] = TOOLBAR_LEVEL_OPACITY;
            p.blur.level_opacity[1] = TOOLBAR_LEVEL_OPACITY;
            p.refraction.amount = TOOLBAR_REFRACTION;
            p.refraction.height = TOOLBAR_REFRACTION_HEIGHT;
            // with Increase Contrast the toolkit draws the stronger control border itself
            p.border = if contrast {
                GlassBorder::NONE
            } else {
                TOOLBAR_BORDER
            };
            // the control border marks the edge: no line outside it (fitted to captures)
            p.shadow.edge_line = 0.0;
        }
        GlassVariant::Menu => {
            p.capture_scale = MENU_CAPTURE_SCALE;
            // the popup's own outline marks the edge (fitted to captures)
            p.shadow.edge_line = 0.0;
        }
        GlassVariant::NotificationCenter => fitted::notification_center(p, dark),
        GlassVariant::SearchResults => fitted::search_results(p, dark, contrast),
        GlassVariant::EntryField => {
            p.blur = GlassBlur {
                radius: 0.0,
                level_opacity: [0.0; BLUR_LEVELS],
                fill_radius: 0.0,
                fill_darken: 0.0,
                fill_normal: 0.0,
                fill_lighten: 0.0,
            };
        }
        _ => {}
    }
}

/// The planar stage of the window sidebars: no key or fill light and no rim, no ring shadow and no bend outside the
/// edge, the edge bleed more saturated and the light face capped at [`SIDEBAR_MAX_LUMA_LIGHT`].
fn planar(p: &mut GlassParams, dark: bool) {
    p.specular = GlassSpecular::NONE;
    p.highlight.amount = 0.0;
    p.shadow.ring_opacity = 0.0;
    p.refraction.outer_opacity = 0.0;
    p.bleed.saturation = PLANAR_BLEED_SATURATION;
    if !dark {
        p.face.max_luma = SIDEBAR_MAX_LUMA_LIGHT;
    }
    // the sidebar meets the window's content at its edge, which a line would mark (fitted to captures)
    p.shadow.edge_line = 0.0;
}

// ---- the inactive branch -------------------------------------------------------------------------------------------

/// The inactive frost: the blur radius in points (the filter's half of the recorded 1), at full opacity.
pub const INACTIVE_BLUR: f32 = 0.5;
/// The share of the key and fill light an inactive window keeps: its face values are not recorded, so the regular face
/// with the light at half strength (inferred).
pub const INACTIVE_HIGHLIGHT_SHARE: f32 = 0.5;
/// An inactive tab's white normal fill, light and dark (recorded; which branch each belongs to is inferred).
pub const TAB_INACTIVE_FILL_LIGHT: f32 = 0.30;
pub const TAB_INACTIVE_FILL_DARK: f32 = 0.07;

/// The inactive branch over `p`: a frost of a point, no shadow, no bend outside the edge, no edge bleed and the light at
/// [`INACTIVE_HIGHLIGHT_SHARE`].
pub(super) fn inactive(p: &mut GlassParams, variant: GlassVariant, dark: bool) {
    p.blur.radius = INACTIVE_BLUR;
    p.blur.level_opacity = [1.0; BLUR_LEVELS];
    p.shadow.opacity = 0.0;
    p.shadow.ring_opacity = 0.0;
    p.refraction.outer_opacity = 0.0;
    p.bleed.opacity = 0.0;
    p.specular.amount *= INACTIVE_HIGHLIGHT_SHARE;
    if variant == GlassVariant::Tab {
        let a = if dark {
            TAB_INACTIVE_FILL_DARK
        } else {
            TAB_INACTIVE_FILL_LIGHT
        };
        p.face.fill = [1.0, 1.0, 1.0, a];
    }
}

/// The window shadow of a glass window or a menu, shown on `p` unless its surface draws its own.
pub(super) fn window_shadow(p: &mut GlassParams, modifiers: GlassModifiers, dark: bool) {
    if !modifiers.exclude_shadow {
        p.shadow = p.shadow.alert(modifiers.active, dark);
    }
}
