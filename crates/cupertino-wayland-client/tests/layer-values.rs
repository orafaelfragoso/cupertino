//! The value types of the layers protocol: property ranges, versions and wire fixed point.

use cupertino_wayland_client::app::{Fixed, LayerProperty, MAX_CONTENT_BLUR};

#[test]
fn each_property_accepts_only_finite_values_in_its_range() {
    use LayerProperty::*;
    let mut rows = Vec::new();
    for property in [X, Y] {
        rows.extend([
            (property, -1e9, true),
            (property, 0.0, true),
            (property, 1e9, true),
        ]);
    }
    rows.extend([
        (Opacity, 0.0, true),
        (Opacity, 0.5, true),
        (Opacity, 1.0, true),
        (Opacity, -f64::EPSILON, false),
        (Opacity, 1.0 + f64::EPSILON, false),
    ]);
    for property in [Width, Height, Scale, CornerRadius] {
        rows.extend([
            (property, 0.0, true),
            (property, 1e6, true),
            (property, -0.001, false),
        ]);
    }
    rows.extend([
        (ContentBlur, 0.0, true),
        (ContentBlur, MAX_CONTENT_BLUR, true),
        (ContentBlur, MAX_CONTENT_BLUR + 0.001, false),
        (ContentBlur, -0.001, false),
    ]);
    for property in [
        X,
        Y,
        Width,
        Height,
        Opacity,
        Scale,
        CornerRadius,
        ContentBlur,
    ] {
        rows.extend([
            (property, f64::NAN, false),
            (property, f64::INFINITY, false),
            (property, f64::NEG_INFINITY, false),
        ]);
    }
    for (property, value, accepted) in rows {
        assert_eq!(property.accepts(value), accepted, "{property:?} {value}");
    }
}

#[test]
fn only_content_blur_needs_a_newer_protocol_and_a_presentation_moves_offset_opacity_and_scale() {
    use LayerProperty::*;
    for (property, since, presented) in [
        (X, 1, true),
        (Y, 1, true),
        (Width, 1, false),
        (Height, 1, false),
        (Opacity, 1, true),
        (Scale, 1, true),
        (CornerRadius, 1, false),
        (ContentBlur, 4, false),
    ] {
        assert_eq!(property.since(), since, "{property:?}");
        assert_eq!(property.presented(), presented, "{property:?}");
    }
}

#[test]
fn fixed_point_is_in_256ths_rounded_to_the_nearest_step_and_saturating() {
    for (row, value, fixed) in [
        ("one", 1.0, 256),
        ("minus one and a half", -1.5, -384),
        ("zero", 0.0, 0),
        ("just over half a step", 1.0 / 512.0 + 1e-9, 1),
        ("just under half a step", 1.0 / 512.0 - 1e-9, 0),
        ("just over half a step down", -1.0 / 512.0 - 1e-9, -1),
        ("infinity", f64::INFINITY, i32::MAX),
        ("minus infinity", f64::NEG_INFINITY, i32::MIN),
        ("too large", 1e30, i32::MAX),
        ("NaN", f64::NAN, 0),
    ] {
        assert_eq!(Fixed::from_f64(value), Fixed(fixed), "{row}");
    }
    assert_eq!(Fixed(64).to_f64(), 0.25);
    for raw in [-8_388_608, -257, -1, 0, 1, 255, 256, 8_388_607] {
        assert_eq!(
            Fixed::from_f64(Fixed(raw).to_f64()),
            Fixed(raw),
            "{raw} round trips"
        );
    }
}
