//! The edge of a window and of a menu: a fine dark outline just outside the shape and a faint light rim just inside it.
//!
//! The outline is half a point wide (one device pixel at 2×); with Increase Contrast it is a full point of the
//! high-contrast `window-rim` colour, a visible border. A window's outline has the same strength all around. A menu's
//! (and a popover's) follows the direction of its edge: full along the sides, fading around the corners to nothing at
//! the top and bottom in the dark appearance and to a quarter of it in the light one ([`OutlineFalloff::Sides`]). The inner rim of a window is white at half the inner-rim density of
//! its row of the shadow table, as wide in points as that row's inner-rim scale (1 pt in the dark appearance, ½ pt in
//! the light one). A menu's inner rim is a white line half a point wide, brightest along the top and bottom and fainter
//! along the sides ([`OutlineFalloff::Ends`]), so the glass reads lit all around; with Increase Contrast the border
//! stands alone.
//!
//! A line narrower than a device pixel is drawn one device pixel wide at the share of its strength the pixel's
//! coverage gives ([`Hairline::device`]), so half a point at 1× is one pixel at half strength.

use crate::decorations::palette::Rgba;
use cupertino_theme::window_shadow::{window_shadow, ShadowOption};

/// Width in points of the outline around a window or a menu.
pub const OUTLINE_WIDTH: f64 = 0.5;
/// Width in points of the outline with Increase Contrast.
pub const CONTRAST_OUTLINE_WIDTH: f64 = 1.0;
/// Opacity of the black outline of an active window in the dark appearance.
pub const WINDOW_OUTLINE_DARK: f32 = 0.93;
/// Opacity of the black outline of an active window in the light appearance.
pub const WINDOW_OUTLINE_LIGHT: f32 = 0.2;
/// Share of the active outline's opacity an inactive window keeps: an inactive window's outline stays nearly as dark
/// (0.83 against 0.93 in the dark appearance).
pub const INACTIVE_OUTLINE_SHARE: f32 = 0.89;
/// Opacity of the black outline along the sides of a menu in the dark appearance.
pub const MENU_OUTLINE_DARK: f32 = 0.7;
/// Opacity of the black outline along the sides of a menu in the light appearance.
pub const MENU_OUTLINE_LIGHT: f32 = 0.5;
/// Share of the shadow table's inner-rim density the white inner rim of a window shows.
pub const INNER_RIM_SHARE: f64 = 0.5;
/// How sharply a menu's outline fades from its sides towards its top and bottom: the outline's strength is the
/// horizontal part of the edge's outward normal to this power.
pub const SIDE_FALLOFF_POWER: f32 = 2.0;
/// Share of a menu's side outline left along its top and bottom in the light appearance, a faint line there; the dark
/// appearance keeps none.
pub const MENU_OUTLINE_ENDS_LIGHT: f32 = 0.25;
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
    /// Full along the sides, `floor + (1 − floor) · |n.x|^power` of the unit outward normal `n` elsewhere: `floor` of
    /// it along the top and bottom.
    Sides { power: f32, floor: f32 },
    /// Full along the top and bottom, `floor + (1 − floor) · |n.y|^power` elsewhere: `floor` of it along the sides.
    Ends { power: f32, floor: f32 },
}

impl OutlineFalloff {
    /// What a shader takes: the powers on the horizontal and the vertical part of the unit outward normal (0 leaves
    /// that factor out) and the floor, for a strength of `floor + (1 − floor) · |n.x|^sides · |n.y|^ends`.
    pub fn shader(self) -> (f32, f32, f32) {
        match self {
            OutlineFalloff::Uniform => (0.0, 0.0, 0.0),
            OutlineFalloff::Sides { power, floor } => (power, 0.0, floor),
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
            floor: if dark { 0.0 } else { MENU_OUTLINE_ENDS_LIGHT },
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
