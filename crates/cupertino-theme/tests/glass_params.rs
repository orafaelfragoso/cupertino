//! The glass recipes: how a shape's size, style and the display options set them.

use cupertino_theme::glass::{
    apply_matrix, foreground_matrix, foreground_matrix_for, foreground_rgb, FaceTone, BLUR_LEVELS,
    CARD_CAPTURE_SCALE, GLASS_BACKDROP_SCALE, LARGE_PANEL_BLUR_SCALE, TRACK_CAPTURE_SCALE,
};
use cupertino_theme::{GlassMode, GlassParams, GlassStyle};

const STYLES: [GlassStyle; 16] = [
    GlassStyle::Regular,
    GlassStyle::Clear,
    GlassStyle::LargePanel,
    GlassStyle::Bar,
    GlassStyle::Dense,
    GlassStyle::Module,
    GlassStyle::Card,
    GlassStyle::Track,
    GlassStyle::Popover,
    GlassStyle::Switcher,
    GlassStyle::Sidebar,
    GlassStyle::SubduedModule,
    GlassStyle::Snippet,
    GlassStyle::Veiled,
    GlassStyle::Toolbar,
    GlassStyle::Menu,
];

const SIZES: [f32; 6] = [0.0, 10.0, 32.0, 63.0, 64.0, 400.0];

fn mode(style: GlassStyle, dark: bool) -> GlassMode {
    GlassMode {
        style,
        dark,
        ..GlassMode::default()
    }
}

fn params(style: GlassStyle, dark: bool, side: f32) -> GlassParams {
    GlassParams::new(mode(style, dark), side)
}

fn every_mode() -> Vec<GlassMode> {
    let mut out = Vec::new();
    for style in STYLES {
        for bits in 0..16u8 {
            out.push(GlassMode {
                style,
                dark: bits & 1 != 0,
                reduce_transparency: bits & 2 != 0,
                increase_contrast: bits & 4 != 0,
                reduce_motion: bits & 8 != 0,
                density: 0,
            });
        }
    }
    out
}

fn finite(p: &GlassParams) -> bool {
    let mut v = vec![p.blur.radius, p.blur.fill_radius, p.capture_scale];
    v.extend(p.blur.level_opacity);
    v.extend(p.face.fill);
    v.extend([
        p.face.saturation,
        p.face.white,
        p.face.black,
        p.face.max_luma,
    ]);
    v.extend([p.refraction.amount, p.refraction.height]);
    v.extend(p.shadow.offset);
    v.extend(p.shadow.fill);
    v.extend(p.veil);
    v.iter().all(|x| x.is_finite())
}

#[test]
fn every_style_in_every_mode_and_size_gives_finite_values() {
    for m in every_mode() {
        for side in SIZES {
            let p = GlassParams::new(m, side);
            assert!(finite(&p), "{m:?} {side}");
            assert!(p.blur.radius >= 0.0 && p.blur.fill_radius >= 0.0, "{m:?}");
            assert!(p.blur_sigma() >= 0.0);
        }
    }
}

#[test]
fn a_size_that_is_not_a_length_is_taken_as_zero() {
    for style in STYLES {
        let zero = params(style, false, 0.0);
        assert_eq!(params(style, false, f32::NAN), zero, "{style:?}");
        assert_eq!(params(style, false, f32::INFINITY), zero, "{style:?}");
        assert_eq!(params(style, false, -12.0), zero, "{style:?}");
    }
}

#[test]
fn the_backdrop_is_captured_at_half_scale_but_cards_and_tracks_differ() {
    assert_eq!(
        params(GlassStyle::Regular, false, 40.0).capture_scale,
        GLASS_BACKDROP_SCALE
    );
    assert_eq!(
        params(GlassStyle::Card, false, 40.0).capture_scale,
        CARD_CAPTURE_SCALE
    );
    assert_eq!(
        params(GlassStyle::Track, false, 40.0).capture_scale,
        TRACK_CAPTURE_SCALE
    );
    const {
        assert!(
            CARD_CAPTURE_SCALE < GLASS_BACKDROP_SCALE && GLASS_BACKDROP_SCALE < TRACK_CAPTURE_SCALE
        )
    };
}

