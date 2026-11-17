//! A colour table's crossing from a window's inactive colours to its active ones.

use cupertino_theme::ui::{
    ColorRole, ColorTokens, FaceTone, StateVariant, ThemePreferences, ThemeTokens,
};
use cupertino_theme::{Appearance, Color, Theme};

const TOKENS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/theme/tokens.json"
));
const MATERIALS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/theme/materials.json"
));
const ANIMATIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/theme/animations.json"
));

const PROGRESS: [f32; 3] = [0.0, 0.5, 1.0];

fn inactive() -> StateVariant {
    StateVariant {
        window_active: false,
        ..StateVariant::NORMAL
    }
}

fn table() -> ColorTokens {
    ColorTokens::new([
        (
            (ColorRole::Text, StateVariant::NORMAL),
            Color::new(1.0, 1.0, 1.0, 1.0),
        ),
        (
            (ColorRole::Text, inactive()),
            Color::new(0.0, 0.0, 0.0, 1.0),
        ),
    ])
}

#[test]
fn a_table_shows_the_active_colours_until_told_otherwise() {
    let colors = table();
    assert_eq!(
        colors.get(ColorRole::Text, StateVariant::NORMAL),
        Some(Color::new(1.0, 1.0, 1.0, 1.0))
    );
    assert_eq!(ColorTokens::default(), ColorTokens::new([]));
}

#[test]
fn an_active_state_blends_from_the_inactive_one() {
    let mut colors = table();
    colors.set_activation(0.25);
    let shown = colors.get(ColorRole::Text, StateVariant::NORMAL).unwrap();
    assert!((shown.r - 0.25).abs() < 1e-6, "{shown:?}");
    assert_eq!(
        colors.get(ColorRole::Text, inactive()),
        Some(Color::new(0.0, 0.0, 0.0, 1.0)),
        "an inactive state does not cross"
    );
}

#[test]
fn the_progress_is_clamped_and_kept_by_map() {
    let mut colors = table();
    colors.set_activation(-1.0);
    assert_eq!(
        colors.get(ColorRole::Text, StateVariant::NORMAL),
        Some(Color::new(0.0, 0.0, 0.0, 1.0))
    );
    colors.set_activation(0.5);
    let mapped = colors.map(|_, _, c| c);
    let shown = mapped.get(ColorRole::Text, StateVariant::NORMAL).unwrap();
    assert!((shown.r - 0.5).abs() < 1e-6, "{shown:?}");
}

/// The window themes of every appearance, from the theme data and without it.
fn themes() -> Vec<ThemeTokens> {
    let theme = Theme::from_strs(TOKENS, MATERIALS, ANIMATIONS).unwrap();
    Appearance::ALL
        .into_iter()
        .flat_map(|appearance| {
            let preferences = ThemePreferences {
                appearance,
                reduce_transparency: false,
                reduce_motion: false,
            };
            [
                ThemeTokens::cupertino(&theme, preferences).unwrap(),
                ThemeTokens::neutral(preferences).unwrap(),
            ]
        })
        .collect()
}

/// Every colour of `colors`, in a fixed order.
fn every_colour(colors: &ColorTokens) -> Vec<Option<Color>> {
    ColorRole::ALL
        .into_iter()
        .flat_map(|role| StateVariant::ALL.map(|state| colors.get(role, state)))
        .collect()
}

#[test]
fn a_crossed_copy_shows_the_colours_of_a_table_rebuilt_at_that_progress() {
    for theme in themes() {
        for progress in PROGRESS {
            // the copy a crossing window paints with, which shares its table with the window's theme
            let mut crossed = theme.colors.clone();
            crossed.set_activation(progress);
            // a table of its own, rebuilt from every colour, at the same progress
            let mut rebuilt = theme.colors.map(|_, _, c| c);
            rebuilt.set_activation(progress);
            assert_eq!(
                every_colour(&crossed),
                every_colour(&rebuilt),
                "{:?} at {progress}",
                theme.preferences.appearance
            );
            for role in ColorRole::ALL {
                for state in StateVariant::ALL {
                    // at 1 the active colours themselves, not a blend that may round them
                    let expected = if state.window_active && progress < 1.0 {
                        let inactive = StateVariant {
                            window_active: false,
                            ..state
                        };
                        let before = theme.colors.get(role, inactive);
                        let after = theme.colors.get(role, state);
                        before.zip(after).map(|(b, a)| b.lerp(a, progress))
                    } else {
                        theme.colors.get(role, state)
                    };
                    assert_eq!(
                        crossed.get(role, state),
                        expected,
                        "{role:?} {state:?} at {progress}"
                    );
                }
            }
        }
    }
}

#[test]
fn crossing_a_copy_leaves_the_theme_it_shares_a_table_with_alone() {
    for theme in themes() {
        let before = every_colour(&theme.colors);
        let mut crossed = theme.colors.clone();
        crossed.set_activation(0.0);
        assert_eq!(every_colour(&theme.colors), before);
        assert_eq!(theme.colors, theme.colors.map(|_, _, c| c));
    }
}

#[test]
fn text_on_glass_crosses_the_same_before_or_after_it_is_made() {
    for theme in themes() {
        for tone in [None, Some(FaceTone::Dark), Some(FaceTone::Light)] {
            let on_glass = theme.on_glass(tone);
            for progress in PROGRESS {
                let mut crossed_first = theme.clone();
                crossed_first.colors.set_activation(progress);
                let mut crossed_after = on_glass.clone();
                crossed_after.colors.set_activation(progress);
                assert_eq!(
                    every_colour(&crossed_first.on_glass(tone).colors),
                    every_colour(&crossed_after.colors),
                    "{:?} {tone:?} at {progress}",
                    theme.preferences.appearance
                );
            }
        }
    }
}
