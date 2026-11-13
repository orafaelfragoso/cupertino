//! Text and symbols on glass. The compositor draws glass under a part of a window, and the window draws its text over
//! it; that text goes through the foreground matrix of the glass (`cupertino_theme::glass`): the face under it lifted
//! to white on dark glass, pushed to black on light glass. A small shape takes the matrix of the side its face was
//! measured on, so a control over a light backdrop in the dark appearance gets dark text; larger bars and panels keep
//! their appearance's. Labels on glass are also drawn more opaque than on a plain window, since the face behind them
//! is busier: a primary label at full strength, a placeholder somewhat less, and a secondary label at the strength
//! that shifts the face under it by the secondary shift of its side (darker on light glass, lighter on dark glass).

use crate::{Color, ColorRole, InteractionVariant, ThemeTokens};
use cupertino_theme::glass::{foreground_rgb, FaceTone};
pub use cupertino_theme::glass::{vibrant_secondary, VIBRANT_PLACEHOLDER, VIBRANT_PRIMARY};

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
            // the secondary label and the placeholder stay below the primary label
            let secondary = get(&large, ColorRole::SecondaryText).a;
            let placeholder = get(&large, ColorRole::PlaceholderText).a;
            assert!(secondary >= vibrant_secondary(dark) && placeholder >= VIBRANT_PLACEHOLDER);
            assert!(placeholder <= VIBRANT_PRIMARY && secondary <= VIBRANT_PRIMARY);
        }
    }

    #[test]
    fn a_secondary_label_shifts_the_label_face_by_its_sides_shift() {
        use cupertino_theme::glass::{
            LABEL_FACE_DARK, LABEL_FACE_LIGHT, SECONDARY_SHIFT_DARK, SECONDARY_SHIFT_LIGHT,
        };
        // light: black over the light label face at the strength lowers it by 0.4
        let light = vibrant_secondary(false);
        assert!(
            (LABEL_FACE_LIGHT * (1.0 - light) - (LABEL_FACE_LIGHT + SECONDARY_SHIFT_LIGHT)).abs()
                < 1e-5
        );
        // dark: white over the dark label face at the strength raises it by 0.3
        let dark = vibrant_secondary(true);
        let over = LABEL_FACE_DARK + dark * (1.0 - LABEL_FACE_DARK);
        assert!((over - (LABEL_FACE_DARK + SECONDARY_SHIFT_DARK)).abs() < 1e-5);
        assert!(
            (0.5..0.6).contains(&light) && (0.5..0.6).contains(&dark),
            "{light} {dark}"
        );
        // a small shape's secondary label takes the strength of the side its face was measured on
        let t = theme(Appearance::Dark);
        let over_light = get(&t.on_glass(Some(FaceTone::Light)), ColorRole::SecondaryText);
        let over_dark = get(&t.on_glass(Some(FaceTone::Dark)), ColorRole::SecondaryText);
        assert!(over_light.a >= light && over_dark.a >= dark);
        assert_eq!([over_light.r, over_dark.r], [0.0, 1.0]);
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

    #[test]
    fn grey_labels_of_the_high_contrast_themes_go_through_the_matrix_as_strongly_as_they_were() {
        for appearance in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
            let t = theme(appearance);
            let dark = appearance.is_dark();
            let glass = t.on_glass(None);
            let fg = glass_foreground(dark, None);
            for role in [
                ColorRole::Text,
                ColorRole::SecondaryText,
                ColorRole::PlaceholderText,
            ] {
                let before = get(&t, role);
                let after = get(&glass, role);
                let strength = label_strength(before, dark).expect("a label colour");
                assert_eq!(
                    [after.r, after.g, after.b],
                    [fg.r, fg.g, fg.b],
                    "{appearance:?} {role:?}"
                );
                assert!(
                    after.a >= strength && after.a >= vibrant_level(role, dark).unwrap(),
                    "{appearance:?} {role:?}: {before:?} became {after:?}"
                );
            }
        }
        // a colour that is not grey is not a label's
        assert_eq!(label_strength(Color::new(0.2, 0.5, 1.0, 1.0), true), None);
        // an opaque light grey in the dark appearance is that far towards white
        let grey = label_strength(Color::new(0.6, 0.6, 0.6, 1.0), true).unwrap();
        assert!((grey - 0.6).abs() < 1e-6);
    }
}
