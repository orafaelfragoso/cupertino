//! The value types of the layers protocol: property ranges, versions, wire fixed point and the error texts.

use cupertino_wayland_client::app::{
    Error, Fixed, LayerId, LayerProperty, MAX_CONTENT_BLUR, MAX_GLASS_EDGE, MAX_GROUP_SPACING,
    MAX_SAMPLES,
};

const ALL: [LayerProperty; 8] = [
    LayerProperty::X,
    LayerProperty::Y,
    LayerProperty::Width,
    LayerProperty::Height,
    LayerProperty::Opacity,
    LayerProperty::Scale,
    LayerProperty::CornerRadius,
    LayerProperty::ContentBlur,
];

#[test]
fn positions_take_any_finite_value() {
    for property in [LayerProperty::X, LayerProperty::Y] {
        assert!(property.accepts(-1e9));
        assert!(property.accepts(0.0));
        assert!(property.accepts(1e9));
        assert!(!property.accepts(f64::NAN));
        assert!(!property.accepts(f64::INFINITY));
        assert!(!property.accepts(f64::NEG_INFINITY));
    }
}

#[test]
fn opacity_is_a_unit_interval() {
    let opacity = LayerProperty::Opacity;
    assert!(opacity.accepts(0.0));
    assert!(opacity.accepts(0.5));
    assert!(opacity.accepts(1.0));
    assert!(!opacity.accepts(-f64::EPSILON));
    assert!(!opacity.accepts(1.0 + f64::EPSILON));
    assert!(!opacity.accepts(f64::NAN));
    assert!(!opacity.accepts(f64::INFINITY));
}

#[test]
fn extents_scale_and_radius_are_not_negative() {
    for property in [
        LayerProperty::Width,
        LayerProperty::Height,
        LayerProperty::Scale,
        LayerProperty::CornerRadius,
    ] {
        assert!(property.accepts(0.0));
        assert!(property.accepts(1e6));
        assert!(!property.accepts(-0.001));
        assert!(!property.accepts(f64::NAN));
        assert!(!property.accepts(f64::INFINITY));
    }
}

#[test]
fn content_blur_stops_at_its_maximum() {
    let blur = LayerProperty::ContentBlur;
    assert_eq!(MAX_CONTENT_BLUR, 64.0);
    assert!(blur.accepts(0.0));
    assert!(blur.accepts(MAX_CONTENT_BLUR));
    assert!(!blur.accepts(MAX_CONTENT_BLUR + 0.001));
    assert!(!blur.accepts(-0.001));
    assert!(!blur.accepts(f64::NAN));
    assert!(!blur.accepts(f64::INFINITY));
}

#[test]
fn only_content_blur_needs_a_newer_protocol() {
    for property in ALL {
        let expected = if property == LayerProperty::ContentBlur {
            4
        } else {
            1
        };
        assert_eq!(property.since(), expected, "{property:?}");
    }
}

#[test]
fn a_presentation_has_offset_opacity_and_scale_only() {
    let presented: Vec<_> = ALL.into_iter().filter(|p| p.presented()).collect();
    assert_eq!(
        presented,
        [
            LayerProperty::X,
            LayerProperty::Y,
            LayerProperty::Opacity,
            LayerProperty::Scale
        ]
    );
}

#[test]
fn limits_keep_their_documented_values() {
    assert_eq!(MAX_GLASS_EDGE, 4.0);
    assert_eq!(MAX_GROUP_SPACING, 64.0);
    assert_eq!(MAX_SAMPLES, 1000);
}

#[test]
fn fixed_point_is_in_256ths() {
    assert_eq!(Fixed::from_f64(1.0), Fixed(256));
    assert_eq!(Fixed::from_f64(-1.5), Fixed(-384));
    assert_eq!(Fixed::from_f64(0.0), Fixed(0));
    assert_eq!(Fixed(64).to_f64(), 0.25);
    assert_eq!(Fixed::default(), Fixed(0));
}

#[test]
fn fixed_point_rounds_to_the_nearest_step() {
    assert_eq!(Fixed::from_f64(1.0 / 512.0 + 1e-9), Fixed(1));
    assert_eq!(Fixed::from_f64(1.0 / 512.0 - 1e-9), Fixed(0));
    assert_eq!(Fixed::from_f64(-1.0 / 512.0 - 1e-9), Fixed(-1));
}

#[test]
fn fixed_point_round_trips_every_step_it_holds() {
    for raw in [-8_388_608, -257, -1, 0, 1, 255, 256, 8_388_607] {
        assert_eq!(Fixed::from_f64(Fixed(raw).to_f64()), Fixed(raw));
    }
}

#[test]
fn fixed_point_saturates_and_maps_nan_to_zero() {
    assert_eq!(Fixed::from_f64(f64::INFINITY), Fixed(i32::MAX));
    assert_eq!(Fixed::from_f64(f64::NEG_INFINITY), Fixed(i32::MIN));
    assert_eq!(Fixed::from_f64(1e30), Fixed(i32::MAX));
    assert_eq!(Fixed::from_f64(f64::NAN), Fixed(0));
}

#[test]
fn layer_ids_compare_by_number() {
    assert_eq!(LayerId(3), LayerId(3));
    assert_ne!(LayerId(3), LayerId(4));
}

#[test]
fn each_error_says_what_was_refused() {
    let texts = [
        (
            Error::InvalidShape,
            "backdrop shape has a negative extent or radius, or is not finite",
        ),
        (Error::InvalidAlpha, "backdrop alpha is outside 0 to 1"),
        (
            Error::InvalidGlass,
            "glass spacing is negative or not finite",
        ),
        (
            Error::InvalidLayerValue,
            "layer value is outside its property's range",
        ),
        (
            Error::Unsupported("cupertino_layer_v1.set_glass_edge"),
            "the server does not take cupertino_layer_v1.set_glass_edge",
        ),
    ];
    for (error, text) in texts {
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn the_error_is_a_std_error() {
    fn takes(_: &dyn std::error::Error) {}
    takes(&Error::InvalidAlpha);
}
