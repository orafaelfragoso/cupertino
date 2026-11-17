//! The colour value type, appearances and evidence classes.

use cupertino_theme::{Appearance, Color, Evidence};

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

#[test]
fn eight_bit_and_hex_forms_convert_and_clamp() {
    for c in [[0, 0, 0, 0], [255, 255, 255, 255], [12, 200, 77, 128]] {
        assert_eq!(Color::from_rgba8(c).to_rgba8(), c, "{c:?} round trip");
    }
    assert!(close(
        Color::from_rgba8([255, 0, 51, 255]).to_array(),
        [1.0, 0.0, 0.2, 1.0]
    ));
    assert_eq!(
        Color::new(2.0, -1.0, 0.5, 1.5).to_rgba8(),
        [255, 0, 128, 255],
        "out-of-range channels clamp"
    );
    assert_eq!(
        Color::from_rgba8([255, 0, 171, 10]).hex(),
        "#FF00AB",
        "hex drops alpha"
    );
}

#[test]
fn colour_arithmetic() {
    let a = Color::new(0.0, 0.0, 0.0, 0.0);
    let b = Color::new(1.0, 0.5, 0.2, 1.0);
    for (label, got, expected) in [
        (
            "premultiplied",
            Color::new(1.0, 0.5, 0.0, 0.5).premultiplied(),
            [0.5, 0.25, 0.0, 0.5],
        ),
        (
            "with_alpha",
            Color::new(0.1, 0.2, 0.3, 1.0).with_alpha(0.4).to_array(),
            [0.1, 0.2, 0.3, 0.4],
        ),
        ("lerp at 0", a.lerp(b, 0.0).to_array(), a.to_array()),
        ("lerp at 1", a.lerp(b, 1.0).to_array(), b.to_array()),
        (
            "lerp at 0.5",
            a.lerp(b, 0.5).to_array(),
            [0.5, 0.25, 0.1, 0.5],
        ),
    ] {
        assert!(close(got, expected), "{label}: {got:?}");
    }
}

#[test]
fn appearances_tell_darkness_and_contrast_apart_and_convert() {
    use Appearance::*;
    // (appearance, dark, high contrast, base, name)
    for (a, dark, contrast, base, name) in [
        (Light, false, false, Light, "light"),
        (Dark, true, false, Dark, "dark"),
        (LightHighContrast, false, true, Light, "light-high-contrast"),
        (DarkHighContrast, true, true, Dark, "dark-high-contrast"),
    ] {
        assert_eq!(
            (a.is_dark(), a.is_high_contrast()),
            (dark, contrast),
            "{a:?}"
        );
        assert_eq!(a.base(), base, "{a:?}");
        assert_eq!(Appearance::of_session(dark, contrast), a, "{a:?}");
        assert_eq!(a.with_high_contrast(false), base, "{a:?}");
        assert_eq!(
            a.with_high_contrast(true),
            Appearance::of_session(dark, true),
            "{a:?}"
        );
        assert_eq!(name.parse::<Appearance>(), Ok(a), "{name}");
        assert_eq!(a.as_str().parse::<Appearance>(), Ok(a), "{name}");
    }
    assert!("sepia".parse::<Appearance>().is_err());
}

#[test]
fn the_weakest_evidence_class_in_a_label_wins() {
    let v = Evidence::Verified.as_str();
    let d = Evidence::Documented.as_str();
    let i = Evidence::Inferred.as_str();
    let a = Evidence::Approximated.as_str();
    for (label, expected) in [
        (v.to_string(), Evidence::Verified),
        (format!("{v} (a) / {i} (b)"), Evidence::Inferred),
        (format!("{d} / {a}"), Evidence::Approximated),
        (d.to_string(), Evidence::Documented),
        ("nothing here".to_string(), Evidence::Unknown),
        (String::new(), Evidence::Unknown),
    ] {
        assert_eq!(Evidence::parse_weakest(&label), expected, "{label:?}");
    }
}
