//! Material and shadow parameters of the notification platter (tables), as data for the renderer.
//!
//! The backdrop recipe is: capture at 1/8 scale, saturate by `S[m], Gaussian blur radius 30
//! (edges normalised and hard), brightness `B[m]`. [`PlatterMaterial::backdrop`] expresses it in
//! the theme's recipe form, which has no brightness operation, so `B[m] is data only. The banner
//! platter uses `.medium` (role).
//!
//! The shadow layer is wired into the renderer through [`PlatterShadow::params`]; the black rim
//! layer and the 1 pt white inner rim have no counterpart in the renderer and are data only.

use cupertino_decorations::shadow::ShadowParams;
use cupertino_theme::recipe::{Backdrop, ColorOp};
use cupertino_theme::window_shadow::{window_shadow, ShadowOption};

/// Backdrop capture scale: 0.125.
pub const BACKDROP_SCALE: f32 = 0.125;
/// Gaussian blur: 30 (whether it applies in 1/8-scale space
/// needs runtime verification).
pub const BLUR_RADIUS: f32 = 30.0;
/// Platter outline stroke width: 0.5 pt.
pub const OUTLINE_WIDTH: f64 = 0.5;
/// Inner rim border width: 1.0 pt, white.
pub const INNER_RIM_WIDTH: f64 = 1.0;
/// The rim layer's corner radius exceeds the card's by this much: r + 1 pt (role, data only).
pub const RIM_RADIUS_OUTSET: f64 = 1.0;

/// Material thickness (light and dark rows of the design table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatterMaterial {
    UltraThin,
    Thin,
    Medium,
    Thick,
    UltraThick,
}

impl PlatterMaterial {
    pub const ALL: [PlatterMaterial; 5] = [Self::UltraThin, Self::Thin, Self::Medium, Self::Thick, Self::UltraThick];

    const SATURATION: [[f32; 5]; 2] = [[1.8, 1.9, 2.0, 2.1, 2.2], [1.6, 1.8, 2.0, 2.2, 2.4]];
    const BRIGHTNESS: [[f32; 5]; 2] = [[-0.01, -0.02, -0.03, -0.04, -0.05], [0.015, 0.02, 0.025, 0.03, 0.035]];

    /// amount (light/dark row choice ).
    pub fn saturation(self, dark: bool) -> f32 {
        Self::SATURATION[usize::from(dark)][self as usize]
    }

    /// amount (light/dark row choice ).
    pub fn brightness(self, dark: bool) -> f32 {
        Self::BRIGHTNESS[usize::from(dark)][self as usize]
    }

    /// The backdrop in the theme's recipe form: scale, blur and saturation (brightness has no
    /// recipe operation and is left out).
    pub fn backdrop(self, dark: bool) -> Backdrop {
        Backdrop { scale: BACKDROP_SCALE, sigma_pt: BLUR_RADIUS, ops: vec![ColorOp::Saturate(self.saturation(dark))] }
    }
}

/// The platter's drop shadow layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlatterShadow {
    /// 0.20 light, 0.25 dark (which value is dark is ).
    pub opacity: f64,
    /// Layer shadow radius: 4.0 pt, offset (0, 0).
    pub radius: f64,
}

impl PlatterShadow {
    pub fn new(dark: bool) -> Self {
        Self { opacity: if dark { 0.25 } else { 0.20 }, radius: 4.0 }
    }

    /// Parameters for the renderer's shadow rasteriser: density = opacity, no offset, no top
    /// clip, Gaussian σ = radius / 2 (the radius-to-σ conversion is approximate).
    pub fn params(&self) -> ShadowParams {
        let mut entry = *window_shadow(ShadowOption::Panel, true, false, false);
        entry.density = self.opacity;
        entry.sigma = self.radius / 2.0;
        entry.vertical_offset = 0.0;
        entry.clip_top = 0.0;
        ShadowParams::new(entry)
    }
}

/// The black rim layer of the vibrant style (data only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VibrantRim {
    /// 1.0 dark, 0.35 light (which value is dark is ).
    pub opacity: f64,
    /// 0.5 dark, 0.625 light.
    pub radius: f64,
}

impl VibrantRim {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self { opacity: 1.0, radius: 0.5 }
        } else {
            Self { opacity: 0.35, radius: 0.625 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_tables_follow_the_reference_rows() {
        let light: Vec<(f32, f32)> = PlatterMaterial::ALL.iter().map(|m| (m.saturation(false), m.brightness(false))).collect();
        assert_eq!(light, [(1.8, -0.01), (1.9, -0.02), (2.0, -0.03), (2.1, -0.04), (2.2, -0.05)]);
        let dark: Vec<(f32, f32)> = PlatterMaterial::ALL.iter().map(|m| (m.saturation(true), m.brightness(true))).collect();
        assert_eq!(dark, [(1.6, 0.015), (1.8, 0.02), (2.0, 0.025), (2.2, 0.03), (2.4, 0.035)]);
        assert_eq!(PlatterMaterial::Thick.backdrop(true), Backdrop { scale: 0.125, sigma_pt: 30.0, ops: vec![ColorOp::Saturate(2.2)] });
    }

    #[test]
    fn shadow_has_no_offset_and_appearance_dependent_opacity() {
        let light = PlatterShadow::new(false).params();
        assert_eq!((light.density(), light.sigma(), light.vertical_offset(), light.entry.clip_top), (0.20, 2.0, 0.0, 0.0));
        assert_eq!(PlatterShadow::new(true).params().density(), 0.25);
        assert_eq!(VibrantRim::new(false), VibrantRim { opacity: 0.35, radius: 0.625 });
    }
}
