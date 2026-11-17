//! The display's colour chain: which operations the accessibility settings ask for and what the chain does to a
//! colour.

use cupertino_defaults::accessibility::{AccessibilitySettings, ColorFilter};
use cupertino_glass::cpu::apply_color_ops;
use cupertino_glass::display_filter::{display_filter_ops, filter_matrix};
use cupertino_glass::ops::color_ops;
use cupertino_theme::recipe::ColorOp;

fn apply(settings: &AccessibilitySettings, c: [f32; 3]) -> [f32; 3] {
    apply_color_ops(c, &color_ops(&display_filter_ops(settings)))
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
}

#[test]
fn settings_that_change_nothing_leave_the_display_alone() {
    let settings = AccessibilitySettings::default();
    assert!(display_filter_ops(&settings).is_empty());
    let c = [0.2, 0.5, 0.9];
    assert_eq!(apply(&settings, c), c);
}

#[test]
fn each_setting_adds_its_step_in_the_order_filter_invert_contrast() {
    let settings = AccessibilitySettings {
        color_filter: Some(ColorFilter::Grayscale),
        invert_colors: true,
        display_contrast: 0.5,
        ..AccessibilitySettings::default()
    };
    let ops = display_filter_ops(&settings);
    assert!(matches!(
        ops.as_slice(),
        [
            ColorOp::LinearMatrix(_),
            ColorOp::Invert,
            ColorOp::Contrast(_)
        ]
    ));
}

#[test]
fn invert_flips_the_encoded_values_and_contrast_stretches_the_range() {
    let inverted = AccessibilitySettings {
        invert_colors: true,
        ..AccessibilitySettings::default()
    };
    assert!(close(apply(&inverted, [0.2, 0.5, 1.0]), [0.8, 0.5, 0.0]));

    let contrast = AccessibilitySettings {
        display_contrast: 0.4,
        ..AccessibilitySettings::default()
    };
    // The range [0.2, 0.8] becomes [0, 1]; the middle stays.
    assert!(close(apply(&contrast, [0.2, 0.5, 0.8]), [0.0, 0.5, 1.0]));
}

#[test]
fn grayscale_makes_the_channels_equal_and_keeps_grays() {
    let settings = AccessibilitySettings {
        color_filter: Some(ColorFilter::Grayscale),
        color_filter_intensity: Some(1.0),
        ..AccessibilitySettings::default()
    };
    let [r, g, b] = apply(&settings, [0.9, 0.2, 0.1]);
    assert!((r - g).abs() < 2e-3 && (g - b).abs() < 2e-3, "{r} {g} {b}");
    assert!(close(apply(&settings, [0.4, 0.4, 0.4]), [0.4, 0.4, 0.4]));
}

#[test]
fn a_filter_at_no_intensity_is_the_identity() {
    let filters = [
        ColorFilter::Protanopia,
        ColorFilter::Deuteranopia,
        ColorFilter::Tritanopia,
        ColorFilter::Grayscale,
        ColorFilter::Tint { hue: 0.3 },
    ];
    for filter in filters {
        let none = filter_matrix(filter, 0.0);
        for (i, row) in none.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((v - expected).abs() < 1e-9, "{filter:?} at zero intensity");
            }
        }
    }
}
