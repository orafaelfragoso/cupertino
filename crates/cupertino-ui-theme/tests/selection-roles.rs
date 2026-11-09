//! How the selection roles derive from the theme's accent and from the sidebar's material, checked on the colours a
//! theme resolves. The selected fills every control draws (a sidebar's selected row, a list, table or outline row, a
//! menu's highlighted item, the default button, an on switch) are the accent token itself: no role lightens, fades,
//! saturates or tints it, on a window or on glass. A sidebar's neutral selection is a grey laid over the material, so
//! it lifts the material's channels evenly and adds no hue of its own.

use cupertino_theme::Theme;
use cupertino_ui_theme::{
    Appearance, Color, ColorRole, FaceTone, InteractionVariant, StateVariant, ThemePreferences,
    ThemeTokens,
};

fn tokens(appearance: Appearance) -> ThemeTokens {
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::load().unwrap(), preferences).unwrap()
}

fn state(interaction: InteractionVariant, selected: bool, window_active: bool) -> StateVariant {
    StateVariant {
        interaction,
        selected,
        window_active,
    }
}

/// `top` drawn over the opaque `bottom`.
fn over(top: Color, bottom: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = bottom;
    [(top.r, r), (top.g, g), (top.b, b)].map(|(t, b)| t * top.a + b * (1.0 - top.a))
}

#[test]
fn every_selected_fill_is_the_accent_token_unprocessed() {
    let theme = Theme::load().unwrap();
    for appearance in [Appearance::Light, Appearance::Dark] {
        let accent = theme
            .tokens
            .color(appearance, "control-accent-blue")
            .expect("the accent token");
        let t = tokens(appearance);
        let faces = [None, Some(FaceTone::Light), Some(FaceTone::Dark)];
        for drawn in std::iter::once(t.clone()).chain(faces.map(|face| t.on_glass(face))) {
            let get = |role, s| drawn.colors.get(role, s).unwrap();
            for interaction in [
                InteractionVariant::Normal,
                InteractionVariant::Hover,
                InteractionVariant::Pressed,
            ] {
                // a selected row of a sidebar, a list, a table or an outline in an active window
                assert_eq!(
                    get(ColorRole::ControlFill, state(interaction, true, true)),
                    accent,
                    "{appearance:?} {interaction:?}"
                );
            }
            let normal = StateVariant::NORMAL;
            // a menu's highlighted item, the default button and an on switch
            assert_eq!(get(ColorRole::SelectedControlFill, normal), accent);
            assert_eq!(get(ColorRole::DefaultButtonFill, normal), accent);
            assert_eq!(get(ColorRole::SwitchTrack, state(InteractionVariant::Normal, true, false)), accent);
            // the selected text on it is opaque white: nothing lets the accent show through it differently
            let text = get(ColorRole::SelectedControlText, normal);
            assert_eq!((text.r, text.g, text.b, text.a), (1.0, 1.0, 1.0, 1.0));
        }
        assert_eq!(accent.to_rgba8(), [52, 120, 246, 255], "{appearance:?}");
    }
}

/// The neutral selection of a sidebar lifts each channel of any material under it by the same amount (within a
/// rounding step), so the selected row keeps the material's hue and only grows lighter (dark) or darker (light);
/// fainter in a window that is not active.
#[test]
fn a_sidebar_s_neutral_selection_adds_no_hue_to_the_material() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let t = tokens(appearance);
        let active = t.colors.get(ColorRole::SidebarSelection, StateVariant::NORMAL).unwrap();
        let inactive = t
            .colors
            .get(
                ColorRole::SidebarSelection,
                state(InteractionVariant::Normal, true, false),
            )
            .unwrap();
        assert!(active.r == active.g && active.g == active.b, "{appearance:?}: a grey");
        assert!(inactive.r == inactive.g && inactive.g == inactive.b, "{appearance:?}: a grey");
        assert!(inactive.a < active.a, "{appearance:?}: fainter when inactive");
        // a navy material, as a sidebar over a blue wallpaper is
        let material = [34.0 / 255.0, 39.0 / 255.0, 56.0 / 255.0];
        let lifted = over(active, material);
        let steps: Vec<f32> = lifted
            .iter()
            .zip(material)
            .map(|(l, m)| (l - m) * 255.0)
            .collect();
        let (min, max) = steps.iter().fold((f32::MAX, f32::MIN), |(a, b), &s| (a.min(s), b.max(s)));
        assert!(max - min <= 3.0, "{appearance:?}: even channels {steps:?}");
        if appearance.is_dark() {
            // the selected row of a place lifts the material by about 29 of 255
            assert!((26.0..=31.0).contains(&min), "{steps:?}");
            let lifted = over(inactive, material);
            let step = (lifted[0] - material[0]) * 255.0;
            assert!((12.0..=16.0).contains(&step), "inactive {step}");
        }
    }
}
