//! Text and symbols on glass. The compositor draws glass under a part of a window, and the window draws its text over
//! it; that text goes through the foreground matrix of the glass (`cupertino_theme::glass`): the face under it lifted
//! to white on dark glass, pushed to black on light glass. A small shape takes the matrix of the side its face was
//! measured on, so a control over a light backdrop in the dark appearance gets dark text; larger bars and panels keep
//! their appearance's. Labels on glass are also drawn more opaque than on a plain window, since the face behind them
//! is busier: a primary label at full strength, a secondary one and a placeholder somewhat less.

use crate::{Color, ColorRole, InteractionVariant, ThemeTokens};
use cupertino_theme::glass::{apply_matrix, foreground_matrix_for, FaceTone};

/// How strongly a primary label (text, a control's text, a header) is drawn on glass.
pub const VIBRANT_PRIMARY: f32 = 1.0;
/// How strongly a secondary label is drawn on glass.
pub const VIBRANT_SECONDARY: f32 = 0.75;
/// How strongly a text field's placeholder is drawn on glass.
pub const VIBRANT_PLACEHOLDER: f32 = 0.7;

/// The face the foreground matrix is applied to when the window does not know the face: a mid grey. Both matrices
/// carry any face but the darkest to white or to black, so the exact face hardly matters.
const NOMINAL_FACE: f32 = 0.5;

/// How strongly the text of `role` is drawn on glass, or `None` for a colour that stays as the theme has it (fills,
/// accents, the text of a filled control).
pub fn vibrant_level(role: ColorRole) -> Option<f32> {
    match role {
        ColorRole::Text | ColorRole::ControlText | ColorRole::Header => Some(VIBRANT_PRIMARY),
        ColorRole::SecondaryText => Some(VIBRANT_SECONDARY),
        ColorRole::PlaceholderText => Some(VIBRANT_PLACEHOLDER),
        _ => None,
    }
}

/// The colour of text and symbols on glass of the `dark` appearance whose face was measured as `tone` (`None` when it
/// was not, or when the shape is too large to follow what lies behind it), opaque.
pub fn glass_foreground(dark: bool, tone: Option<FaceTone>) -> Color {
    let m = foreground_matrix_for(dark, tone);
    let out = apply_matrix(&m, [NOMINAL_FACE, NOMINAL_FACE, NOMINAL_FACE, 1.0]);
    Color::new(
        out[0].clamp(0.0, 1.0),
        out[1].clamp(0.0, 1.0),
        out[2].clamp(0.0, 1.0),
        1.0,
    )
}

/// Whether `c` is a neutral label colour: white or black at any opacity, as the theme draws text on a plain window.
fn neutral(c: Color) -> bool {
    let white = [c.r, c.g, c.b].iter().all(|v| *v >= 0.99);
    let black = [c.r, c.g, c.b].iter().all(|v| *v <= 0.01);
    white || black
}

impl ThemeTokens {
    /// The theme of what a window draws on glass whose face was measured as `tone`: the neutral label colours of text
    /// and symbols take [`glass_foreground`] at their [`vibrant_level`] (a disabled label keeps its own opacity); a
    /// selected label, drawn on the fill of its selection, and every other colour are unchanged.
    pub fn on_glass(&self, tone: Option<FaceTone>) -> ThemeTokens {
        let fg = glass_foreground(self.preferences.appearance.is_dark(), tone);
        let colors = self.colors.map(|role, state, c| {
            // a selected label is drawn on its selection's fill, not on the glass
            let Some(level) = vibrant_level(role).filter(|_| neutral(c) && !state.selected) else {
                return c;
            };
            let a = if state.interaction == InteractionVariant::Disabled {
                c.a
            } else {
                c.a.max(level)
            };
            Color { a, ..fg }
        });
        ThemeTokens {
            colors,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Appearance, StateVariant, ThemePreferences};

    fn theme(appearance: Appearance) -> ThemeTokens {
        let preferences = ThemePreferences {
            appearance,
            reduce_transparency: false,
            reduce_motion: false,
        };
        ThemeTokens::cupertino(&cupertino_theme::Theme::load().unwrap(), preferences).unwrap()
    }

    fn get(t: &ThemeTokens, role: ColorRole) -> Color {
        t.colors.get(role, StateVariant::NORMAL).unwrap()
    }

    #[test]
    fn labels_on_glass_follow_the_matrix_of_the_face() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let t = theme(appearance);
            let dark = appearance.is_dark();
            let white = |c: Color| c.r == 1.0 && c.g == 1.0 && c.b == 1.0;
            let black = |c: Color| c.r == 0.0 && c.g == 0.0 && c.b == 0.0;
            // unmeasured, or a large shape: the appearance's foreground, primary labels at full strength
            let large = t.on_glass(None);
            let text = get(&large, ColorRole::Text);
            assert!(if dark { white(text) } else { black(text) });
            assert_eq!(text.a, VIBRANT_PRIMARY);
            // a small shape flips with its face, whatever the appearance
            let over_light = get(&t.on_glass(Some(FaceTone::Light)), ColorRole::Text);
            let over_dark = get(&t.on_glass(Some(FaceTone::Dark)), ColorRole::Text);
            assert!(black(over_light) && white(over_dark), "{appearance:?}");
            // the secondary label and the placeholder keep their places below the primary label
            let secondary = get(&large, ColorRole::SecondaryText).a;
            let placeholder = get(&large, ColorRole::PlaceholderText).a;
            assert!(secondary >= VIBRANT_SECONDARY && placeholder >= VIBRANT_PLACEHOLDER);
            assert!(placeholder <= secondary && secondary <= VIBRANT_PRIMARY);
        }
    }

    #[test]
    fn accents_fills_and_disabled_text_keep_their_colours() {
        let t = theme(Appearance::Dark);
        let glass = t.on_glass(Some(FaceTone::Light));
        for role in [
            ColorRole::SelectedControlFill,
            ColorRole::Link,
            ColorRole::DefaultButtonText,
            ColorRole::GroupFill,
            ColorRole::SwitchThumb,
        ] {
            assert_eq!(get(&glass, role), get(&t, role), "{role:?}");
        }
        let disabled = StateVariant {
            interaction: InteractionVariant::Disabled,
            ..StateVariant::NORMAL
        };
        let before = t.colors.get(ColorRole::Text, disabled).unwrap();
        let after = glass.colors.get(ColorRole::Text, disabled).unwrap();
        assert!(neutral(before));
        assert_eq!(
            after.a, before.a,
            "a disabled label stays as dim as the theme has it"
        );
        // the symbol on an accent-filled switch is the selected control text; it stays as the theme has it
        let selected = StateVariant {
            selected: true,
            ..StateVariant::NORMAL
        };
        let light = theme(Appearance::Light);
        assert_eq!(
            light
                .on_glass(None)
                .colors
                .get(ColorRole::ControlText, selected),
            light.colors.get(ColorRole::ControlText, selected)
        );
    }
}
