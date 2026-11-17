//! Text and symbols on glass. The compositor draws glass under a part of a window, and the window draws its text over
//! it; that text goes through the foreground matrix of the glass (`crate::glass`): the face under it lifted
//! to white on dark glass, pushed to black on light glass. A small shape takes the matrix of the side its face was
//! measured on, so a control over a light backdrop in the dark appearance gets dark text; larger bars and panels keep
//! their appearance's. Labels on glass are also drawn more opaque than on a plain window, since the face behind them
//! is busier: a primary label at full strength, a placeholder somewhat less, and a secondary label at the strength
//! that shifts the face under it by the secondary shift of its side (darker on light glass, lighter on dark glass).

use crate::glass::{foreground_rgb, FaceTone};
pub use crate::glass::{vibrant_secondary, VIBRANT_PLACEHOLDER, VIBRANT_PRIMARY};
use crate::ui::{Color, ColorRole, InteractionVariant, ThemeTokens};

/// How strongly the text of `role` is drawn on glass whose foreground is the `dark` side's (white text), or `None`
/// for a colour that stays as the theme has it (fills, accents, the text of a filled control).
pub fn vibrant_level(role: ColorRole, dark: bool) -> Option<f32> {
    match role {
        ColorRole::Text | ColorRole::ControlText | ColorRole::Header => Some(VIBRANT_PRIMARY),
        ColorRole::SecondaryText => Some(vibrant_secondary(dark)),
        ColorRole::PlaceholderText => Some(VIBRANT_PLACEHOLDER),
        _ => None,
    }
}

/// The colour of text and symbols on glass of the `dark` appearance whose face was measured as `tone` (`None` when it
/// was not, or when the shape is too large to follow what lies behind it), opaque.
pub fn glass_foreground(dark: bool, tone: Option<FaceTone>) -> Color {
    let [r, g, b] = foreground_rgb(dark, tone);
    Color::new(r, g, b, 1.0)
}

/// Whether `c` is a neutral label colour: white or black at any opacity, as the theme draws text on a plain window.
fn neutral(c: Color) -> bool {
    let white = [c.r, c.g, c.b].iter().all(|v| *v >= 0.99);
    let black = [c.r, c.g, c.b].iter().all(|v| *v <= 0.01);
    white || black
}

/// How far a grey channel may stray from the others for the colour to count as grey.
const GREY_SPREAD: f32 = 0.02;

/// How strongly the label colour `c` is drawn, as the opacity of its appearance's foreground, or `None` for a colour
/// that is not a label's: white or black at its own opacity, and an opaque grey (the labels of the high-contrast
/// themes) as far as it lies towards the appearance's foreground, white in the `dark` appearance, black in the light.
fn label_strength(c: Color, dark: bool) -> Option<f32> {
    if neutral(c) {
        return Some(c.a);
    }
    let (lo, hi) = (c.r.min(c.g).min(c.b), c.r.max(c.g).max(c.b));
    if hi - lo > GREY_SPREAD {
        return None;
    }
    let lightness = (c.r + c.g + c.b) / 3.0;
    Some(c.a * if dark { lightness } else { 1.0 - lightness })
}

impl ThemeTokens {
    /// The theme of what a window draws on glass whose face was measured as `tone`: the label colours of text and
    /// symbols (white, black or grey) take [`glass_foreground`] at their [`vibrant_level`], or at their own strength
    /// where that is more ([`label_strength`]; a disabled label keeps its own); a selected label, drawn on the fill of
    /// its selection, and every other colour are unchanged. The foreground is the same in every accessibility mode.
    pub fn on_glass(&self, tone: Option<FaceTone>) -> ThemeTokens {
        let dark = self.preferences.appearance.is_dark();
        let fg = glass_foreground(dark, tone);
        // the side whose foreground the labels take: the measured face's, else the appearance's
        let side = tone.map_or(dark, |t| t == FaceTone::Dark);
        let colors = self.colors.map(|role, state, c| {
            let (Some(level), Some(strength)) =
                (vibrant_level(role, side), label_strength(c, dark))
            else {
                return c;
            };
            // a selected label is drawn on its selection's fill, not on the glass
            if state.selected {
                return c;
            }
            let a = if state.interaction == InteractionVariant::Disabled {
                strength
            } else {
                strength.max(level)
            };
            Color { a, ..fg }
        });
        ThemeTokens {
            colors,
            ..self.clone()
        }
    }
}
