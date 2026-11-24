//! The glass the launcher's panel names: the search results' glass, with the compositor's soft shadow (no modifier),
//! for the plain search bar and the mode buttons beside it at the large size and for a panel holding results below its
//! field at its own size. None adapts to what is behind it; the legacy large panel style stays what a window system
//! without variants draws.

use cupertino_kit::core::{GlassModifiers, GlassSize, GlassVariant};
use cupertino_kit::platform::GlassStyle;
use cupertino_launcher::motion::Motion;
use cupertino_launcher::view::layers::{button_glass_look, glass_look};
use cupertino_launcher::view::{
    compact_shape, enclosure_variant, BUTTON_VARIANT, COLLAPSED_HEIGHT, CORNER_RADIUS, WIDTH,
};

/// The height of a panel that shows results below its field.
const RESULTS_HEIGHT: f64 = 400.0;

/// `variant` at `size`, with no other modifier.
fn named(variant: GlassVariant, size: GlassSize) -> Option<(GlassVariant, GlassModifiers)> {
    Some((variant, GlassModifiers::DEFAULT.sized(size)))
}

#[test]
fn the_plain_search_bar_is_the_search_results_glass_at_the_large_size() {
    let frame = Motion::new(compact_shape()).frame(0.0);
    assert_eq!(frame.shape.height, COLLAPSED_HEIGHT);
    assert_eq!(
        enclosure_variant(&frame),
        (GlassVariant::SearchResults, GlassSize::Large)
    );
    let look = glass_look(&frame);
    assert_eq!(look.glass, Some(GlassStyle::LargePanel));
    assert_eq!(
        look.variant,
        named(GlassVariant::SearchResults, GlassSize::Large),
        "the milky sheet with its shadow, not clear regular glass without one"
    );
}

#[test]
fn a_panel_with_results_is_the_search_results_glass_at_its_own_size() {
    let shape = cupertino_launcher::motion::Shape::new(WIDTH, RESULTS_HEIGHT, CORNER_RADIUS);
    let frame = Motion::new(shape).frame(0.0);
    assert_eq!(
        enclosure_variant(&frame),
        (GlassVariant::SearchResults, GlassSize::Dynamic)
    );
    let look = glass_look(&frame);
    assert_eq!(look.glass, Some(GlassStyle::LargePanel));
    assert_eq!(
        look.variant,
        named(GlassVariant::SearchResults, GlassSize::Dynamic),
        "the compositor draws its shadow: none is left out"
    );
}

#[test]
fn a_mode_button_is_the_search_bars_glass() {
    assert_eq!(
        BUTTON_VARIANT,
        (GlassVariant::SearchResults, GlassSize::Large)
    );
    let look = button_glass_look(44.0);
    assert_eq!(look.glass, Some(GlassStyle::LargePanel));
    assert_eq!(
        look.variant,
        named(GlassVariant::SearchResults, GlassSize::Large)
    );
}
