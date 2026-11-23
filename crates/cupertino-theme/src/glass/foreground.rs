//! The colour of text and symbols on glass: the foreground matrices, the face tone that picks one, and the opacity
//! labels are drawn at.

use super::*;

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
