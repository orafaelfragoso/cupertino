//! A colour table's crossing from a window's inactive colours to its active ones.

use cupertino_theme::ui::{ColorRole, ColorTokens, StateVariant};
use cupertino_theme::Color;

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
