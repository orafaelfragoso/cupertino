//! The person's accent and highlight colours laid over a palette: the fills that mark a default choice or a selection
//! take the accent, and the background of selected text takes a light tint of the highlight.

use crate::{Color, ColorRole, ColorTokens, InteractionVariant, StateVariant};

/// How much of the colour a pressed accent keeps: a little darker than the resting one.
const PRESSED_KEEP: f32 = 0.86;
/// How much of the accent a disabled selected control keeps, the rest being the surface it sits on.
const DISABLED_KEEP: f32 = 0.4;
/// How much of the colour the selected text's background keeps, the rest being a neutral light (or dark) base.
const SELECTION_KEEP: f32 = 0.3;
/// The neutral base of the selection tint in the light appearance (white at 70 %) and the dark one (a grey level).
const SELECTION_BASE_LIGHT: f32 = 0.7;
const SELECTION_BASE_DARK: f32 = 63.0 / 255.0;
/// The surface a disabled selected control fades towards in the light and the dark appearance.
const SURFACE_LIGHT: f32 = 1.0;
const SURFACE_DARK: f32 = 30.0 / 255.0;

fn rgb(c: [u8; 3]) -> Color {
    Color::from_rgba8([c[0], c[1], c[2], 255])
}

fn scaled(c: Color, keep: f32) -> Color {
    Color::new(c.r * keep, c.g * keep, c.b * keep, c.a)
}

fn faded(c: Color, keep: f32, surface: f32) -> Color {
    let mix = |v: f32| v * keep + surface * (1.0 - keep);
    Color::new(mix(c.r), mix(c.g), mix(c.b), c.a)
}

/// The background of text selected in `color`, in the light or the dark appearance.
pub fn selection_tint(color: [u8; 3], dark: bool) -> Color {
    let base = if dark {
        SELECTION_BASE_DARK
    } else {
        SELECTION_BASE_LIGHT
    };
    let c = rgb(color);
    let mix = |v: f32| v * SELECTION_KEEP + base;
    Color::new(mix(c.r), mix(c.g), mix(c.b), 1.0)
}

impl ColorTokens {
    /// The same table with the accent `accent` in place of the theme's own wherever the theme marks a default choice,
    /// a selected control or an on switch, in the state's own shade, and with the background of selected text a tint
    /// of `highlight`. `dark` says which appearance the table is for.
    pub fn with_accent(&self, accent: [u8; 3], highlight: [u8; 3], dark: bool) -> Self {
        let state = |interaction, selected| StateVariant {
            interaction,
            selected,
            window_active: true,
        };
        let (Some(rest), Some(pressed), Some(disabled)) = (
            self.get(ColorRole::SelectedControlFill, StateVariant::NORMAL),
            self.get(
                ColorRole::DefaultButtonFill,
                state(InteractionVariant::Pressed, false),
            ),
            self.get(
                ColorRole::SwitchTrack,
                state(InteractionVariant::Disabled, true),
            ),
        ) else {
            return self.clone();
        };
        let accent = rgb(accent);
        let surface = if dark { SURFACE_DARK } else { SURFACE_LIGHT };
        let tint = selection_tint(highlight, dark);
        self.map(|role, _, color| {
            if role == ColorRole::SelectedTextBackground {
                tint
            } else if color == rest {
                accent
            } else if color == pressed {
                scaled(accent, PRESSED_KEEP)
            } else if color == disabled {
                faded(accent, DISABLED_KEEP, surface)
            } else {
                color
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accent_replaces_the_selected_fill_in_every_state_that_shows_it() {
        let colors = ColorTokens::neutral(crate::Appearance::Light);
        let red = [224, 56, 62];
        let accented = colors.with_accent(red, red, false);
        let normal = StateVariant::NORMAL;
        assert_eq!(
            accented.get(ColorRole::DefaultButtonFill, normal),
            Some(rgb(red))
        );
        assert_eq!(
            accented.get(ColorRole::SelectedControlFill, normal),
            Some(rgb(red))
        );
    }
}
