//! The glass recipes: how a shape's size, style and the display options set them.

use cupertino_theme::glass::{
    apply_matrix, foreground_matrix, foreground_matrix_for, foreground_rgb, FaceTone,
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

fn with(m: GlassMode, side: f32, change: impl FnOnce(&mut GlassMode)) -> GlassParams {
    let mut m = m;
    change(&mut m);
    GlassParams::new(m, side)
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
fn every_style_in_every_mode_and_size_gives_finite_non_negative_blurs() {
    for style in STYLES {
        for bits in 0..16u8 {
            let m = GlassMode {
                style,
                dark: bits & 1 != 0,
                reduce_transparency: bits & 2 != 0,
                increase_contrast: bits & 4 != 0,
                reduce_motion: bits & 8 != 0,
                density: 0,
            };
            for side in [0.0, 10.0, 32.0, 63.0, 64.0, 400.0] {
                let p = GlassParams::new(m, side);
                assert!(finite(&p), "{m:?} {side}");
                assert!(p.blur.radius >= 0.0 && p.blur.fill_radius >= 0.0, "{m:?}");
                assert!(p.blur_sigma() >= 0.0, "{m:?}");
            }
        }
    }
}

#[test]
fn a_size_that_is_not_a_length_is_taken_as_zero() {
    for style in STYLES {
        let zero = params(style, false, 0.0);
        for side in [f32::NAN, f32::INFINITY, -12.0] {
            assert_eq!(params(style, false, side), zero, "{style:?} {side}");
        }
    }
}

#[test]
fn cards_and_tracks_capture_the_backdrop_at_their_own_scale() {
    for (style, scale) in [
        (GlassStyle::Regular, GLASS_BACKDROP_SCALE),
        (GlassStyle::Card, CARD_CAPTURE_SCALE),
        (GlassStyle::Track, TRACK_CAPTURE_SCALE),
    ] {
        assert_eq!(params(style, false, 40.0).capture_scale, scale, "{style:?}");
    }
}

#[test]
fn the_blur_fill_darkens_in_the_dark_appearance_and_lightens_in_the_light() {
    let dark = params(GlassStyle::Regular, true, 100.0).blur;
    let light = params(GlassStyle::Regular, false, 100.0).blur;
    assert!(dark.fill_darken > 0.0 && dark.fill_lighten == 0.0);
    assert!(light.fill_lighten > 0.0 && light.fill_darken == 0.0);
    assert!(params(GlassStyle::Regular, true, 100.0).fill_shows());
    assert!(params(GlassStyle::Regular, false, 100.0).fill_shows());
}

#[test]
fn refraction_bends_inward_harder_for_larger_shapes_up_to_a_cap_and_hardest_for_clear_glass() {
    let amount = |style, side| params(style, false, side).refraction.amount;
    assert_eq!(amount(GlassStyle::Regular, 0.0), 0.0);
    assert!(amount(GlassStyle::Regular, 40.0) < 0.0, "inward");
    assert!(amount(GlassStyle::Regular, 400.0) < amount(GlassStyle::Regular, 40.0));
    assert_eq!(
        amount(GlassStyle::Regular, 400.0),
        amount(GlassStyle::Regular, 4000.0),
        "capped"
    );
    assert!(amount(GlassStyle::Clear, 40.0) < amount(GlassStyle::Regular, 40.0));
    let small = params(GlassStyle::Regular, false, 40.0).refraction;
    let big = params(GlassStyle::Regular, false, 400.0).refraction;
    assert!(big.height >= small.height);
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
    assert_eq!(p.blur_sigma(), 0.0, "no first level, no blur");
}

#[test]
fn a_large_panel_blurs_wider_than_regular_glass_with_a_broader_shadow() {
    for dark in [false, true] {
        let regular = params(GlassStyle::Regular, dark, 300.0);
        let panel = params(GlassStyle::LargePanel, dark, 300.0);
        assert_eq!(
            panel.blur.radius,
            regular.blur.radius * LARGE_PANEL_BLUR_SCALE
        );
        assert_eq!(
            panel.blur.fill_radius,
            regular.blur.fill_radius * LARGE_PANEL_BLUR_SCALE
        );
        assert!(panel.shadow.amount > regular.shadow.amount);
    }
}

#[test]
fn only_veiled_glass_darkens_more_towards_the_bottom() {
    for style in STYLES {
        let [top, bottom] = params(style, true, 50.0).veil;
        if style == GlassStyle::Veiled {
            assert!(0.0 < top && top < bottom, "{top} {bottom}");
        } else {
            assert_eq!([top, bottom], [0.0, 0.0], "{style:?}");
        }
    }
}

#[test]
fn reduce_transparency_drops_refraction_and_the_ring_for_a_wider_blur_and_a_denser_face() {
    for style in STYLES {
        for dark in [false, true] {
            let plain = params(style, dark, 100.0);
            let p = with(mode(style, dark), 100.0, |m| m.reduce_transparency = true);
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
            assert!(p.blur.radius > plain.blur.radius, "{style:?} dark={dark}");
            if style == GlassStyle::Regular {
                assert_ne!(plain.face, p.face, "dark={dark}");
            }
        }
    }
}

#[test]
fn increase_contrast_changes_only_the_colours_of_regular_glass() {
    for dark in [false, true] {
        let plain = params(GlassStyle::Regular, dark, 100.0);
        let contrast = with(mode(GlassStyle::Regular, dark), 100.0, |m| {
            m.increase_contrast = true
        });
        assert_ne!(plain.face, contrast.face);
        assert_eq!(plain.blur, contrast.blur);
    }
}

#[test]
fn reduce_motion_flattens_the_bending_edge_and_never_narrows_the_blur() {
    for style in STYLES {
        for side in [100.0, 400.0] {
            let plain = params(style, false, side);
            let p = with(mode(style, false), side, |m| m.reduce_motion = true);
            assert_eq!(p.refraction.height, 0.0, "{style:?}");
            assert!(p.blur.radius >= plain.blur.radius, "{style:?} {side}");
        }
    }
}

#[test]
fn a_menu_loses_its_glow_without_transparency_or_with_more_contrast() {
    let plain = params(GlassStyle::Menu, false, 200.0);
    assert!(plain.highlight.glow_amount > 0.0 || plain.highlight.amount > 0.0);
    for (rt, ic) in [(true, false), (false, true), (true, true)] {
        let p = with(mode(GlassStyle::Menu, false), 200.0, |m| {
            m.reduce_transparency = rt;
            m.increase_contrast = ic;
        });
        assert_eq!(p.highlight.glow_amount, 0.0, "rt={rt} ic={ic}");
    }
}

#[test]
fn density_only_thickens_the_face_up_to_a_hundred_and_not_under_reduced_transparency() {
    for style in STYLES {
        for dark in [false, true] {
            let plain = params(style, dark, 100.0);
            let dense = with(mode(style, dark), 100.0, |m| m.density = 100);
            assert!(
                dense.face.fill[3] >= plain.face.fill[3] - 1e-6,
                "{style:?} {dark}"
            );
            assert_eq!(dense.blur, plain.blur, "{style:?} {dark}");
        }
    }
    let at = |density, reduce_transparency| {
        with(mode(GlassStyle::Regular, false), 100.0, |m| {
            m.density = density;
            m.reduce_transparency = reduce_transparency;
        })
    };
    assert_ne!(at(0, false), at(100, false));
    assert_eq!(
        at(100, false),
        at(250, false),
        "beyond a hundred is a hundred"
    );
    assert_eq!(at(0, true), at(80, true));
}

#[test]
fn an_alert_shadow_is_stronger_while_it_has_the_keyboard_and_keeps_the_ring() {
    let p = params(GlassStyle::Regular, false, 200.0).shadow;
    let on = p.alert(true, false);
    let off = p.alert(false, false);
    assert!(on.fill[3] > off.fill[3]);
    assert_eq!(&on.fill[..3], &[0.0, 0.0, 0.0], "a black shadow");
    assert_eq!(on.offset[0], 0.0);
    assert!(on.offset[1] > 0.0, "cast downward");
    assert_eq!(on.ring_blur, p.ring_blur);
    assert_ne!(p.alert(true, true).fill[3], on.fill[3], "dark differs");
}

#[test]
fn the_foreground_matrix_follows_the_measured_face_then_the_appearance() {
    assert_ne!(foreground_matrix(true), foreground_matrix(false));
    // (dark appearance, measured tone, matrix of a dark face)
    for (dark, tone, dark_matrix) in [
        (false, None, false),
        (true, None, true),
        (false, Some(FaceTone::Dark), true),
        (true, Some(FaceTone::Light), false),
    ] {
        assert_eq!(
            foreground_matrix_for(dark, tone),
            foreground_matrix(dark_matrix),
            "dark={dark} {tone:?}"
        );
    }
    assert_eq!(FaceTone::Dark.matrix(), foreground_matrix(true));
    assert_eq!(FaceTone::Light.matrix(), foreground_matrix(false));
}

#[test]
fn the_foreground_is_white_on_dark_glass_and_black_on_light_within_the_unit_range() {
    let mid = [0.5, 0.5, 0.5, 1.0];
    assert!(apply_matrix(&foreground_matrix(true), mid)[..3]
        .iter()
        .all(|v| *v > 0.9));
    assert!(apply_matrix(&foreground_matrix(false), mid)[..3]
        .iter()
        .all(|v| *v < 0.1));
    for dark in [false, true] {
        for tone in [None, Some(FaceTone::Dark), Some(FaceTone::Light)] {
            let rgb = foreground_rgb(dark, tone);
            assert!(
                rgb.iter().all(|v| (0.0..=1.0).contains(v)),
                "{dark} {tone:?}"
            );
        }
    }
    let mut identity = foreground_matrix(true);
    identity.fill(0.0);
    for i in 0..4 {
        identity[i * 5 + i] = 1.0;
    }
    assert_eq!(
        apply_matrix(&identity, [0.1, 0.2, 0.3, 0.4]),
        [0.1, 0.2, 0.3, 0.4]
    );
}