#[test]
fn regular_glass_blurs_five_points_under_an_eight_point_fill() {
    for dark in [false, true] {
        let p = params(GlassStyle::Regular, dark, 100.0);
        assert_eq!((p.blur.radius, p.blur.fill_radius), (5.0, 8.0));
        assert_eq!(p.blur.fill_normal, 0.46);
    }
}

#[test]
fn the_blur_fill_darkens_in_the_dark_appearance_and_lightens_in_the_light() {
    let dark = params(GlassStyle::Regular, true, 100.0);
    let light = params(GlassStyle::Regular, false, 100.0);
    assert_eq!(
        (dark.blur.fill_darken, dark.blur.fill_lighten),
        (0.882, 0.0)
    );
    assert_eq!(
        (light.blur.fill_darken, light.blur.fill_lighten),
        (0.0, 0.882)
    );
    assert!(dark.fill_shows() && light.fill_shows());
}

#[test]
fn the_refraction_strength_grows_with_the_shape_up_to_a_cap() {
    let small = params(GlassStyle::Regular, false, 40.0).refraction;
    assert_eq!(
        small.amount, -20.0,
        "half a point per point of side, bending inward"
    );
    let big = params(GlassStyle::Regular, false, 400.0).refraction;
    assert_eq!(big.amount, -60.0);
    assert!(big.height <= 20.0 && big.height >= small.height);
    assert_eq!(
        params(GlassStyle::Regular, false, 0.0).refraction.amount,
        0.0
    );
}

#[test]
fn clear_glass_refracts_harder_per_point_than_regular() {
    let regular = params(GlassStyle::Regular, false, 40.0).refraction.amount;
    let clear = params(GlassStyle::Clear, false, 40.0).refraction.amount;
    assert!(clear < regular, "{clear} vs {regular}");
    assert_eq!(clear, -26.0);
}

#[test]
fn a_small_control_blurs_less_than_a_large_panel_of_the_same_recipe() {
    let small = params(GlassStyle::Regular, false, 8.0);
    let large = params(GlassStyle::Regular, false, 300.0);
    assert!(small.blur.level_opacity[0] < large.blur.level_opacity[0]);
    assert!(small.blur_sigma() < large.blur_sigma());
}

#[test]
fn the_blur_deviation_follows_the_radius_and_is_capped() {
    let mut p = params(GlassStyle::Regular, false, 300.0);
    let base = p.blur_sigma();
    p.blur.radius *= 2.0;
    assert!((p.blur_sigma() / base - 2.0).abs() < 1e-5);
    p.blur.radius = 1.0e6;
    let capped = p.blur_sigma();
    assert!(capped > 0.0 && capped < 1.0e6);
    p.blur.radius = 2.0e6;
    assert_eq!(p.blur_sigma(), capped);
    p.blur.level_opacity[0] = 0.0;
    assert_eq!(p.blur_sigma(), 0.0);
}

#[test]
fn a_large_panel_blurs_twice_as_wide_as_regular_glass() {
    assert_eq!(LARGE_PANEL_BLUR_SCALE, 2.0);
    for dark in [false, true] {
        let regular = params(GlassStyle::Regular, dark, 300.0);
        let panel = params(GlassStyle::LargePanel, dark, 300.0);
        assert_eq!(panel.blur.radius, regular.blur.radius * 2.0);
        assert_eq!(panel.blur.fill_radius, regular.blur.fill_radius * 2.0);
        assert!(
            panel.shadow.amount > regular.shadow.amount,
            "a broad shadow of its own"
        );
    }
}

#[test]
fn veiled_glass_darkens_from_a_fifth_at_the_top_to_fully_at_the_bottom() {
    assert_eq!(params(GlassStyle::Veiled, true, 50.0).veil, [0.2, 1.0]);
    assert_eq!(params(GlassStyle::Regular, true, 50.0).veil, [0.0, 0.0]);
}

#[test]
fn only_modes_that_differ_in_the_options_give_different_recipes() {
    let base = mode(GlassStyle::Regular, false);
    assert_eq!(GlassParams::new(base, 50.0), GlassParams::new(base, 50.0));
    assert_ne!(
        GlassParams::new(base, 50.0),
        GlassParams::new(GlassMode { dark: true, ..base }, 50.0)
    );
}

