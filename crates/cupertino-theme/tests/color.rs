//! The colour value type, appearances and evidence classes.

use cupertino_theme::{Appearance, Color, Evidence};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn eight_bit_channels_round_trip() {
    for c in [[0, 0, 0, 0], [255, 255, 255, 255], [12, 200, 77, 128]] {
        assert_eq!(Color::from_rgba8(c).to_rgba8(), c);
    }
}

#[test]
fn eight_bit_channels_map_to_the_unit_range() {
    let c = Color::from_rgba8([255, 0, 51, 255]);
    assert!(close(c.r, 1.0) && close(c.g, 0.0) && close(c.b, 0.2) && close(c.a, 1.0));
}

#[test]
fn converting_to_eight_bits_clamps() {
    assert_eq!(
        Color::new(2.0, -1.0, 0.5, 1.5).to_rgba8(),
        [255, 0, 128, 255]
    );
}

#[test]
fn the_hex_form_drops_alpha() {
    assert_eq!(Color::from_rgba8([255, 0, 171, 10]).hex(), "#FF00AB");
}

#[test]
fn premultiplying_scales_the_channels_by_alpha() {
    let p = Color::new(1.0, 0.5, 0.0, 0.5).premultiplied();
    assert_eq!(p, [0.5, 0.25, 0.0, 0.5]);
}

#[test]
fn with_alpha_changes_only_alpha() {
    let c = Color::new(0.1, 0.2, 0.3, 1.0).with_alpha(0.4);
    assert_eq!(c.to_array(), [0.1, 0.2, 0.3, 0.4]);
}

#[test]
fn lerp_runs_from_one_colour_to_the_other() {
    let a = Color::new(0.0, 0.0, 0.0, 0.0);
    let b = Color::new(1.0, 0.5, 0.2, 1.0);
    assert_eq!(a.lerp(b, 0.0), a);
    assert_eq!(a.lerp(b, 1.0), b);
    let m = a.lerp(b, 0.5);
    assert!(close(m.r, 0.5) && close(m.g, 0.25) && close(m.b, 0.1) && close(m.a, 0.5));
}

#[test]
fn there_are_four_appearances() {
    assert_eq!(Appearance::ALL.len(), 4);
}

#[test]
fn dark_and_high_contrast_are_told_apart() {
    assert!(!Appearance::Light.is_dark() && !Appearance::Light.is_high_contrast());
    assert!(Appearance::Dark.is_dark() && !Appearance::Dark.is_high_contrast());
    assert!(
        !Appearance::LightHighContrast.is_dark()
            && Appearance::LightHighContrast.is_high_contrast()
    );
    assert!(
        Appearance::DarkHighContrast.is_dark() && Appearance::DarkHighContrast.is_high_contrast()
    );
}

#[test]
fn the_base_appearance_drops_high_contrast() {
    assert_eq!(Appearance::LightHighContrast.base(), Appearance::Light);
    assert_eq!(Appearance::DarkHighContrast.base(), Appearance::Dark);
    assert_eq!(Appearance::Dark.base(), Appearance::Dark);
}

#[test]
fn a_session_picks_its_catalog_from_dark_and_contrast() {
    assert_eq!(Appearance::of_session(false, false), Appearance::Light);
    assert_eq!(Appearance::of_session(true, false), Appearance::Dark);
    assert_eq!(
        Appearance::of_session(false, true),
        Appearance::LightHighContrast
    );
    assert_eq!(
        Appearance::of_session(true, true),
        Appearance::DarkHighContrast
    );
}

#[test]
fn contrast_switches_without_changing_darkness() {
    for a in Appearance::ALL {
        assert_eq!(a.with_high_contrast(true).is_dark(), a.is_dark());
        assert!(a.with_high_contrast(true).is_high_contrast());
        assert_eq!(a.with_high_contrast(false), a.base());
    }
}

#[test]
fn appearance_names_round_trip_through_parsing() {
    for a in Appearance::ALL {
        assert_eq!(a.as_str().parse::<Appearance>(), Ok(a));
    }
    assert_eq!(Appearance::DarkHighContrast.as_str(), "dark-high-contrast");
    assert!("sepia".parse::<Appearance>().is_err());
}

#[test]
fn evidence_classes_order_from_unknown_to_verified() {
    assert!(Evidence::Unknown < Evidence::Approximated);
    assert!(Evidence::Approximated < Evidence::Inferred);
    assert!(Evidence::Inferred < Evidence::Documented);
    assert!(Evidence::Documented < Evidence::Verified);
}

#[test]
fn the_weakest_class_in_a_label_wins() {
    assert_eq!(Evidence::parse_weakest("VERIFIED"), Evidence::Verified);
    assert_eq!(
        Evidence::parse_weakest("VERIFIED (a) / INFERRED (b)"),
        Evidence::Inferred
    );
    assert_eq!(
        Evidence::parse_weakest("DOCUMENTED / APPROXIMATED"),
        Evidence::Approximated
    );
}

#[test]
fn a_label_without_a_class_is_unknown() {
    assert_eq!(Evidence::parse_weakest("nothing here"), Evidence::Unknown);
    assert_eq!(Evidence::parse_weakest(""), Evidence::Unknown);
}

#[test]
fn evidence_names_are_upper_case_words() {
    assert_eq!(Evidence::Verified.as_str(), "VERIFIED");
    for e in [
        Evidence::Unknown,
        Evidence::Approximated,
        Evidence::Inferred,
        Evidence::Documented,
        Evidence::Verified,
    ] {
        assert_eq!(Evidence::parse_weakest(e.as_str()), e);
    }
}
