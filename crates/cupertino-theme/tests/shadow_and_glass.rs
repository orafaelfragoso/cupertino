//! The window shadow table and the glass tone rules.

use cupertino_theme::glass::{
    adapts_foreground, contrast_ratio, face_tone, refraction_profile, relative_luminance, FaceTone,
    FACE_TONE_HYSTERESIS, FACE_TONE_THRESHOLD, REFRACTION_EDGE_SHARE, SMALL_CONTROL_SIDE,
};
use cupertino_theme::{
    rim_radius, sigma_to_radius_px, window_shadow, ShadowOption, SHADOW_SIGMA_MAXIMUM,
    WINDOW_SHADOW_TABLE,
};

#[test]
fn the_table_has_a_row_for_every_class_and_state() {
    assert_eq!(WINDOW_SHADOW_TABLE.len(), 32);
    for option in ShadowOption::ALL {
        for active in [false, true] {
            for dark in [false, true] {
                for contrast in [false, true] {
                    let row = window_shadow(option, active, dark, contrast);
                    assert_eq!(
                        (row.option, row.active, row.dark, row.increased_contrast),
                        (option, active, dark, contrast)
                    );
                }
            }
        }
    }
}

#[test]
fn shadow_classes_are_single_bits() {
    let bits: Vec<u8> = ShadowOption::ALL.iter().map(|o| o.bits()).collect();
    assert_eq!(bits, vec![1, 2, 4, 8]);
}

#[test]
fn a_standard_window_shadow_is_wider_and_denser_while_active() {
    let on = window_shadow(ShadowOption::Standard, true, false, false);
    let off = window_shadow(ShadowOption::Standard, false, false, false);
    assert_eq!((on.sigma, on.vertical_offset), (20.0, 18.0));
    assert!((on.density - 0.40).abs() < 1e-9);
    assert_eq!((off.sigma, off.vertical_offset), (12.0, 8.0));
    assert!((off.density - 0.25).abs() < 1e-9);
}

#[test]
fn every_row_keeps_its_sigma_within_the_maximum() {
    for row in &WINDOW_SHADOW_TABLE {
        assert!(row.sigma > 0.0 && row.sigma <= SHADOW_SIGMA_MAXIMUM);
        assert!((0.0..=1.0).contains(&row.density));
    }
}

#[test]
fn the_pixel_radius_is_sigma_times_two_point_eight_rounded_up() {
    assert_eq!(sigma_to_radius_px(20.0), 56);
    assert_eq!(sigma_to_radius_px(12.0), 34);
    assert_eq!(sigma_to_radius_px(1.0), 3);
}

#[test]
fn the_shadow_extends_by_its_radius_and_offset() {
    let row = window_shadow(ShadowOption::Standard, true, false, false);
    assert_eq!(row.radius_px(), 56);
    assert_eq!(row.extent_side(), 56.0);
    assert_eq!(row.extent_below(), 56.0 + 18.0);
    assert_eq!(row.extent_above(), 56.0 - 18.0);
}

#[test]
fn the_rim_is_three_points_on_dense_screens_and_two_otherwise() {
    assert_eq!(rim_radius(1.0, 1.0), 2.0);
    assert_eq!(rim_radius(2.0, 1.0), 3.0);
    assert_eq!(rim_radius(1.5, 1.0), 2.0);
    assert_eq!(rim_radius(3.0, 1.0), 3.0);
}

#[test]
fn only_shapes_shorter_than_the_small_side_adapt_their_foreground() {
    assert!(adapts_foreground(SMALL_CONTROL_SIDE - 0.5));
    assert!(!adapts_foreground(SMALL_CONTROL_SIDE));
    assert!(!adapts_foreground(300.0));
}

#[test]
fn a_face_with_no_earlier_tone_splits_at_the_threshold() {
    assert_eq!(
        face_tone(FACE_TONE_THRESHOLD + 0.001, None),
        FaceTone::Light
    );
    assert_eq!(face_tone(FACE_TONE_THRESHOLD - 0.001, None), FaceTone::Dark);
}

#[test]
fn a_dark_face_turns_light_only_past_the_hysteresis() {
    let inside = FACE_TONE_THRESHOLD + FACE_TONE_HYSTERESIS - 0.001;
    let past = FACE_TONE_THRESHOLD + FACE_TONE_HYSTERESIS + 0.001;
    assert_eq!(face_tone(inside, Some(FaceTone::Dark)), FaceTone::Dark);
    assert_eq!(face_tone(past, Some(FaceTone::Dark)), FaceTone::Light);
}

#[test]
fn a_light_face_turns_dark_only_past_the_hysteresis() {
    let inside = FACE_TONE_THRESHOLD - FACE_TONE_HYSTERESIS + 0.001;
    let past = FACE_TONE_THRESHOLD - FACE_TONE_HYSTERESIS - 0.001;
    assert_eq!(face_tone(inside, Some(FaceTone::Light)), FaceTone::Light);
    assert_eq!(face_tone(past, Some(FaceTone::Light)), FaceTone::Dark);
}

#[test]
fn luminance_runs_from_black_to_white() {
    assert!(relative_luminance([0.0, 0.0, 0.0]).abs() < 1e-6);
    assert!((relative_luminance([1.0, 1.0, 1.0]) - 1.0).abs() < 1e-4);
}

#[test]
fn luminance_clamps_its_channels() {
    assert_eq!(
        relative_luminance([2.0, 2.0, 2.0]),
        relative_luminance([1.0, 1.0, 1.0])
    );
    assert_eq!(relative_luminance([-1.0, 0.0, 0.0]), 0.0);
}

#[test]
fn green_weighs_more_than_red_and_blue() {
    let r = relative_luminance([1.0, 0.0, 0.0]);
    let g = relative_luminance([0.0, 1.0, 0.0]);
    let b = relative_luminance([0.0, 0.0, 1.0]);
    assert!((r - 0.2126).abs() < 1e-4 && (g - 0.7152).abs() < 1e-4 && (b - 0.0722).abs() < 1e-4);
}

#[test]
fn contrast_between_black_and_white_is_twenty_one_to_one() {
    let c = contrast_ratio([0.0; 3], [1.0; 3]);
    assert!((c - 21.0).abs() < 0.01);
    assert!((contrast_ratio([1.0; 3], [0.0; 3]) - c).abs() < 1e-6);
    assert!((contrast_ratio([0.5; 3], [0.5; 3]) - 1.0).abs() < 1e-6);
}

#[test]
fn the_refraction_profile_starts_at_the_edge_share_and_dies_out_at_the_band_end() {
    assert_eq!(refraction_profile(0.0), REFRACTION_EDGE_SHARE);
    assert_eq!(refraction_profile(-1.0), REFRACTION_EDGE_SHARE);
    assert_eq!(refraction_profile(1.0), 0.0);
    assert_eq!(refraction_profile(3.0), 0.0);
}

#[test]
fn the_refraction_profile_falls_across_the_band() {
    let mut last = refraction_profile(0.0);
    for i in 1..20 {
        let v = refraction_profile(i as f32 / 20.0);
        assert!(v < last && v > 0.0);
        last = v;
    }
}
