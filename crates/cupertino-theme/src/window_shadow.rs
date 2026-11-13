//! Window shadow parameter table and window corner radii as typed data.
//!
//! One entry per window class × active/inactive × light/dark × contrast variant. `sigma` is the
//! Gaussian standard deviation in points; the renderer turns it into a pixel radius of
//! `ceil(sigma · 2.8)` and never takes a `sigma` above 128.
//!
//! [`ShadowOption::Standard`] is for regular windows, [`ShadowOption::Panel`] for panels, menus,
//! popovers, combo lists and drawers, [`ShadowOption::Tooltip`] for tooltips and
//! [`ShadowOption::Other`] for sheets and modal documents. The `increased_contrast` half of the
//! table is used while Increase Contrast is on: a uniform 0.7 clip and inner-rim density.

/// Window class selecting a row of the table. Discriminants are the class's bit in the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShadowOption {
    Standard = 1,
    Panel = 2,
    Tooltip = 4,
    Other = 8,
}

impl ShadowOption {
    pub const ALL: [ShadowOption; 4] = [
        ShadowOption::Standard,
        ShadowOption::Panel,
        ShadowOption::Tooltip,
        ShadowOption::Other,
    ];

    pub fn bits(self) -> u8 {
        self as u8
    }
}

/// One row of the shadow table (all lengths in points at 1×).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowShadowEntry {
    pub option: ShadowOption,
    pub active: bool,
    pub dark: bool,
    /// Second half of the table, selected by Increase Contrast.
    pub increased_contrast: bool,
    /// Peak shadow opacity (0…1).
    pub density: f64,
    /// Gaussian standard deviation in points.
    pub sigma: f64,
    /// Distance the shadow is shifted below the window, in points.
    pub vertical_offset: f64,
    /// Flag of the entry: 0 for the light set, 1 for the dark and increased-contrast sets.
    pub appearance_flag: bool,
    /// Fraction of the shadow clipped away at the top edge of the window (0…1).
    pub clip_top: f64,
    pub rim_radius_scale: f64,
    pub rim_style_white: bool,
    pub inner_rim_density: f64,
    pub inner_rim_radius_scale: f64,
    pub inner_rim_style_white: bool,
}

impl WindowShadowEntry {
    /// Pixel radius of the blur kernel: `ceil(sigma · 2.8).
    pub fn radius_px(&self) -> u32 {
        sigma_to_radius_px(self.sigma)
    }

    /// Extent of the shadow above the window top edge: pixel radius minus vertical offset, floored at 0
    /// (the rule the design window server uses for the upward frame bounds).
    pub fn extent_above(&self) -> f64 {
        (self.radius_px() as f64 - self.vertical_offset).max(0.0)
    }

    pub fn extent_below(&self) -> f64 {
        self.radius_px() as f64 + self.vertical_offset
    }

    pub fn extent_side(&self) -> f64 {
        self.radius_px() as f64
    }
}

/// Constant converting a shadow sigma to a pixel radius.
pub const SIGMA_TO_RADIUS_FACTOR: f64 = 2.8;

/// Largest sigma the design window server accepts.
pub const SHADOW_SIGMA_MAXIMUM: f64 = 128.0;

pub fn sigma_to_radius_px(sigma: f64) -> u32 {
    (sigma.clamp(0.0, SHADOW_SIGMA_MAXIMUM) * SIGMA_TO_RADIUS_FACTOR).ceil() as u32
}

/// Rim radius in points: 3 when `backing_scale · scale_field + 1/256 > 2`, else 2.
pub fn rim_radius(backing_scale: f64, scale_field: f64) -> f64 {
    if backing_scale * scale_field + 1.0 / 256.0 > 2.0 {
        3.0
    } else {
        2.0
    }
}

#[allow(clippy::too_many_arguments)]
const fn e(
    option: ShadowOption,
    active: bool,
    dark: bool,
    increased_contrast: bool,
    density: f64,
    sigma: f64,
    vertical_offset: f64,
    appearance_flag: bool,
    clip_top: f64,
    rim_radius_scale: f64,
    rim_style_white: bool,
    inner_rim_density: f64,
    inner_rim_radius_scale: f64,
    inner_rim_style_white: bool,
) -> WindowShadowEntry {
    WindowShadowEntry {
        option,
        active,
        dark,
        increased_contrast,
        density,
        sigma,
        vertical_offset,
        appearance_flag,
        clip_top,
        rim_radius_scale,
        rim_style_white,
        inner_rim_density,
        inner_rim_radius_scale,
        inner_rim_style_white,
    }
}