#[test]
fn reduce_transparency_turns_off_refraction_and_the_ring_and_sets_a_wide_even_blur() {
    for style in STYLES {
        let m = GlassMode {
            reduce_transparency: true,
            ..mode(style, false)
        };
        let p = GlassParams::new(m, 100.0);
        assert_eq!(
            (p.refraction.amount, p.refraction.height),
            (0.0, 0.0),
            "{style:?}"
        );
        assert_eq!(
            (
                p.shadow.ring_blur,
                p.shadow.ring_stroke,
                p.shadow.ring_offset
            ),
            (0.0, 0.0, 0.0),
            "{style:?}"
        );
        let expected = if style == GlassStyle::Clear {
            200.0
        } else {
            40.625
        };
        assert_eq!(p.blur.radius, expected, "{style:?}");
        if style != GlassStyle::Clear {
            assert_eq!(p.blur.level_opacity[..2], [1.0, 1.0], "{style:?}");
            assert!((p.blur.level_opacity[2] - 0.123_076_92).abs() < 1e-6);
            assert_eq!(p.blur.level_opacity[3], p.blur.level_opacity[2]);
        }
    }
}

#[test]
fn reduce_transparency_makes_the_face_denser_than_the_plain_recipe() {
    for dark in [false, true] {
        let plain = params(GlassStyle::Regular, dark, 100.0);
        let reduced = GlassParams::new(
            GlassMode {
                reduce_transparency: true,
                ..mode(GlassStyle::Regular, dark)
            },
            100.0,
        );
        assert_ne!(plain.face, reduced.face, "dark={dark}");
    }
}

#[test]
fn a_large_dark_panel_with_reduced_transparency_keeps_only_a_trace_of_colour() {
    let p = GlassParams::new(
        GlassMode {
            reduce_transparency: true,
            ..mode(GlassStyle::LargePanel, true)
        },
        400.0,
    );
    assert_eq!(p.face.saturation, 0.3);
}

#[test]
fn increase_contrast_changes_the_face_of_regular_glass() {
    for dark in [false, true] {
        let plain = params(GlassStyle::Regular, dark, 100.0);
        let contrast = GlassParams::new(
            GlassMode {
                increase_contrast: true,
                ..mode(GlassStyle::Regular, dark)
            },
            100.0,
        );
        assert_ne!(plain.face, contrast.face);
        assert_eq!(plain.blur, contrast.blur, "only the colours change");
    }
}

#[test]
fn reduce_motion_flattens_the_bending_edge_and_softens_the_blur() {
    for style in STYLES {
        let p = GlassParams::new(
            GlassMode {
                reduce_motion: true,
                ..mode(style, false)
            },
            100.0,
        );
        assert_eq!(p.refraction.height, 0.0, "{style:?}");
        assert!(p.blur.radius >= 8.0, "{style:?}");
        assert_eq!(p.blur.level_opacity, [1.0, 1.0, 0.625, 0.625], "{style:?}");
    }
}

#[test]
fn reduce_motion_does_not_shrink_a_wider_blur() {
    let plain = params(GlassStyle::LargePanel, false, 400.0);
    let reduced = GlassParams::new(
        GlassMode {
            reduce_motion: true,
            ..mode(GlassStyle::LargePanel, false)
        },
        400.0,
    );
    assert_eq!(reduced.blur.radius, plain.blur.radius);
}

#[test]
fn a_menu_without_transparency_loses_its_glow() {
    let plain = params(GlassStyle::Menu, false, 200.0);
    assert!(plain.highlight.glow_amount > 0.0 || plain.highlight.amount > 0.0);
    for (rt, ic) in [(true, false), (false, true), (true, true)] {
        let p = GlassParams::new(
            GlassMode {
                reduce_transparency: rt,
                increase_contrast: ic,
                ..mode(GlassStyle::Menu, false)
            },
            200.0,
        );
        assert_eq!(p.highlight.glow_amount, 0.0, "rt={rt} ic={ic}");
    }
}

