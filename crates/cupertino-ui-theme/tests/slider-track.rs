//! The empty part of a slider's track stands 3:1 or more against the surface it sits on, in light and dark, in the
//! default palette, with Increase Contrast and with Reduce Transparency, in every enabled state. Checked on the colours
//! a theme resolves, not on the palette tables.

use cupertino_theme::Theme;
use cupertino_ui_theme::{
    Appearance, Color, ColorRole, InteractionVariant, StateVariant, ThemePreferences, ThemeTokens,
};

/// The least contrast of a slider's empty track with the surface under it.
const TRACK: f32 = 3.0;

/// The WCAG contrast ratio of two opaque colours.
fn contrast(a: Color, b: Color) -> f32 {
    let luminance = |c: Color| {
        let lin = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    };
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `top` drawn over the opaque `bottom`.
fn over(top: Color, bottom: Color) -> Color {
    let mix = |t: f32, b: f32| t * top.a + b * (1.0 - top.a);
    Color {
        r: mix(top.r, bottom.r),
        g: mix(top.g, bottom.g),
        b: mix(top.b, bottom.b),
        a: 1.0,
    }
}

/// Every theme a session can show: light and dark, each as is, with Increase Contrast, with Reduce Transparency and
/// with both.
fn themes() -> Vec<(String, ThemeTokens)> {
    let theme = Theme::load().unwrap();
    let mut themes = Vec::new();
    for appearance in Appearance::ALL {
        for reduce_transparency in [false, true] {
            let preferences = ThemePreferences {
                appearance,
                reduce_transparency,
                reduce_motion: false,
            };
            themes.push((
                format!("{appearance:?}, reduce transparency {reduce_transparency}"),
                ThemeTokens::cupertino(&theme, preferences).unwrap(),
            ));
        }
    }
    themes
}

/// The states an enabled slider's empty track is drawn in: at rest, hovered and dragged, in an active window or not.
fn enabled() -> impl Iterator<Item = StateVariant> {
    StateVariant::ALL
        .into_iter()
        .filter(|s| s.interaction != InteractionVariant::Disabled && !s.selected)
}

#[test]
fn every_appearance_is_checked() {
    let names: Vec<String> = themes().into_iter().map(|(name, _)| name).collect();
    for wanted in [
        "Light, reduce transparency false",
        "Dark, reduce transparency false",
        "LightHighContrast, reduce transparency false",
        "DarkHighContrast, reduce transparency false",
        "Light, reduce transparency true",
        "Dark, reduce transparency true",
    ] {
        assert!(names.iter().any(|n| n == wanted), "{wanted} in {names:?}");
    }
}

/// A form's slider, as in Settings, sits on the window; a field's text background may sit under one too.
#[test]
fn a_slider_track_stands_three_to_one_against_the_window() {
    let black = Color::from_rgba8([0, 0, 0, 255]);
    for (name, theme) in themes() {
        let normal = |role| theme.colors.get(role, StateVariant::NORMAL).unwrap();
        for surface in [ColorRole::WindowBackground, ColorRole::TextBackground] {
            let under = over(normal(surface), black);
            for state in enabled() {
                let track = theme.colors.get(ColorRole::SliderTrack, state).unwrap();
                let track = over(track, under);
                let ratio = contrast(track, under);
                assert!(
                    ratio >= TRACK,
                    "{name}, {state:?}: track {track:?} on {surface:?} {under:?} is {ratio:.2}:1"
                );
            }
        }
        // the knob stays apart from the track it slides on
        let knob = contrast(
            normal(ColorRole::SwitchThumb),
            normal(ColorRole::SliderTrack),
        );
        assert!(knob >= TRACK, "{name}: knob on track {knob:.2}:1");
    }
}

/// A panel's level slider, as in Control Center, sits in a module: the group fill over the panel, whose opaque colour
/// is the window background (the material it asks for blurs what is behind it instead).
#[test]
fn a_panel_slider_track_stands_three_to_one_against_its_module() {
    let black = Color::from_rgba8([0, 0, 0, 255]);
    for (name, theme) in themes() {
        let normal = |role| theme.colors.get(role, StateVariant::NORMAL).unwrap();
        let panel = over(normal(ColorRole::WindowBackground), black);
        let module = over(normal(ColorRole::GroupFill), panel);
        for state in enabled() {
            let track = over(
                theme.colors.get(ColorRole::PanelTrack, state).unwrap(),
                module,
            );
            let ratio = contrast(track, module);
            assert!(
                ratio >= TRACK,
                "{name}, {state:?}: panel track {track:?} on module {module:?} is {ratio:.2}:1"
            );
        }
        // the fill, in the thumb's colour, stays apart from the empty part
        let fill = contrast(
            normal(ColorRole::SwitchThumb),
            normal(ColorRole::PanelTrack),
        );
        assert!(fill >= TRACK, "{name}: fill on panel track {fill:.2}:1");
    }
}

/// A disabled panel track has its own colour: still apart from its module, but fainter than the enabled track.
#[test]
fn a_disabled_panel_track_is_fainter_than_the_enabled_one() {
    let black = Color::from_rgba8([0, 0, 0, 255]);
    for (name, theme) in themes() {
        let normal = |role| theme.colors.get(role, StateVariant::NORMAL).unwrap();
        let module = over(
            normal(ColorRole::GroupFill),
            over(normal(ColorRole::WindowBackground), black),
        );
        let disabled = StateVariant {
            interaction: InteractionVariant::Disabled,
            ..StateVariant::NORMAL
        };
        let off = theme.colors.get(ColorRole::PanelTrack, disabled).unwrap();
        assert_ne!(off, normal(ColorRole::PanelTrack), "{name}");
        let faint = contrast(over(off, module), module);
        let full = contrast(over(normal(ColorRole::PanelTrack), module), module);
        assert!(
            faint > 1.1 && faint < full,
            "{name}: disabled panel track {faint:.2}:1, enabled {full:.2}:1"
        );
    }
}
