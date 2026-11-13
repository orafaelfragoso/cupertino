//! The control sizes and a few shape/shadow tokens. Window corner radii and the shadow of a regular window are as
//! designed. Each control's geometry at each size is in [`crate::control_sizes`].

use crate::window_shadow::{
    window_shadow, ShadowOption, WindowCornerSize, WINDOW_CORNER_RADIUS_REGULAR,
    WINDOW_CORNER_RADIUS_SMALL,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlSize {
    Regular,
    Small,
    Mini,
    Large,
    /// The largest size, for prominent actions.
    ExtraLarge,
}

impl ControlSize {
    pub const ALL: [ControlSize; 5] = [
        ControlSize::Regular,
        ControlSize::Small,
        ControlSize::Mini,
        ControlSize::Large,
        ControlSize::ExtraLarge,
    ];

    /// From the smallest to the largest.
    pub const ASCENDING: [ControlSize; 5] = [
        ControlSize::Mini,
        ControlSize::Small,
        ControlSize::Regular,
        ControlSize::Large,
        ControlSize::ExtraLarge,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeTokens {
    /// regular window corner radius 12 pt, supersedes the earlier
    /// 8–9 pt estimate.
    pub window_corner_radius: f64,
    /// small (compact) window corner radius 10 pt.
    pub window_corner_radius_small: f64,
    /// Corner radius of the highlight behind a pressed menu bar title: a capsule in the bar's height.
    pub menu_bar_selection_corner_radius: f64,
    /// window rim is one pixel wide (window rim colour).
    pub window_rim_width_px: f64,
    /// Height of the menu bar: 33 pt, its 22 pt items centred with 5.5 pt above and below.
    pub menu_bar_height: f64,
}

impl Default for ShapeTokens {
    fn default() -> Self {
        Self {
            window_corner_radius: WINDOW_CORNER_RADIUS_REGULAR,
            window_corner_radius_small: WINDOW_CORNER_RADIUS_SMALL,
            menu_bar_selection_corner_radius: 11.0,
            window_rim_width_px: 1.0,
            menu_bar_height: 33.0,
        }
    }
}

impl ShapeTokens {
    pub fn window_corner_radius_for(&self, size: WindowCornerSize) -> f64 {
        match size {
            WindowCornerSize::Regular => self.window_corner_radius,
            WindowCornerSize::Small => self.window_corner_radius_small,
        }
    }
}

/// Window shadow of a regular window, taken from the verified table: active
/// σ 20 pt, offset 18 pt, density 0.40; inactive σ 12 pt, offset 8 pt, density 0.25. The tight
/// contact shadow is approximate (the table has no such term). The complete table is in
/// [`crate::window_shadow`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowShadowPreset {
    /// Active window sigma.
    pub sigma: f64,
    /// Active window density.
    pub alpha: f64,
    /// Active window vertical offset.
    pub offset_y: f64,
    /// approximate.
    pub contact_sigma: f64,
    /// Inactive density divided by active density (0.25 / 0.40).
    pub inactive_scale: f64,
    /// Inactive window sigma.
    pub inactive_sigma: f64,
    /// Inactive window vertical offset.
    pub inactive_offset_y: f64,
}

impl Default for WindowShadowPreset {
    fn default() -> Self {
        let a = window_shadow(ShadowOption::Standard, true, false, false);
        let i = window_shadow(ShadowOption::Standard, false, false, false);
        Self {
            sigma: a.sigma,
            alpha: a.density,
            offset_y: a.vertical_offset,
            contact_sigma: 1.0,
            inactive_scale: i.density / a.density,
            inactive_sigma: i.sigma,
            inactive_offset_y: i.vertical_offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_and_shadow_tokens_are_verified_values() {
        let s = ShapeTokens::default();
        assert_eq!(s.window_corner_radius, 16.0);
        assert_eq!(s.window_corner_radius_for(WindowCornerSize::Small), 10.0);
        let w = WindowShadowPreset::default();
        assert_eq!((w.sigma, w.alpha, w.offset_y), (20.0, 0.40, 18.0));
        assert_eq!((w.inactive_sigma, w.inactive_offset_y), (12.0, 8.0));
        assert!((w.inactive_scale - 0.625).abs() < 1e-12);
    }
}
