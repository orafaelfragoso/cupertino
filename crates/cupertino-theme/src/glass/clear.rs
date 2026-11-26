//! The clear glass and the variants built on it: the Dock, the application switcher and the controls' lenses (the
//! control centre's modules are in `fitted`).

use super::regular::{regular_specular, FACE_CLAMP};
use super::*;

// ---- the clear base --------------------------------------------------------------------------------------------

/// The clear blur radius in points at full opacity: the resolved filter's for a recipe radius of 1, so a
/// clear-family recipe radius `r` is `r` times this.
pub const CLEAR_BLUR: f32 = 0.690_272_15;
/// The clear face, the same in both appearances: black lift, white gain and saturation; no luma cap and no fill.
const CLEAR_BLACK: f32 = 0.125;
const CLEAR_WHITE: f32 = 1.08;
const CLEAR_SATURATION: f32 = 1.06;
/// The clear inner refraction per point of short side (negative: inwards) and its band per point, each at most the cap
/// below: −13 at 20 pt, −26 at 40, −39 over a 20 pt band at 60.
const REFRACTION_PER_POINT_CLEAR: f32 = 0.65;
const REFRACTION_HEIGHT_PER_POINT_CLEAR: f32 = 0.36;
const REFRACTION_AMOUNT_MAX: f32 = 60.0;
const REFRACTION_HEIGHT_MAX: f32 = 20.0;
/// The clear rim: strength, colour shift per unit of it and depth in points.
const CLEAR_RIM: f32 = 0.4;
const CLEAR_RIM_BIAS: f32 = -0.25;
const CLEAR_RIM_HEIGHT: f32 = 0.5;
/// The clear drop shadow (shown at no opacity at rest): strength per point of short side and the side it stops growing
/// at, and blur per point of short side and its side.
const CLEAR_SHADOW_AMOUNT_PER_POINT: f32 = 0.625;
const CLEAR_SHADOW_AMOUNT_SIDE_MAX: f32 = 120.0;
const CLEAR_SHADOW_HEIGHT_PER_POINT: f32 = 0.4;
const CLEAR_SHADOW_HEIGHT_SIDE_MAX: f32 = 240.0;
/// How strongly the soft band lights the far end of its axis on clear glass, as a share of the near end.
const GLOW_FILL: f32 = 0.75;
/// The colour of the soft band's light on clear glass: brighter, keeping the hue.
const HIGHLIGHT_MATRIX_CLEAR: ColorMatrix = [
    1.3192, -0.0478, -0.0047, 0.0, 0.2, //
    -0.0142, 1.2856, -0.0048, 0.0, 0.2, //
    -0.0142, -0.0476, 1.3285, 0.0, 0.2, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// The clear glass resolved at short side `s`, the same in both appearances.
pub(super) fn clear_base(s: f32) -> GlassParams {
    GlassParams {
        blur: GlassBlur {
            radius: CLEAR_BLUR,
            level_opacity: [1.0; BLUR_LEVELS],
            fill_radius: 0.0,
            fill_darken: 0.0,
            fill_normal: 0.0,
            fill_lighten: 0.0,
        },
        face: GlassFace {
            saturation: CLEAR_SATURATION,
            white: CLEAR_WHITE,
            black: CLEAR_BLACK,
            clamp: FACE_CLAMP,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.0],
        },
        refraction: GlassRefraction {
            amount: -(REFRACTION_PER_POINT_CLEAR * s).min(REFRACTION_AMOUNT_MAX),
            height: (REFRACTION_HEIGHT_PER_POINT_CLEAR * s).min(REFRACTION_HEIGHT_MAX),
            outer_amount: 0.0,
            outer_height: 0.0,
            outer_opacity: 0.0,
        },
        bleed: GlassBleed {
            amount: 0.0,
            height: 0.0,
            blur: 0.0,
            white: 1.0,
            black: 0.75,
            saturation: 1.2,
            darken: true,
            opacity: 0.0,
        },
        highlight: GlassHighlight {
            amount: CLEAR_RIM,
            angle: HIGHLIGHT_ANGLE,
            spread: 5.0 * std::f32::consts::PI / 12.0,
            height: CLEAR_RIM_HEIGHT,
            offset: -0.5,
            outside: false,
            color_bias: CLEAR_RIM_BIAS,
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
            edge_line: 0.0,
            edge_line_sides: 0.0,
            edge_line_even: false,
        },
        capture_scale: GLASS_BACKDROP_SCALE,
        veil: [0.0; 2],
        specular: GlassSpecular::NONE,
        border: GlassBorder::NONE,
        tint: [0.0; 4],
    }
}