/// The complete table, 32 rows in table order (contrast variant, active, dark, option 1/2/4/8)..
pub const WINDOW_SHADOW_TABLE: [WindowShadowEntry; 32] = [
    e(
        ShadowOption::Standard,
        true,
        false,
        false,
        0.4,
        20.0,
        18.0,
        false,
        0.6,
        1.0,
        false,
        0.1,
        0.5,
        true,
    ),
    e(
        ShadowOption::Panel,
        true,
        false,
        false,
        0.25,
        8.0,
        4.0,
        false,
        0.55,
        1.0,
        false,
        0.1,
        0.5,
        true,
    ),
    e(
        ShadowOption::Tooltip,
        true,
        false,
        false,
        0.2,
        4.0,
        2.0,
        false,
        0.35,
        1.0,
        false,
        0.1,
        0.5,
        true,
    ),
    e(
        ShadowOption::Other,
        true,
        false,
        false,
        0.25,
        20.0,
        18.0,
        false,
        0.6,
        1.0,
        false,
        0.1,
        0.5,
        true,
    ),
    e(
        ShadowOption::Standard,
        true,
        true,
        false,
        0.7,
        20.0,
        18.0,
        true,
        0.95,
        0.5,
        false,
        0.2,
        1.0,
        true,
    ),
    e(
        ShadowOption::Panel,
        true,
        true,
        false,
        0.3,
        8.0,
        4.0,
        true,
        0.85,
        0.5,
        false,
        0.2,
        1.0,
        true,
    ),
    e(
        ShadowOption::Tooltip,
        true,
        true,
        false,
        0.25,
        4.0,
        2.0,
        true,
        0.45,
        0.5,
        false,
        0.15,
        0.5,
        true,
    ),
    e(
        ShadowOption::Other,
        true,
        true,
        false,
        0.45,
        20.0,
        18.0,
        true,
        0.95,
        0.5,
        false,
        0.2,
        1.0,
        true,
    ),
    e(
        ShadowOption::Standard,
        false,
        false,
        false,
        0.25,
        12.0,
        8.0,
        false,
        0.55,
        1.0,
        false,
        0.1,
        0.5,
        true,
    ),
    e(
        ShadowOption::Panel,
        false,
        false,
        false,
        0.25,
        8.0,
        4.0,
        false,
        0.55,
        1.0,
        false,
        0.1,
        0.5,
        true,
    ),
    e(
        ShadowOption::Tooltip,
        false,
        false,
        false,
        0.2,
        4.0,
        2.0,
        false,
        0.35,
        1.0,
        false,
        0.1,
        0.5,
        true,
    ),
    e(
        ShadowOption::Other,
        false,
        false,
        false,
        0.25,
        12.0,
        8.0,
        false,
        0.55,
        1.0,
        false,
        0.1,
        0.5,
        true,
    ),
    e(
        ShadowOption::Standard,
        false,
        true,
        false,
        0.45,
        12.0,
        8.0,
        true,
        0.85,
        0.5,
        false,
        0.18,
        1.0,
        true,
    ),
    e(
        ShadowOption::Panel,
        false,
        true,
        false,
        0.3,
        8.0,
        4.0,
        true,
        0.85,
        0.5,
        false,
        0.2,
        1.0,
        true,
    ),
    e(
        ShadowOption::Tooltip,
        false,
        true,
        false,
        0.25,
        4.0,
        2.0,
        true,
        0.45,
        0.5,
        false,
        0.15,
        0.5,
        true,
    ),
    e(
        ShadowOption::Other,
        false,
        true,
        false,
        0.45,
        12.0,
        8.0,
        true,
        0.85,
        0.5,
        false,
        0.18,
        1.0,
        true,
    ),
    e(
        ShadowOption::Standard,
        true,
        false,
        true,
        0.4,
        20.0,
        18.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Panel,
        true,
        false,
        true,
        0.25,
        8.0,
        4.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Tooltip,
        true,
        false,
        true,
        0.2,
        4.0,
        2.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Other,
        true,
        false,
        true,
        0.25,
        20.0,
        18.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Standard,
        true,
        true,
        true,
        0.7,
        20.0,
        18.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Panel,
        true,
        true,
        true,
        0.3,
        8.0,
        4.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Tooltip,
        true,
        true,
        true,
        0.25,
        4.0,
        2.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Other,
        true,
        true,
        true,
        0.45,
        20.0,
        18.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Standard,
        false,
        false,
        true,
        0.25,
        12.0,
        8.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Panel,
        false,
        false,
        true,
        0.25,
        8.0,
        4.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Tooltip,
        false,
        false,
        true,
        0.2,
        4.0,
        2.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Other,
        false,
        false,
        true,
        0.25,
        12.0,
        8.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Standard,
        false,
        true,
        true,
        0.45,
        12.0,
        8.0,
        true,
        0.7,
        1.0,
        false,
        0.4,
        1.0,
        true,
    ),
    e(
        ShadowOption::Panel,
        false,
        true,
        true,
        0.3,
        8.0,
        4.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Tooltip,
        false,
        true,
        true,
        0.25,
        4.0,
        2.0,
        true,
        0.7,
        1.0,
        false,
        0.7,
        1.0,
        true,
    ),
    e(
        ShadowOption::Other,
        false,
        true,
        true,
        0.45,
        12.0,
        8.0,
        true,
        0.7,
        1.0,
        false,
        0.4,
        1.0,
        true,
    ),
];

