//! Colours of the decorations, resolved once from the theme tokens.
//!
//! Sources (all straight RGBA, sRGB):
//! * titlebar fills: opaque titlebar material recipes of the specification (the
//!   choice of the opaque variant is), `Titlebar-Opaque` for standard and
//!   `SeamlessWindowTitlebar-Opaque` for unified titlebars;
//! * title text: the window frame text colour (active) and the tertiary label colour (inactive), so the title of a
//!   window that is not active recedes as its toolbar's does;
//! * rim: the window rim colour token and its 2× variant;
//! * button colours: the fill, the ring just inside the edge, the fill while pressed and the glyph shown while the
//!   group is hovered, each its own value per button; inactive and disabled buttons are one translucent disc,
//!   white at 17 % (dark) or black at 10 % (light), laid over whatever the window draws under them, so a tinted
//!   sidebar shows through them; Increase Contrast deepens them to 30 % and 18 %;
//! * separator: the grid colour (approximate).
//!
//! [`Palette::builtin`] carries the same numbers for use without the specification files; a test
//! compares it with the values loaded from the specification.

use crate::metrics::TitlebarStyle;
use cupertino_theme::recipe::{MaterialOptions, WindowState};
use cupertino_theme::{Appearance, SemanticColor, Theme};

pub type Rgba = [f32; 4];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub titlebar_active: Rgba,
    pub titlebar_inactive: Rgba,
    pub unified_active: Rgba,
    pub unified_inactive: Rgba,
    pub separator: Rgba,
    pub title_active: Rgba,
    pub title_inactive: Rgba,
    pub close: Rgba,
    pub minimize: Rgba,
    pub zoom: Rgba,
    pub button_inactive: Rgba,
    /// The ring of each button just inside its edge, in the order close, minimise, zoom.
    pub button_rim: [Rgba; 3],
    /// The colour each button's edge takes at its two sides, where the ring is deepest.
    pub button_edge: [Rgba; 3],
    /// The fill of each button while it is pressed.
    pub button_pressed: [Rgba; 3],
    /// The glyph each button shows while the group is hovered.
    pub button_glyph: [Rgba; 3],
    pub rim_1x: Rgba,
    pub rim_2x: Rgba,
    pub shadow: Rgba,
    pub window_background: Rgba,
}

/// The fills of the close, minimise and zoom buttons in both appearances, and the glyphs of the last two.
const CLOSE: [u8; 4] = [0xFF, 0x5F, 0x57, 255];
const MINIMIZE: [u8; 4] = [0xFE, 0xBC, 0x2E, 255];
const ZOOM: [u8; 4] = [0x28, 0xC8, 0x40, 255];
const MINIMIZE_GLYPH: [u8; 4] = [199, 56, 0, 255];
const ZOOM_GLYPH: [u8; 4] = [0, 118, 0, 255];
/// The close glyph of the light appearance.
const CLOSE_GLYPH_LIGHT: [u8; 4] = [165, 0, 0, 255];
/// The deepest colour of each button's edge, at its two sides (close, minimise, zoom).
const EDGE: [[u8; 4]; 3] = [[175, 3, 0, 255], [118, 20, 0, 255], [0, 78, 0, 255]];

/// A ring a little darker than the fill it goes round, for the appearance with no ring colours of its own.
fn darkened(c: Rgba) -> Rgba {
    [c[0] * 0.84, c[1] * 0.84, c[2] * 0.84, c[3]]
}

fn rgba8(c: [u8; 4]) -> Rgba {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        c[3] as f32 / 255.0,
    ]
}

/// A window button of a window that is not active, or a disabled one: white at 17 % in the dark appearance, laid over
/// what is under it, so the sidebar's tint shows through.
pub const INACTIVE_BUTTON_DARK: Rgba = [1.0, 1.0, 1.0, 0.17];
/// The same in the light appearance: black at 10 %.
pub const INACTIVE_BUTTON_LIGHT: Rgba = [0.0, 0.0, 0.0, 0.1];
/// With Increase Contrast: deeper, white at 30 % and black at 18 %.
pub const INACTIVE_BUTTON_DARK_HIGH_CONTRAST: Rgba = [1.0, 1.0, 1.0, 0.3];
pub const INACTIVE_BUTTON_LIGHT_HIGH_CONTRAST: Rgba = [0.0, 0.0, 0.0, 0.18];

fn gray(v: f32) -> Rgba {
    [v, v, v, 1.0]
}

/// Source-over of `top` (straight alpha) on an opaque `bottom`.
pub fn over(top: Rgba, bottom: Rgba) -> Rgba {
    let a = top[3];
    [
        top[0] * a + bottom[0] * (1.0 - a),
        top[1] * a + bottom[1] * (1.0 - a),
        top[2] * a + bottom[2] * (1.0 - a),
        1.0,
    ]
}

