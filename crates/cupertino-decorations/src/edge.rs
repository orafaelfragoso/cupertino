//! The edge of a window and of a menu: a fine dark outline just outside the shape and a faint light rim just inside it.
//!
//! The outline is half a point wide (one device pixel at 2×); with Increase Contrast it is a full point of the
//! high-contrast `window-rim` colour, a visible border. A window's outline has the same strength all around. A menu's
//! (and a popover's) follows the direction of its edge: full along the sides, fading around the corners to nothing at
//! the top and bottom ([`OutlineFalloff::Sides`]). The inner rim of a window is white at half the inner-rim density of
//! its row of the shadow table, as wide in points as that row's inner-rim scale (1 pt in the dark appearance, ½ pt in
//! the light one). A menu's inner rim is a white line half a point wide, brightest along the top and bottom and fainter
//! along the sides ([`OutlineFalloff::Ends`]), so the glass reads lit all around; with Increase Contrast the border
//! stands alone.
//!
//! A line narrower than a device pixel is drawn one device pixel wide at the share of its strength the pixel's
//! coverage gives ([`Hairline::device`]), so half a point at 1× is one pixel at half strength.

use crate::palette::Rgba;
use cupertino_theme::window_shadow::{window_shadow, ShadowOption};

/// Width in points of the outline around a window or a menu.
pub const OUTLINE_WIDTH: f64 = 0.5;
/// Width in points of the outline with Increase Contrast.
pub const CONTRAST_OUTLINE_WIDTH: f64 = 1.0;
/// Opacity of the black outline of an active window in the dark appearance.
pub const WINDOW_OUTLINE_DARK: f32 = 0.93;
/// Opacity of the black outline of an active window in the light appearance.
pub const WINDOW_OUTLINE_LIGHT: f32 = 0.22;
/// Share of the active outline's opacity an inactive window keeps.
pub const INACTIVE_OUTLINE_SHARE: f32 = 0.62;
/// Opacity of the black outline along the sides of a menu in the dark appearance.
pub const MENU_OUTLINE_DARK: f32 = 0.65;
/// Opacity of the black outline along the sides of a menu in the light appearance.
pub const MENU_OUTLINE_LIGHT: f32 = 0.5;
/// Share of the shadow table's inner-rim density the white inner rim of a window shows.
pub const INNER_RIM_SHARE: f64 = 0.5;
/// How sharply a menu's outline fades from its sides towards its top and bottom: the outline's strength is the
/// horizontal part of the edge's outward normal to this power.
pub const SIDE_FALLOFF_POWER: f32 = 2.0;
/// Width in points of the light rim inside a menu, in the dark and the light appearance.
pub const MENU_RIM_WIDTH: f64 = 0.5;
pub const MENU_RIM_WIDTH_LIGHT: f64 = 1.0;
/// Opacity of the white rim along the top and bottom of a menu in the dark appearance.
pub const MENU_RIM_DARK: f32 = 0.36;
/// Opacity of the white rim along the top and bottom of a menu in the light appearance.
pub const MENU_RIM_LIGHT: f32 = 1.0;
/// Share of the menu rim's top and bottom opacity left along its sides.
pub const MENU_RIM_SIDE_SHARE: f32 = 0.1;
/// How quickly the menu rim fades from its top and bottom towards its sides: the vertical part of the edge's outward
/// normal to this power.
pub const MENU_RIM_FALLOFF_POWER: f32 = 1.0;

/// What carries the edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// A window: a uniform outline and a light inner rim.
    Window,
    /// A menu or popover of glass: an outline along the sides and a light rim brightest at the top and bottom.
    Menu,
}

/// How the strength of a line of the edge follows the direction of the edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutlineFalloff {
    /// The same all around.
    Uniform,
    /// Full along the sides, `|n.x|^power` of the unit outward normal `n` elsewhere: nothing at the top and bottom.
    Sides { power: f32 },
    /// Full along the top and bottom, `floor + (1 − floor) · |n.y|^power` elsewhere: `floor` of it along the sides.
    Ends { power: f32, floor: f32 },
}