/// Clear glass with Reduce Transparency: the blur radius in points at full opacity, the light face's black lift and
/// the dark face's saturation.
pub const CLEAR_REDUCED_BLUR: f32 = 200.0;
const CLEAR_REDUCED_BLACK_LIGHT: f32 = 0.125;
const CLEAR_REDUCED_SATURATION_DARK: f32 = 0.7;

/// The clear glass's Reduce Transparency stage over `p`: a wide blur and nothing bent at the edge.
pub(super) fn clear_reduced(p: &mut GlassParams, dark: bool) {
    p.blur.radius = CLEAR_REDUCED_BLUR;
    p.blur.level_opacity = [1.0; BLUR_LEVELS];
    p.refraction.amount = 0.0;
    p.refraction.height = 0.0;
    if dark {
        p.face.saturation = CLEAR_REDUCED_SATURATION_DARK;
    } else {
        p.face.black = CLEAR_REDUCED_BLACK_LIGHT;
    }
}

// ---- the Dock and the application switcher -----------------------------------------------------------------------

/// The Dock's blur, in multiples of the clear blur ([`CLEAR_BLUR`]): a Gaussian of 2 points; the application
/// switcher's in the light and the dark appearance.
pub const DOCK_BLUR_SCALE: f32 = 1.45;
pub const SWITCHER_BLUR_SCALE_LIGHT: f32 = 2.0;
pub const SWITCHER_BLUR_SCALE_DARK: f32 = 3.5;
/// The Dock's face: saturation in both appearances, black lift and white gain light and dark, and the dark face's
/// black fill.
pub const DOCK_SATURATION: f32 = 1.25;
pub const DOCK_BLACK_LIGHT: f32 = 0.15;
pub const DOCK_WHITE_LIGHT: f32 = 0.9;
pub const DOCK_BLACK_DARK: f32 = 0.05;
pub const DOCK_WHITE_DARK: f32 = 0.92;
pub const DOCK_FILL_DARK: f32 = 0.02;
/// The Dock's inner refraction and band as fractions of a dimension (negative: inwards): the application switcher's
/// short side, the bar's [`DOCK_BAR_BEND_DIMENSION`].
pub const DOCK_REFRACTION: f32 = -0.18;
pub const DOCK_REFRACTION_HEIGHT: f32 = 0.08;
/// The application switcher's bend outside its edge: strength and band as fractions of its short side, and its
/// opacity (the bar bends nothing outside its edge).
pub const DOCK_OUTER_REFRACTION: f32 = 0.42;
pub const DOCK_OUTER_REFRACTION_HEIGHT: f32 = 0.13;
pub const DOCK_OUTER_OPACITY: f32 = 0.2;
/// The Dock's face with Increase Contrast: black lift, white gain and fill (white light, black dark).
pub const DOCK_CONTRAST_BLACK_LIGHT: f32 = 0.125;
pub const DOCK_CONTRAST_WHITE_LIGHT: f32 = 0.7;
pub const DOCK_CONTRAST_FILL_LIGHT: f32 = 0.12;
pub const DOCK_CONTRAST_BLACK_DARK: f32 = 0.1;
pub const DOCK_CONTRAST_WHITE_DARK: f32 = 0.65;
pub const DOCK_CONTRAST_FILL_DARK: f32 = 0.15;
/// The key and fill light the Dock shows only with Increase Contrast: their opacity, scaling the regular light's
/// strength.
pub const DOCK_CONTRAST_HIGHLIGHT: f32 = 0.75;
/// The light Dock's face with Reduce Transparency: black lift, white gain and white fill; the dark Dock keeps its own.
pub const DOCK_REDUCED_BLACK_LIGHT: f32 = 0.4;
pub const DOCK_REDUCED_WHITE_LIGHT: f32 = 0.92;
pub const DOCK_REDUCED_FILL_LIGHT: f32 = 0.1;
/// The application switcher's edge line in the dark appearance; the light one keeps regular glass's.
const SWITCHER_EDGE_LINE_DARK: f32 = 0.32;
const SWITCHER_EDGE_LINE_LIGHT: f32 = 0.2;