impl Palette {
    /// Palette without specification files; equal to [`Palette::from_theme`] (tested).
    pub fn builtin(dark: bool) -> Self {
        if dark {
            // an inactive window's title bar is the active one's: only its title and buttons change
            let titlebar_inactive = gray(0.149);
            Self {
                dark,
                titlebar_active: gray(0.149),
                titlebar_inactive,
                unified_active: gray(0.169),
                unified_inactive: gray(0.169),
                separator: rgba8([0x1A, 0x1A, 0x1A, 255]),
                title_active: rgba8([255, 255, 255, 216]),
                title_inactive: rgba8([255, 255, 255, 63]),
                close: rgba8(CLOSE),
                minimize: rgba8(MINIMIZE),
                zoom: rgba8(ZOOM),
                button_inactive: INACTIVE_BUTTON_DARK,
                button_rim: [
                    darkened(rgba8(CLOSE)),
                    darkened(rgba8(MINIMIZE)),
                    darkened(rgba8(ZOOM)),
                ],
                button_edge: EDGE.map(rgba8),
                button_pressed: [
                    rgba8([255, 141, 133, 255]),
                    rgba8([255, 234, 92, 255]),
                    rgba8([86, 246, 110, 255]),
                ],
                button_glyph: [
                    rgba8([153, 0, 0, 255]),
                    rgba8(MINIMIZE_GLYPH),
                    rgba8(ZOOM_GLYPH),
                ],
                rim_1x: rgba8([0, 0, 0, 89]),
                rim_2x: rgba8([0, 0, 0, 89]),
                shadow: [0.0, 0.0, 0.0, 1.0],
                window_background: rgba8([30, 30, 30, 255]),
            }
        } else {
            let titlebar_inactive = gray(1.0);
            Self {
                dark,
                titlebar_active: gray(1.0),
                titlebar_inactive,
                unified_active: gray(0.957),
                unified_inactive: gray(0.957),
                separator: rgba8([0xE6, 0xE6, 0xE6, 255]),
                title_active: rgba8([0, 0, 0, 216]),
                title_inactive: rgba8([0, 0, 0, 66]),
                close: rgba8(CLOSE),
                minimize: rgba8(MINIMIZE),
                zoom: rgba8(ZOOM),
                button_inactive: INACTIVE_BUTTON_LIGHT,
                button_rim: [
                    rgba8([243, 76, 68, 255]),
                    rgba8([241, 173, 25, 255]),
                    rgba8([19, 185, 44, 255]),
                ],
                button_edge: EDGE.map(rgba8),
                button_pressed: [
                    rgba8([252, 148, 142, 255]),
                    rgba8([254, 210, 115, 255]),
                    rgba8([110, 218, 126, 255]),
                ],
                button_glyph: [
                    rgba8(CLOSE_GLYPH_LIGHT),
                    rgba8(MINIMIZE_GLYPH),
                    rgba8(ZOOM_GLYPH),
                ],
                rim_1x: rgba8([255, 255, 255, 89]),
                rim_2x: rgba8([255, 255, 255, 153]),
                shadow: [0.0, 0.0, 0.0, 1.0],
                window_background: rgba8([255, 255, 255, 255]),
            }
        }
    }

    /// Resolve every colour from the theme; anything the theme lacks falls back to
    /// [`Palette::builtin`].
    pub fn from_theme(theme: &Theme, appearance: Appearance) -> Self {
        let mut p = Self::builtin(appearance.is_dark());
        let sem = |c: SemanticColor| theme.tokens.semantic(appearance, c).map(|c| c.to_array());
        let fill = |material: &str, window: WindowState| -> Option<Rgba> {
            let opts = MaterialOptions {
                window,
                reduce_transparency: true,
                ..Default::default()
            };
            let r = theme.recipe(appearance, material, &opts).ok()?;
            r.fills.last().map(|f| f.rgba)
        };
        if let Some(c) = fill("Titlebar", WindowState::Active) {
            p.titlebar_active = c;
        }
        if let Some(c) = fill("Titlebar", WindowState::Inactive) {
            p.titlebar_inactive = c;
        }
        if let Some(c) = fill("SeamlessWindowTitlebar", WindowState::Active) {
            p.unified_active = c;
        }
        if let Some(c) = fill("SeamlessWindowTitlebar", WindowState::Inactive) {
            p.unified_inactive = c;
        }
        p.separator = sem(SemanticColor::Grid).unwrap_or(p.separator);
        p.title_active = sem(SemanticColor::WindowFrameText).unwrap_or(p.title_active);
        p.title_inactive = sem(SemanticColor::TertiaryLabel).unwrap_or(p.title_inactive);
        p.rim_1x = sem(SemanticColor::WindowRim).unwrap_or(p.rim_1x);
        p.rim_2x = sem(SemanticColor::WindowRim2x).unwrap_or(p.rim_2x);
        p.window_background = sem(SemanticColor::WindowBackground).unwrap_or(p.window_background);
        if appearance.is_high_contrast() {
            p.button_inactive = if appearance.is_dark() {
                INACTIVE_BUTTON_DARK_HIGH_CONTRAST
            } else {
                INACTIVE_BUTTON_LIGHT_HIGH_CONTRAST
            };
        }
        p
    }