impl OutlineFalloff {
    /// What a shader takes: the powers on the horizontal and the vertical part of the unit outward normal (0 leaves
    /// that factor out) and the floor, for a strength of `floor + (1 − floor) · |n.x|^sides · |n.y|^ends`.
    pub fn shader(self) -> (f32, f32, f32) {
        match self {
            OutlineFalloff::Uniform => (0.0, 0.0, 0.0),
            OutlineFalloff::Sides { power } => (power, 0.0, 0.0),
            OutlineFalloff::Ends { power, floor } => (0.0, power, floor),
        }
    }

    /// Strength (0 to 1) of the line where the edge's outward normal is `normal` (need not be unit length).
    pub fn weight(self, normal: (f64, f64)) -> f64 {
        let (sides, ends, floor) = self.shader();
        if sides <= 0.0 && ends <= 0.0 {
            return 1.0;
        }
        let len = normal.0.hypot(normal.1);
        let factor = if len > 0.0 && len.is_finite() {
            let part = |c: f64, power: f32| {
                if power > 0.0 {
                    (c.abs() / len).powf(f64::from(power))
                } else {
                    1.0
                }
            };
            part(normal.0, sides) * part(normal.1, ends)
        } else {
            0.0
        };
        let floor = f64::from(floor);
        floor + (1.0 - floor) * factor
    }
}

/// A line along the edge: its straight colour and its width in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hairline {
    pub color: Rgba,
    pub width_pt: f64,
}

impl Hairline {
    /// The line in device pixels at `scale`: a whole number of pixels, at least one, and the colour with the
    /// opacity the pixels' coverage leaves (a line half a pixel wide is one pixel at half its opacity).
    pub fn device(&self, scale: f64) -> (u32, Rgba) {
        let exact = (self.width_pt * scale).max(0.0);
        let px = exact.round().max(1.0);
        let cover = (exact / px).min(1.0) as f32;
        let mut color = self.color;
        color[3] *= cover;
        (px as u32, color)
    }
}

/// The edge of a window or a menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// Just outside the shape.
    pub outline: Hairline,
    pub falloff: OutlineFalloff,
    /// Just inside the shape, over the content.
    pub inner: Option<Hairline>,
    pub inner_falloff: OutlineFalloff,
}

