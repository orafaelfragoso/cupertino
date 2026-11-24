//! The window shadow table and the glass tone rules.

use cupertino_theme::glass::{
    adapts_foreground, contrast_ratio, face_tone, refraction_profile, relative_luminance, FaceTone,
    FACE_TONE_HYSTERESIS, FACE_TONE_THRESHOLD, REFRACTION_EDGE_SHARE, SMALL_CONTROL_SIDE,
};
use cupertino_theme::{
    rim_radius, sigma_to_radius_px, window_shadow, ShadowOption, SHADOW_SIGMA_MAXIMUM,
};

#[test]
fn every_class_and_state_has_its_own_row_within_the_limits() {
    for option in ShadowOption::ALL {
        for active in [false, true] {
            for dark in [false, true] {
                for contrast in [false, true] {
                    let row = window_shadow(option, active, dark, contrast);
                    assert_eq!(
                        (row.option, row.active, row.dark, row.increased_contrast),
                        (option, active, dark, contrast)
                    );
                    assert!(
                        row.sigma > 0.0 && row.sigma <= SHADOW_SIGMA_MAXIMUM,
                        "{row:?}"
                    );
                    assert!((0.0..=1.0).contains(&row.density), "{row:?}");
                }
            }
        }
    }
}

#[test]
fn a_standard_window_shadow_is_wider_lower_and_denser_while_active() {
    for dark in [false, true] {
        let on = window_shadow(ShadowOption::Standard, true, dark, false);
        let off = window_shadow(ShadowOption::Standard, false, dark, false);
        assert!(on.sigma > off.sigma, "dark={dark}");
        assert!(on.vertical_offset > off.vertical_offset, "dark={dark}");
        assert!(on.density > off.density, "dark={dark}");
    }
}

#[test]
fn an_inactive_standard_window_casts_a_tight_low_shadow_in_both_contrasts() {
    for contrast in [false, true] {
        for (dark, density) in [(false, 0.22), (true, 0.45)] {
            let row = window_shadow(ShadowOption::Standard, false, dark, contrast);
            assert_eq!(
                (row.density, row.sigma, row.vertical_offset),
                (density, 8.0, 7.0),
                "dark {dark}, contrast {contrast}"
            );
        }
    }
}

#[test]
fn the_shadow_reaches_its_rounded_up_radius_shifted_by_its_offset() {
    for (sigma, radius) in [(20.0, 56), (12.0, 34), (1.0, 3)] {
        assert_eq!(sigma_to_radius_px(sigma), radius, "sigma {sigma}");
    }
    let row = window_shadow(ShadowOption::Standard, true, false, false);
    let r = f64::from(row.radius_px());
    assert_eq!(row.radius_px(), sigma_to_radius_px(row.sigma));
    assert_eq!(row.extent_side(), r);
    assert_eq!(row.extent_below(), r + row.vertical_offset);
    assert_eq!(row.extent_above(), r - row.vertical_offset);
}

#[test]
fn the_rim_is_three_points_on_dense_screens_and_two_otherwise() {
    for (scale, rim) in [(1.0, 2.0), (1.5, 2.0), (2.0, 3.0), (3.0, 3.0)] {
        assert_eq!(rim_radius(scale, 1.0), rim, "scale {scale}");
    }
}

#[test]
fn only_shapes_shorter_than_the_small_side_adapt_their_foreground() {
    for (side, adapts) in [
        (SMALL_CONTROL_SIDE - 0.5, true),
        (SMALL_CONTROL_SIDE, false),
        (300.0, false),
    ] {
        assert_eq!(adapts_foreground(side), adapts, "side {side}");
    }
}

#[test]
fn a_face_tone_changes_only_past_the_hysteresis() {
    let t = FACE_TONE_THRESHOLD;
    let h = FACE_TONE_HYSTERESIS;
    let e = 0.001;
    // (luminance, earlier tone, tone)
    for (luminance, earlier, tone) in [
        (t + e, None, FaceTone::Light),
        (t - e, None, FaceTone::Dark),
        (t + h - e, Some(FaceTone::Dark), FaceTone::Dark),
        (t + h + e, Some(FaceTone::Dark), FaceTone::Light),
        (t - h + e, Some(FaceTone::Light), FaceTone::Light),
        (t - h - e, Some(FaceTone::Light), FaceTone::Dark),
    ] {
        assert_eq!(
            face_tone(luminance, earlier),
            tone,
            "{luminance} after {earlier:?}"
        );
    }
}

#[test]
fn relative_luminance_weighs_clamped_channels() {
    for (rgb, luminance) in [
        ([0.0, 0.0, 0.0], 0.0),
        ([1.0, 1.0, 1.0], 1.0),
        ([2.0, 2.0, 2.0], 1.0),
        ([-1.0, 0.0, 0.0], 0.0),
        ([1.0, 0.0, 0.0], 0.2126),
        ([0.0, 1.0, 0.0], 0.7152),
        ([0.0, 0.0, 1.0], 0.0722),
    ] {
        assert!(
            (relative_luminance(rgb) - luminance).abs() < 1e-4,
            "{rgb:?}"
        );
    }
}

#[test]
fn contrast_ratio_is_symmetric_from_one_to_twenty_one() {
    for (a, b, ratio) in [
        ([0.0; 3], [1.0; 3], 21.0),
        ([1.0; 3], [0.0; 3], 21.0),
        ([0.5; 3], [0.5; 3], 1.0),
    ] {
        assert!((contrast_ratio(a, b) - ratio).abs() < 0.01, "{a:?} {b:?}");
    }
}

#[test]
fn the_refraction_profile_falls_from_the_edge_share_to_nothing_across_the_band() {
    for (t, share) in [
        (-1.0, REFRACTION_EDGE_SHARE),
        (0.0, REFRACTION_EDGE_SHARE),
        (1.0, 0.0),
        (3.0, 0.0),
    ] {
        assert_eq!(refraction_profile(t), share, "at {t}");
    }
    let mut last = refraction_profile(0.0);
    for i in 1..20 {
        let v = refraction_profile(i as f32 / 20.0);
        assert!(v < last && v > 0.0, "at {i}/20");
        last = v;
    }
}