    pub fn titlebar(&self, style: TitlebarStyle, active: bool) -> Rgba {
        match (style, active) {
            (TitlebarStyle::Standard, true) => self.titlebar_active,
            (TitlebarStyle::Standard, false) => self.titlebar_inactive,
            (_, true) => self.unified_active,
            (_, false) => self.unified_inactive,
        }
    }

    /// Rim colour for a backing scale: the 2× token from a scale of 2 up (selection).
    pub fn rim(&self, scale: f64) -> Rgba {
        if scale >= 2.0 {
            self.rim_2x
        } else {
            self.rim_1x
        }
    }

    pub fn button_color(&self, kind: crate::metrics::ButtonKind) -> Rgba {
        match kind {
            crate::metrics::ButtonKind::Close => self.close,
            crate::metrics::ButtonKind::Minimize => self.minimize,
            crate::metrics::ButtonKind::Zoom => self.zoom,
        }
    }

    fn index(kind: crate::metrics::ButtonKind) -> usize {
        match kind {
            crate::metrics::ButtonKind::Close => 0,
            crate::metrics::ButtonKind::Minimize => 1,
            crate::metrics::ButtonKind::Zoom => 2,
        }
    }

    /// The ring of a button just inside its edge.
    pub fn rim_of(&self, kind: crate::metrics::ButtonKind) -> Rgba {
        self.button_rim[Self::index(kind)]
    }

    /// The colour of a button's edge at its sides.
    pub fn edge_of(&self, kind: crate::metrics::ButtonKind) -> Rgba {
        self.button_edge[Self::index(kind)]
    }

    /// The fill of a pressed button.
    pub fn pressed_fill(&self, kind: crate::metrics::ButtonKind) -> Rgba {
        self.button_pressed[Self::index(kind)]
    }

    /// The glyph a button shows while the group is hovered.
    pub fn glyph_of(&self, kind: crate::metrics::ButtonKind) -> Rgba {
        self.button_glyph[Self::index(kind)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rgba, b: Rgba) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1.5 / 255.0)
    }

    #[test]
    fn builtin_equals_the_palette_loaded_from_the_specification() {
        let theme = Theme::load().unwrap();
        for (appearance, dark) in [(Appearance::Light, false), (Appearance::Dark, true)] {
            let (a, b) = (
                Palette::from_theme(&theme, appearance),
                Palette::builtin(dark),
            );
            assert!(
                close(a.titlebar_active, b.titlebar_active),
                "{appearance:?} titlebar active {:?} {:?}",
                a.titlebar_active,
                b.titlebar_active
            );
            assert!(
                close(a.titlebar_inactive, b.titlebar_inactive),
                "{appearance:?} inactive"
            );
            assert!(
                close(a.unified_active, b.unified_active)
                    && close(a.unified_inactive, b.unified_inactive),
                "{appearance:?} unified"
            );
            assert!(
                close(a.separator, b.separator)
                    && close(a.title_active, b.title_active)
                    && close(a.title_inactive, b.title_inactive)
            );
            assert!(
                close(a.close, b.close) && close(a.minimize, b.minimize) && close(a.zoom, b.zoom),
                "{appearance:?} buttons"
            );
            assert!(
                close(a.rim_1x, b.rim_1x)
                    && close(a.rim_2x, b.rim_2x)
                    && close(a.window_background, b.window_background)
            );
            assert!(
                close(a.button_inactive, b.button_inactive),
                "{appearance:?} {:?} {:?}",
                a.button_inactive,
                b.button_inactive
            );
        }
    }

    /// An inactive window's title bars are the active one's, in every appearance: only the title and the buttons
    /// change, so the window does not darken when another takes the focus.
    #[test]
    fn an_inactive_title_bar_is_the_active_one() {
        let theme = Theme::load().unwrap();
        for appearance in Appearance::ALL {
            let p = Palette::from_theme(&theme, appearance);
            assert_eq!(p.titlebar_inactive, p.titlebar_active, "{appearance:?}");
            assert_eq!(p.unified_inactive, p.unified_active, "{appearance:?}");
            assert_ne!(p.title_inactive, p.title_active, "{appearance:?}");
        }
    }

    #[test]
    fn rim_token_depends_on_the_scale() {
        let p = Palette::builtin(false);
        assert!((p.rim(1.0)[3] - 0.349).abs() < 0.002);
        assert!((p.rim(2.0)[3] - 0.6).abs() < 0.002);
        assert!((p.rim(1.5)[3] - 0.349).abs() < 0.002);
    }

    #[test]
    fn titlebar_selection() {
        let p = Palette::builtin(false);
        assert_eq!(p.titlebar(TitlebarStyle::Standard, true), p.titlebar_active);
        assert_eq!(
            p.titlebar(TitlebarStyle::Unified, false),
            p.unified_inactive
        );
    }

    #[test]
    fn high_contrast_catalogs_resolve() {
        let theme = Theme::load().unwrap();
        let p = Palette::from_theme(&theme, Appearance::DarkHighContrast);
        assert!(p.dark);
        assert!(p.titlebar_active.iter().all(|c| c.is_finite()));
    }
}