/// The Dock's glass, or the application switcher's (`switcher`), at short side `s` with its own stages of Increase
/// Contrast and Reduce Transparency: both keep the bend and the blur.
pub(super) fn dock(
    dark: bool,
    s: f32,
    switcher: bool,
    contrast: bool,
    reduced: bool,
) -> GlassParams {
    let mut p = clear_base(s);
    p.blur.radius = CLEAR_BLUR
        * match (switcher, dark) {
            (false, _) => DOCK_BLUR_SCALE,
            (true, false) => SWITCHER_BLUR_SCALE_LIGHT,
            (true, true) => SWITCHER_BLUR_SCALE_DARK,
        };
    p.face = GlassFace {
        saturation: DOCK_SATURATION,
        white: if dark {
            DOCK_WHITE_DARK
        } else {
            DOCK_WHITE_LIGHT
        },
        black: if dark {
            DOCK_BLACK_DARK
        } else {
            DOCK_BLACK_LIGHT
        },
        clamp: FACE_CLAMP,
        max_luma: 1.0,
        fill: if dark {
            [0.0, 0.0, 0.0, DOCK_FILL_DARK]
        } else {
            [1.0, 1.0, 1.0, 0.0]
        },
    };
    p.refraction = GlassRefraction {
        amount: DOCK_REFRACTION * s,
        height: DOCK_REFRACTION_HEIGHT * s,
        outer_amount: DOCK_OUTER_REFRACTION * s,
        outer_height: DOCK_OUTER_REFRACTION_HEIGHT * s,
        outer_opacity: DOCK_OUTER_OPACITY,
    };
    if !switcher {
        super::fitted::dock_bar(&mut p, dark);
    }
    if contrast {
        let f = &mut p.face;
        if dark {
            (f.black, f.white) = (DOCK_CONTRAST_BLACK_DARK, DOCK_CONTRAST_WHITE_DARK);
            f.fill = [0.0, 0.0, 0.0, DOCK_CONTRAST_FILL_DARK];
        } else {
            (f.black, f.white) = (DOCK_CONTRAST_BLACK_LIGHT, DOCK_CONTRAST_WHITE_LIGHT);
            f.fill = [1.0, 1.0, 1.0, DOCK_CONTRAST_FILL_LIGHT];
        }
        p.specular = regular_specular(dark);
        p.specular.amount *= DOCK_CONTRAST_HIGHLIGHT;
    }
    if reduced && !dark {
        let f = &mut p.face;
        (f.black, f.white) = (DOCK_REDUCED_BLACK_LIGHT, DOCK_REDUCED_WHITE_LIGHT);
        f.fill = [1.0, 1.0, 1.0, DOCK_REDUCED_FILL_LIGHT];
    }
    if switcher {
        p.shadow.edge_line = if dark {
            SWITCHER_EDGE_LINE_DARK
        } else {
            SWITCHER_EDGE_LINE_LIGHT
        };
    }
    p
}

// ---- the controls' lenses ----------------------------------------------------------------------------------------

