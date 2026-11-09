//! Colours of the decorations, resolved once from the theme tokens.
//!
//! Sources (all straight RGBA, sRGB):
//! * titlebar fills: opaque titlebar material recipes of the specification (the
//!   choice of the opaque variant is), `Titlebar-Opaque` for standard and
//!   `SeamlessWindowTitlebar-Opaque` for unified titlebars;
//! * title text: the window frame text colour (active) and the secondary label colour (inactive,
//!   approximate);
//! * rim: the window rim colour token and its 2× variant;
//! * button fills: the system red/yellow/green colours (approximate mapping; the design
//!   colours of the buttons are not part of the catalogs), inactive and disabled buttons use
//!   the system fill colour composited on the titlebar (approximate);
//! * glyph colour: black at 55 % (approximate, authored);
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
    pub glyph: Rgba,
    pub rim_1x: Rgba,
    pub rim_2x: Rgba,
    pub shadow: Rgba,
    pub window_background: Rgba,
}

fn rgba8(c: [u8; 4]) -> Rgba {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        c[3] as f32 / 255.0,
    ]
}

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
                title_inactive: rgba8([255, 255, 255, 140]),
                close: rgba8([0xFF, 0x45, 0x3A, 255]),
                minimize: rgba8([0xFF, 0xD6, 0x0A, 255]),
                zoom: rgba8([50, 215, 75, 255]),
                button_inactive: over(rgba8([255, 255, 255, 25]), titlebar_inactive),
                glyph: [0.0, 0.0, 0.0, 0.55],
                rim_1x: rgba8([0, 0, 0, 89]),
                rim_2x: rgba8([0, 0, 0, 89]),
                shadow: [0.0, 0.0, 0.0, 1.0],
                window_background: rgba8([31, 32, 37, 255]),
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
                title_inactive: rgba8([0, 0, 0, 127]),
                close: rgba8([0xFF, 0x3B, 0x30, 255]),
                minimize: rgba8([0xFF, 0xCC, 0x00, 255]),
                zoom: rgba8([40, 205, 65, 255]),
                button_inactive: over(rgba8([0, 0, 0, 25]), titlebar_inactive),
                glyph: [0.0, 0.0, 0.0, 0.55],
                rim_1x: rgba8([255, 255, 255, 89]),
                rim_2x: rgba8([255, 255, 255, 153]),
                shadow: [0.0, 0.0, 0.0, 1.0],
                window_background: rgba8([236, 236, 236, 255]),
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
        p.title_inactive = sem(SemanticColor::SecondaryLabel).unwrap_or(p.title_inactive);
        p.close = sem(SemanticColor::SystemRed).unwrap_or(p.close);
        p.minimize = sem(SemanticColor::SystemYellow).unwrap_or(p.minimize);
        p.zoom = sem(SemanticColor::SystemGreen).unwrap_or(p.zoom);
        p.rim_1x = sem(SemanticColor::WindowRim).unwrap_or(p.rim_1x);
        p.rim_2x = sem(SemanticColor::WindowRim2x).unwrap_or(p.rim_2x);
        p.window_background = sem(SemanticColor::WindowBackground).unwrap_or(p.window_background);
        if let Some(f) = sem(SemanticColor::SystemFill) {
            p.button_inactive = over(f, p.titlebar_inactive);
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