/// Row for a window class and state. `increased_contrast = false` selects the normal set.
pub fn window_shadow(
    option: ShadowOption,
    active: bool,
    dark: bool,
    increased_contrast: bool,
) -> &'static WindowShadowEntry {
    WINDOW_SHADOW_TABLE
        .iter()
        .find(|r| {
            r.option == option
                && r.active == active
                && r.dark == dark
                && r.increased_contrast == increased_contrast
        })
        .expect("the table has a row for every combination")
}

/// Window corner radius class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowCornerSize {
    Regular,
    /// the compact variant selected by a window predicate that was not resolved.
    Small,
}

impl WindowCornerSize {
    pub const ALL: [WindowCornerSize; 2] = [WindowCornerSize::Regular, WindowCornerSize::Small];

    /// Corner radius in points (16 regular, 10 small).
    pub fn radius_pt(self) -> f64 {
        match self {
            WindowCornerSize::Regular => WINDOW_CORNER_RADIUS_REGULAR,
            WindowCornerSize::Small => WINDOW_CORNER_RADIUS_SMALL,
        }
    }

    /// Edge length in texels of the 1× window-shape rendition (27 regular, 23 small).
    pub fn shape_rendition_px(self) -> u32 {
        match self {
            WindowCornerSize::Regular => 27,
            WindowCornerSize::Small => 23,
        }
    }
}

pub const WINDOW_CORNER_RADIUS_REGULAR: f64 = 16.0;
pub const WINDOW_CORNER_RADIUS_SMALL: f64 = 10.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_combination_resolves_exactly_once() {
        for opt in ShadowOption::ALL {
            for active in [true, false] {
                for dark in [true, false] {
                    for ic in [true, false] {
                        let n = WINDOW_SHADOW_TABLE
                            .iter()
                            .filter(|r| {
                                r.option == opt
                                    && r.active == active
                                    && r.dark == dark
                                    && r.increased_contrast == ic
                            })
                            .count();
                        assert_eq!(n, 1);
                        let _ = window_shadow(opt, active, dark, ic);
                    }
                }
            }
        }
    }

    #[test]
    fn documented_rows() {
        let a = window_shadow(ShadowOption::Standard, true, false, false);
        assert_eq!((a.sigma, a.vertical_offset, a.density), (20.0, 18.0, 0.40));
        let i = window_shadow(ShadowOption::Standard, false, false, false);
        assert_eq!((i.sigma, i.vertical_offset, i.density), (12.0, 8.0, 0.25));
        let d = window_shadow(ShadowOption::Standard, true, true, false);
        assert_eq!((d.sigma, d.density, d.clip_top), (20.0, 0.70, 0.95));
        let di = window_shadow(ShadowOption::Standard, false, true, false);
        assert_eq!((di.sigma, di.density), (12.0, 0.45));
        let t = window_shadow(ShadowOption::Tooltip, true, false, false);
        assert_eq!((t.sigma, t.vertical_offset, t.density), (4.0, 2.0, 0.20));
        let p = window_shadow(ShadowOption::Panel, true, true, false);
        assert_eq!((p.sigma, p.density), (8.0, 0.30));
        let o = window_shadow(ShadowOption::Other, true, true, false);
        assert_eq!(o.density, 0.45);
    }

    #[test]
    fn radius_conversion() {
        assert_eq!(sigma_to_radius_px(20.0), 56);
        assert_eq!(sigma_to_radius_px(12.0), 34);
        assert_eq!(sigma_to_radius_px(8.0), 23);
        assert_eq!(sigma_to_radius_px(4.0), 12);
        assert_eq!(sigma_to_radius_px(1000.0), 359);
        assert_eq!(rim_radius(1.0, 1.0), 2.0);
        assert_eq!(rim_radius(2.0, 1.0), 3.0);
        assert_eq!(rim_radius(1.0, 2.0), 3.0);
        assert_eq!(rim_radius(1.0, 1.99), 2.0);
    }

    #[test]
    fn extents() {
        let a = window_shadow(ShadowOption::Standard, true, false, false);
        assert_eq!(a.extent_above(), 56.0 - 18.0);
        assert_eq!(a.extent_below(), 56.0 + 18.0);
        assert_eq!(a.extent_side(), 56.0);
    }

    #[test]
    fn corner_radii() {
        assert_eq!(WindowCornerSize::Regular.radius_pt(), 16.0);
        assert_eq!(WindowCornerSize::Small.radius_pt(), 10.0);
        assert_eq!(WindowCornerSize::Regular.shape_rendition_px(), 27);
        assert_eq!(WindowCornerSize::Small.shape_rendition_px(), 23);
    }
}