/// The edge of `kind` in the dark or light appearance, active or not; with Increase Contrast the outline is
/// `contrast_rim` (the high-contrast `window-rim` colour) all around.
pub fn edge(
    kind: EdgeKind,
    dark: bool,
    active: bool,
    increased_contrast: bool,
    contrast_rim: Rgba,
) -> Edge {
    let outline = if increased_contrast {
        Hairline {
            color: contrast_rim,
            width_pt: CONTRAST_OUTLINE_WIDTH,
        }
    } else {
        let full = match (kind, dark) {
            (EdgeKind::Window, true) => WINDOW_OUTLINE_DARK,
            (EdgeKind::Window, false) => WINDOW_OUTLINE_LIGHT,
            (EdgeKind::Menu, true) => MENU_OUTLINE_DARK,
            (EdgeKind::Menu, false) => MENU_OUTLINE_LIGHT,
        };
        let share = if active || kind == EdgeKind::Menu {
            1.0
        } else {
            INACTIVE_OUTLINE_SHARE
        };
        Hairline {
            color: [0.0, 0.0, 0.0, full * share],
            width_pt: OUTLINE_WIDTH,
        }
    };
    let falloff = match kind {
        EdgeKind::Menu if !increased_contrast => OutlineFalloff::Sides {
            power: SIDE_FALLOFF_POWER,
        },
        _ => OutlineFalloff::Uniform,
    };
    let (inner, inner_falloff) = match kind {
        EdgeKind::Window => {
            let row = window_shadow(ShadowOption::Standard, active, dark, increased_contrast);
            let rim = Hairline {
                color: [
                    1.0,
                    1.0,
                    1.0,
                    (row.inner_rim_density * INNER_RIM_SHARE) as f32,
                ],
                width_pt: row.inner_rim_radius_scale,
            };
            (Some(rim), OutlineFalloff::Uniform)
        }
        // the high-contrast border stands alone
        EdgeKind::Menu if increased_contrast => (None, OutlineFalloff::Uniform),
        EdgeKind::Menu => {
            let rim = Hairline {
                color: [
                    1.0,
                    1.0,
                    1.0,
                    if dark { MENU_RIM_DARK } else { MENU_RIM_LIGHT },
                ],
                width_pt: if dark {
                    MENU_RIM_WIDTH
                } else {
                    MENU_RIM_WIDTH_LIGHT
                },
            };
            let falloff = OutlineFalloff::Ends {
                power: MENU_RIM_FALLOFF_POWER,
                floor: MENU_RIM_SIDE_SHARE,
            };
            (Some(rim), falloff)
        }
    };
    Edge {
        outline,
        falloff,
        inner,
        inner_falloff,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIM: Rgba = [0.0, 0.0, 0.0, 0.5];

    fn window(dark: bool, active: bool) -> Edge {
        edge(EdgeKind::Window, dark, active, false, RIM)
    }

    #[test]
    fn a_window_has_a_dark_hairline_outside_and_a_faint_white_rim_inside() {
        let d = window(true, true);
        assert_eq!(d.outline.color, [0.0, 0.0, 0.0, 0.93]);
        assert_eq!(d.outline.width_pt, 0.5);
        assert_eq!(d.falloff, OutlineFalloff::Uniform);
        let inner = d.inner.unwrap();
        assert_eq!(inner.color, [1.0, 1.0, 1.0, 0.1]);
        assert_eq!(inner.width_pt, 1.0, "a point wide in the dark appearance");
        let l = window(false, true);
        assert_eq!(l.outline.color[3], 0.22);
        let inner = l.inner.unwrap();
        assert_eq!(inner.color, [1.0, 1.0, 1.0, 0.05]);
        assert_eq!(inner.width_pt, 0.5, "half a point in the light appearance");
        for e in [d, l] {
            assert!(e.outline.color[..3].iter().all(|c| *c == 0.0), "black");
        }
    }

    #[test]
    fn an_inactive_window_has_a_lighter_outline() {
        for dark in [true, false] {
            let (a, i) = (window(dark, true), window(dark, false));
            assert!((i.outline.color[3] - a.outline.color[3] * 0.62).abs() < 1e-6);
        }
        assert!((window(true, false).inner.unwrap().color[3] - 0.09).abs() < 1e-6);
    }

    #[test]
    fn a_menu_is_outlined_along_its_sides_and_lit_along_its_top_and_bottom() {
        let d = edge(EdgeKind::Menu, true, true, false, RIM);
        assert_eq!(d.outline.color[3], MENU_OUTLINE_DARK);
        assert_eq!(d.falloff, OutlineFalloff::Sides { power: 2.0 });
        assert_eq!(
            d.inner,
            Some(Hairline {
                color: [1.0, 1.0, 1.0, MENU_RIM_DARK],
                width_pt: 0.5
            })
        );
        let rim = d.inner_falloff;
        assert_eq!(rim.weight((0.0, -1.0)), 1.0, "full along the top");
        assert_eq!(rim.weight((0.0, 1.0)), 1.0, "and the bottom");
        assert!(
            (rim.weight((1.0, 0.0)) - f64::from(MENU_RIM_SIDE_SHARE)).abs() < 1e-6,
            "faint along the sides"
        );
        let diagonal = rim.weight((1.0, 1.0));
        assert!(diagonal > f64::from(MENU_RIM_SIDE_SHARE) && diagonal < 1.0);
        let light = edge(EdgeKind::Menu, false, true, false, RIM);
        assert_eq!(
            light.inner,
            Some(Hairline {
                color: [1.0, 1.0, 1.0, MENU_RIM_LIGHT],
                width_pt: 1.0
            }),
            "a point wide in the light appearance"
        );
        assert_eq!(light.outline.color[3], MENU_OUTLINE_LIGHT);
        assert_eq!(
            edge(EdgeKind::Menu, true, false, false, RIM),
            d,
            "a menu is never inactive"
        );
    }

    #[test]
    fn increase_contrast_keeps_a_point_wide_border_all_around() {
        for kind in [EdgeKind::Window, EdgeKind::Menu] {
            for dark in [true, false] {
                let e = edge(kind, dark, true, true, RIM);
                assert_eq!(
                    e.outline,
                    Hairline {
                        color: RIM,
                        width_pt: 1.0
                    }
                );
                assert_eq!(e.falloff, OutlineFalloff::Uniform);
            }
        }
        assert_eq!(edge(EdgeKind::Menu, true, true, true, RIM).inner, None);
        let inner = edge(EdgeKind::Window, true, true, true, RIM).inner.unwrap();
        assert!((inner.color[3] - 0.35).abs() < 1e-6 && inner.width_pt == 1.0);
    }

    #[test]
    fn the_side_falloff_is_full_at_the_sides_and_nothing_at_the_top_and_bottom() {
        let f = OutlineFalloff::Sides { power: 2.0 };
        assert_eq!(f.weight((1.0, 0.0)), 1.0);
        assert_eq!(f.weight((-3.0, 0.0)), 1.0);
        assert_eq!(f.weight((0.0, 1.0)), 0.0);
        assert_eq!(f.weight((0.0, -1.0)), 0.0);
        assert!((f.weight((1.0, 1.0)) - 0.5).abs() < 1e-12, "half at 45°");
        assert_eq!(f.weight((0.0, 0.0)), 0.0);
        assert_eq!(OutlineFalloff::Uniform.weight((0.0, 1.0)), 1.0);
        assert_eq!(OutlineFalloff::Uniform.shader(), (0.0, 0.0, 0.0));
        assert_eq!(f.shader(), (2.0, 0.0, 0.0));
    }

    #[test]
    fn the_end_falloff_is_full_at_the_top_and_bottom_and_its_floor_at_the_sides() {
        let f = OutlineFalloff::Ends {
            power: 1.0,
            floor: 0.4,
        };
        assert_eq!(f.weight((0.0, -2.0)), 1.0);
        assert_eq!(f.weight((0.0, 1.0)), 1.0);
        assert!((f.weight((1.0, 0.0)) - 0.4).abs() < 1e-6);
        assert!((f.weight((-1.0, 0.0)) - 0.4).abs() < 1e-6);
        let diagonal = 0.4 + 0.6 * std::f64::consts::FRAC_1_SQRT_2;
        assert!((f.weight((1.0, 1.0)) - diagonal).abs() < 1e-6);
        assert!((f.weight((0.0, 0.0)) - 0.4).abs() < 1e-6, "no direction");
        assert_eq!(f.shader(), (0.0, 1.0, 0.4));
    }

    #[test]
    fn a_hairline_is_whole_device_pixels_at_the_strength_its_coverage_gives() {
        let line = Hairline {
            color: [0.0, 0.0, 0.0, 0.8],
            width_pt: 0.5,
        };
        assert_eq!(line.device(1.0), (1, [0.0, 0.0, 0.0, 0.4]));
        assert_eq!(line.device(2.0), (1, [0.0, 0.0, 0.0, 0.8]));
        let (px, c) = line.device(3.0);
        assert_eq!(px, 2);
        assert!((c[3] - 0.6).abs() < 1e-6);
        let (px, c) = line.device(1.5);
        assert_eq!(px, 1);
        assert!((c[3] - 0.6).abs() < 1e-6);
        let point = Hairline {
            width_pt: 1.0,
            ..line
        };
        assert_eq!(point.device(1.0), (1, line.color));
        assert_eq!(point.device(2.0), (2, line.color));
    }
}
