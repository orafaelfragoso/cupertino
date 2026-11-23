//! The stages the display options and the legibility amount lay over a recipe: the regular glass's Reduce
//! Transparency, Reduce Motion, and the amount of fill a denser material asks for.

use super::*;

// ---- Reduce Transparency on the regular base (recorded) -------------------------------------------------------------

/// Blur radius of regular glass with Reduce Transparency, in points.
pub const REDUCED_TRANSPARENCY_BLUR: f32 = 40.625;
/// Level opacities with Reduce Transparency: the two narrow levels win, the first at full opacity.
const REDUCED_TRANSPARENCY_LEVELS: [f32; BLUR_LEVELS] = [1.0, 1.0, 0.123_076_92, 0.123_076_92];
/// The light face with Reduce Transparency: its black lift (with Increase Contrast too, the contrast lift) and
/// saturation.
const REDUCED_BLACK_LIGHT: f32 = 0.9;
const REDUCED_CONTRAST_BLACK_LIGHT: f32 = 0.5;
const REDUCED_SATURATION_LIGHT: f32 = 0.9;
/// The dark face with Reduce Transparency: its saturation (with Increase Contrast too, the contrast saturation), and
/// the levels that map it into a dark surface with no luma cap: black lift and white gain (inferred from the system's
/// dark chrome with the setting, as the glass's own are not recorded).
const REDUCED_SATURATION_DARK: f32 = 0.7;
const REDUCED_CONTRAST_SATURATION_DARK: f32 = 0.8;
const REDUCED_BLACK_DARK: f32 = 0.225;
const REDUCED_WHITE_DARK: f32 = 0.31;

/// The regular glass's Reduce Transparency stage over `p`: a wide blur at full opacity, nothing bent at the edge or
/// over it, no ring and no edge bleed, and a denser face (`contrast`: with Increase Contrast too).
pub(super) fn regular_reduced(p: &mut GlassParams, dark: bool, contrast: bool) {
    p.blur.radius = REDUCED_TRANSPARENCY_BLUR;
    p.blur.level_opacity = REDUCED_TRANSPARENCY_LEVELS;
    flatten(p);
    p.bleed.opacity = 0.0;
    let f = &mut p.face;
    if dark {
        f.saturation = if contrast {
            REDUCED_CONTRAST_SATURATION_DARK
        } else {
            REDUCED_SATURATION_DARK
        };
        f.black = REDUCED_BLACK_DARK;
        f.white = REDUCED_WHITE_DARK;
        f.max_luma = 1.0;
    } else {
        f.saturation = REDUCED_SATURATION_LIGHT;
        f.black = if contrast {
            REDUCED_CONTRAST_BLACK_LIGHT
        } else {
            REDUCED_BLACK_LIGHT
        };
    }
}

/// Nothing bent at the edge or outside it, and no ring shadow.
pub(super) fn flatten(p: &mut GlassParams) {
    p.refraction.amount = 0.0;
    p.refraction.height = 0.0;
    p.refraction.outer_opacity = 0.0;
    p.shadow.ring_blur = 0.0;
    p.shadow.ring_stroke = 0.0;
    p.shadow.ring_offset = 0.0;
    p.shadow.ring_opacity = 0.0;
}

// ---- Reduce Motion (recorded) ---------------------------------------------------------------------------------------

/// Smallest blur radius with Reduce Motion.
const REDUCED_MOTION_BLUR: f32 = 8.0;
/// Level opacities with Reduce Motion.
const REDUCED_MOTION_LEVELS: [f32; BLUR_LEVELS] = [1.0, 1.0, 0.625, 0.625];
/// How much wider the control centre's and the notification centre's blur is with Reduce Motion (their shared stage).
pub const REDUCED_MOTION_BLUR_SCALE: f32 = 4.0;

/// The Reduce Motion stage over `p`: no inner bend, and a blur at least [`REDUCED_MOTION_BLUR`] wide over the Reduce
/// Motion levels, [`REDUCED_MOTION_BLUR_SCALE`] times as wide first for the variants whose recipes take the shared
/// stage.
pub(super) fn reduced_motion(p: &mut GlassParams, variant: GlassVariant) {
    p.refraction.height = 0.0;
    if matches!(
        variant,
        GlassVariant::ControlCenter | GlassVariant::NotificationCenter
    ) {
        p.blur.radius *= REDUCED_MOTION_BLUR_SCALE;
    }
    p.blur.radius = p.blur.radius.max(REDUCED_MOTION_BLUR);
    p.blur.level_opacity = REDUCED_MOTION_LEVELS;
}

// ---- the legibility amount (recorded) -------------------------------------------------------------------------------

/// The legibility amount of the system at which nothing changes.
pub const LEGIBILITY_NEUTRAL: f32 = 0.5;
/// The opacity the regular glass's normal fill and its lighten or darken fill rise to at the largest amount.
const LEGIBILITY_NORMAL_MAX: f32 = 1.0;
const LEGIBILITY_BLEND_MAX: f32 = 0.9;
/// The radius in points of the clear glass's blur fill once the amount is past neutral (the filter's half of the
/// recipe's 16).
const LEGIBILITY_CLEAR_FILL_RADIUS: f32 = 8.0;

/// The legibility amount of a density of `density` percent: from neutral at 0 to 1 at 100 and beyond.
pub fn legibility_amount(density: u8) -> f32 {
    LEGIBILITY_NEUTRAL + f32::from(density.min(100)) / 200.0
}

/// The legibility stage over `p`, of `base`, at amount `a`: past neutral, the regular glass's normal fill rises
/// linearly towards 1 and its lighten or darken fill towards 0.9; the clear glass takes a blur fill of its own, in
/// normal blending at `2(a − 0.5)`. Nothing changes at neutral or below.
pub(super) fn legibility(p: &mut GlassParams, base: GlassBase, dark: bool, a: f32) {
    let k = ((a - LEGIBILITY_NEUTRAL) / (1.0 - LEGIBILITY_NEUTRAL)).clamp(0.0, 1.0);
    if k <= 0.0 {
        return;
    }
    let b = &mut p.blur;
    match base {
        GlassBase::Regular => {
            b.fill_normal += (LEGIBILITY_NORMAL_MAX - b.fill_normal) * k;
            let blend = if dark {
                &mut b.fill_darken
            } else {
                &mut b.fill_lighten
            };
            *blend += (LEGIBILITY_BLEND_MAX - *blend) * k;
        }
        GlassBase::Clear => {
            b.fill_radius = LEGIBILITY_CLEAR_FILL_RADIUS;
            b.fill_lighten = 0.0;
            b.fill_darken = 0.0;
            b.fill_normal = 2.0 * (a - LEGIBILITY_NEUTRAL);
        }
    }
}