#[test]
fn density_makes_the_face_fill_more_opaque_and_never_less() {
    for style in STYLES {
        for dark in [false, true] {
            let plain = params(style, dark, 100.0);
            let dense = GlassParams::new(
                GlassMode {
                    density: 100,
                    ..mode(style, dark)
                },
                100.0,
            );
            assert!(
                dense.face.fill[3] >= plain.face.fill[3] - 1e-6,
                "{style:?} {dark}"
            );
            assert_eq!(dense.blur, plain.blur);
        }
    }
}

#[test]
fn density_beyond_a_hundred_is_a_hundred() {
    let at = |density| {
        GlassParams::new(
            GlassMode {
                density,
                ..mode(GlassStyle::Regular, false)
            },
            100.0,
        )
    };
    assert_eq!(at(100), at(250));
    assert_ne!(at(0), at(100));
}

#[test]
fn density_does_not_change_a_recipe_that_reduces_transparency() {
    let at = |density| {
        GlassParams::new(
            GlassMode {
                density,
                reduce_transparency: true,
                ..mode(GlassStyle::Regular, false)
            },
            100.0,
        )
    };
    assert_eq!(at(0), at(80));
}

#[test]
fn an_alert_shadow_is_stronger_while_it_has_the_keyboard() {
    let p = params(GlassStyle::Regular, false, 200.0).shadow;
    let on = p.alert(true, false);
    let off = p.alert(false, false);
    assert!(on.fill[3] > off.fill[3]);
    assert_eq!(on.amount, 100.0);
    assert_eq!(on.opacity, 1.0);
    assert_eq!(on.offset[0], 0.0);
    assert!(on.offset[1] > 0.0);
    assert_eq!(on.ring_blur, p.ring_blur, "the ring is kept");
    let dark = p.alert(true, true);
    assert_ne!(dark.fill[3], on.fill[3]);
    assert_eq!(&on.fill[..3], &[0.0, 0.0, 0.0]);
}

// foreground

#[test]
fn the_foreground_matrix_depends_on_the_appearance() {
    assert_ne!(foreground_matrix(true), foreground_matrix(false));
    assert_eq!(FaceTone::Dark.matrix(), foreground_matrix(true));
    assert_eq!(FaceTone::Light.matrix(), foreground_matrix(false));
}

#[test]
fn an_unmeasured_face_uses_the_appearances_matrix() {
    for dark in [false, true] {
        assert_eq!(foreground_matrix_for(dark, None), foreground_matrix(dark));
    }
    assert_eq!(
        foreground_matrix_for(false, Some(FaceTone::Dark)),
        foreground_matrix(true)
    );
    assert_eq!(
        foreground_matrix_for(true, Some(FaceTone::Light)),
        foreground_matrix(false)
    );
}

#[test]
fn the_matrix_carries_a_mid_face_to_white_on_dark_glass_and_black_on_light() {
    let mid = [0.5, 0.5, 0.5, 1.0];
    let on_dark = apply_matrix(&foreground_matrix(true), mid);
    let on_light = apply_matrix(&foreground_matrix(false), mid);
    assert!(on_dark[..3].iter().all(|v| *v > 0.9));
    assert!(on_light[..3].iter().all(|v| *v < 0.1));
}

#[test]
fn the_foreground_colour_is_clamped_to_the_unit_range() {
    for dark in [false, true] {
        for tone in [None, Some(FaceTone::Dark), Some(FaceTone::Light)] {
            let rgb = foreground_rgb(dark, tone);
            assert!(rgb.iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }
}

#[test]
fn the_identity_matrix_leaves_a_colour_alone() {
    let mut id = foreground_matrix(true);
    id.fill(0.0);
    for i in 0..4 {
        id[i * 5 + i] = 1.0;
    }
    assert_eq!(
        apply_matrix(&id, [0.1, 0.2, 0.3, 0.4]),
        [0.1, 0.2, 0.3, 0.4]
    );
}

#[test]
fn there_are_four_blur_levels() {
    assert_eq!(BLUR_LEVELS, 4);
    assert_eq!(
        params(GlassStyle::Regular, false, 10.0)
            .blur
            .level_opacity
            .len(),
        4
    );
}