/// The control face: black lift and white gain light and dark, no colour change, no luma cap and no fill.
pub const CONTROL_BLACK_LIGHT: f32 = 0.5;
pub const CONTROL_WHITE_LIGHT: f32 = 1.03;
pub const CONTROL_BLACK_DARK: f32 = 0.2;
pub const CONTROL_WHITE_DARK: f32 = 0.6;
/// The control's ring shadow: Gaussian deviation, stroke and downward offset in points, and opacity.
pub const CONTROL_RING_BLUR: f32 = 3.0;
pub const CONTROL_RING_STROKE: f32 = 4.0;
pub const CONTROL_RING_OFFSET: f32 = 8.0;
pub const CONTROL_RING_OPACITY: f32 = 0.1;
/// The control's inner refraction and band as fractions of its short side (negative: inwards).
pub const CONTROL_REFRACTION: f32 = -0.15;
pub const CONTROL_REFRACTION_HEIGHT: f32 = 0.1;
/// The slider's ring opacity light and dark, and its refraction and band as fractions of its short side.
pub const SLIDER_RING_LIGHT: f32 = 0.15;
pub const SLIDER_RING_DARK: f32 = 0.4;
pub const SLIDER_REFRACTION: f32 = -0.27;
pub const SLIDER_REFRACTION_HEIGHT: f32 = 0.18;
/// The loupe's ring: downward offset and deviation in points and opacity; its refraction and band as fractions.
pub const LOUPE_RING_OFFSET: f32 = 12.0;
pub const LOUPE_RING_BLUR: f32 = 4.0;
pub const LOUPE_RING_OPACITY: f32 = 0.07;
pub const LOUPE_REFRACTION: f32 = -0.3;
pub const LOUPE_REFRACTION_HEIGHT: f32 = 0.095;
/// A slider track's edge line, light and dark.
const SLIDER_EDGE_LINE_LIGHT: f32 = 0.22;
const SLIDER_EDGE_LINE_DARK: f32 = 0.24;

/// The glass of a control's lens (`Control`, `Slider` or `Loupe`) at short side `s`: no blur, read at the output's
/// scale, the control face, its ring and its bend, with no values of its own for Increase Contrast or Reduce
/// Transparency.
pub(super) fn control(variant: GlassVariant, dark: bool, s: f32) -> GlassParams {
    let mut p = clear_base(s);
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
        white: if dark {
            CONTROL_WHITE_DARK
        } else {
            CONTROL_WHITE_LIGHT
        },
        black: if dark {
            CONTROL_BLACK_DARK
        } else {
            CONTROL_BLACK_LIGHT
        },
        clamp: FACE_CLAMP,
        max_luma: 1.0,
        fill: [1.0, 1.0, 1.0, 0.0],
    };
    let sh = &mut p.shadow;
    (
        sh.ring_blur,
        sh.ring_stroke,
        sh.ring_offset,
        sh.ring_opacity,
    ) = (
        CONTROL_RING_BLUR,
        CONTROL_RING_STROKE,
        CONTROL_RING_OFFSET,
        CONTROL_RING_OPACITY,
    );
    let (amount, height) = match variant {
        GlassVariant::Slider => {
            sh.ring_opacity = if dark {
                SLIDER_RING_DARK
            } else {
                SLIDER_RING_LIGHT
            };
            sh.edge_line = if dark {
                SLIDER_EDGE_LINE_DARK
            } else {
                SLIDER_EDGE_LINE_LIGHT
            };
            (SLIDER_REFRACTION, SLIDER_REFRACTION_HEIGHT)
        }
        GlassVariant::Loupe => {
            (sh.ring_offset, sh.ring_blur, sh.ring_opacity) =
                (LOUPE_RING_OFFSET, LOUPE_RING_BLUR, LOUPE_RING_OPACITY);
            (LOUPE_REFRACTION, LOUPE_REFRACTION_HEIGHT)
        }
        _ => (CONTROL_REFRACTION, CONTROL_REFRACTION_HEIGHT),
    };
    p.refraction.amount = amount * s;
    p.refraction.height = height * s;
    p.capture_scale = TRACK_CAPTURE_SCALE;
    p
}
