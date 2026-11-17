//! The blur, shadows and the shrinking filter.

use cupertino_raster_math::blur::{
    gaussian_weights, plan_blur, sigma_for_radius, Edge, Image, Precision, MAX_BLUR_RADIUS,
};
use cupertino_raster_math::resample::{
    bilinear_row, catmull_rom_row, footprint, shrink, shrink_row, weights, GLSL_TAPS,
};
use cupertino_raster_math::shadow::{
    blur_mass, drop_shadow, erf, inner_shadow, rounded_box_blur, Caster, Shadow, SHADOW_STEPS,
};

fn solid(w: usize, h: usize, px: [f32; 4]) -> Image {
    Image::new(w, h, vec![px; w * h])
}

#[test]
fn kernel_sums_to_one_over_both_sides() {
    for sigma in [0.3, 0.7, 1.0, 2.5, 5.0, 11.0] {
        let w = gaussian_weights(sigma);
        let sum = w[0] + 2.0 * w[1..].iter().sum::<f32>();
        assert!((sum - 1.0).abs() < 1e-5, "sigma {sigma}: {sum}");
        assert!(
            w.windows(2).all(|p| p[0] > p[1]),
            "weights fall with distance"
        );
    }
}

#[test]
fn a_small_sigma_is_a_single_tap() {
    assert_eq!(gaussian_weights(0.0), vec![1.0]);
    assert_eq!(gaussian_weights(0.29), vec![1.0]);
    assert_eq!(gaussian_weights(0.3).len(), 2);
}

#[test]
fn the_kernel_radius_is_three_sigma_up_to_the_shader_limit() {
    assert_eq!(gaussian_weights(2.0).len() - 1, 6);
    assert_eq!(gaussian_weights(2.1).len() - 1, 7);
    assert_eq!(gaussian_weights(100.0).len() - 1, MAX_BLUR_RADIUS);
    assert_eq!(gaussian_weights(1e6).len() - 1, MAX_BLUR_RADIUS);
}

#[test]
fn a_toolkit_radius_names_half_as_much_deviation() {
    assert_eq!(sigma_for_radius(20.0), 10.0);
    assert_eq!(sigma_for_radius(0.0), 0.0);
}

#[test]
fn a_plan_halves_to_the_backdrop_scale() {
    let plan = plan_blur(100, 60, 0.5, 0.0, 1.0);
    assert_eq!(plan.levels, vec![(50, 30)]);
    assert_eq!(plan.final_size(100, 60), (50, 30));
    let full = plan_blur(100, 60, 1.0, 0.0, 1.0);
    assert!(full.levels.is_empty());
    assert_eq!(full.final_size(100, 60), (100, 60));
    assert_eq!(full.weights, vec![1.0]);
}

#[test]
fn halving_never_makes_a_side_empty() {
    let plan = plan_blur(1, 3, 0.25, 0.0, 1.0);
    assert_eq!(plan.levels, vec![(1, 1), (1, 1)]);
}

#[test]
fn a_wide_blur_keeps_halving_until_the_variance_is_small_or_the_level_is() {
    let wide = plan_blur(1920, 1080, 0.5, 180.0, 1.0);
    assert!(wide.levels.len() > 2);
    assert!(wide.sigma * wide.sigma <= 64.0);
    // a tiny image stops at 16 texels whatever the blur
    let tiny = plan_blur(32, 32, 1.0, 500.0, 1.0);
    assert!(tiny.levels.iter().all(|&(w, h)| w.min(h) >= 8));
    assert!(tiny.sigma > 8.0);
}

#[test]
fn the_display_scale_adds_a_halving_to_a_wide_blur() {
    let one = plan_blur(800, 600, 0.5, 10.0, 1.0);
    let two = plan_blur(800, 600, 0.5, 10.0, 2.0);
    assert!(two.levels.len() > one.levels.len());
}

#[test]
fn precision_stores_to_the_levels_format() {
    assert_eq!(Precision::Byte.store(0.5), 128.0 / 255.0);
    assert_eq!(Precision::Byte.store(-1.0), 0.0);
    assert_eq!(Precision::Byte.store(2.0), 1.0);
    assert_eq!(Precision::Half.store(0.0), 0.0);
    assert_eq!(Precision::Half.store(1.0), 1.0);
    assert_eq!(Precision::Half.store(0.5), 0.5);
    // 1/3 has no half-float: the nearest one differs, within 2^-11 relative
    let third = Precision::Half.store(1.0 / 3.0);
    assert_ne!(third, 1.0 / 3.0);
    assert!((third - 1.0 / 3.0).abs() < 1.0 / 3.0 / 2048.0);
}

#[test]
fn an_edge_reads_as_the_edge_texel_or_as_transparent() {
    let img = Image::new(2, 1, vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]]);
    assert_eq!(img.read(-5, 0, Edge::Clamp), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(img.read(9, 3, Edge::Clamp), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(img.read(-1, 0, Edge::Transparent), [0.0; 4]);
    assert_eq!(img.read(0, 1, Edge::Transparent), [0.0; 4]);
    assert_eq!(img.read(1, 0, Edge::Transparent), [0.0, 1.0, 0.0, 1.0]);
}

#[test]
#[should_panic(expected = "2 x 2 pixels")]
fn an_image_needs_one_pixel_per_texel() {
    Image::new(2, 2, vec![[0.0; 4]; 3]);
}

#[test]
fn a_halving_averages_each_2_by_2_block() {
    let img = Image::new(
        2,
        2,
        vec![
            [0.0; 4],
            [0.4, 0.0, 0.0, 0.4],
            [0.0; 4],
            [0.4, 0.0, 0.0, 0.4],
        ],
    );
    let half = img.halve((1, 1), Edge::Clamp, Precision::Half);
    assert_eq!(half.pixels.len(), 1);
    assert!((half.pixels[0][0] - 0.2).abs() < 1e-3);
    assert!((half.pixels[0][3] - 0.2).abs() < 1e-3);
}

#[test]
fn a_halving_of_an_odd_edge_reads_past_it_by_the_edge_rule() {
    let img = Image::new(3, 1, vec![[1.0; 4], [1.0; 4], [1.0; 4]]);
    let clamped = img.halve((2, 1), Edge::Clamp, Precision::Half);
    assert_eq!(clamped.pixels[1], [1.0; 4]);
    let transparent = img.halve((2, 1), Edge::Transparent, Precision::Half);
    assert_eq!(transparent.pixels[1], [0.25; 4]);
}

#[test]
fn blurring_a_flat_image_with_clamped_edges_changes_nothing() {
    let img = solid(20, 12, [0.2, 0.4, 0.6, 0.8]);
    let plan = plan_blur(20, 12, 1.0, 3.0, 1.0);
    let out = img.blur(&plan, Edge::Clamp, Precision::Half);
    for p in &out.pixels {
        for c in 0..4 {
            assert!((p[c] - [0.2, 0.4, 0.6, 0.8][c]).abs() < 2e-3, "{p:?}");
        }
    }
}

#[test]
fn blurring_with_transparent_edges_fades_the_borders() {
    let img = solid(20, 20, [1.0; 4]);
    let plan = plan_blur(20, 20, 1.0, 3.0, 1.0);
    let out = img.blur(&plan, Edge::Transparent, Precision::Half);
    let at = |x: usize, y: usize| out.pixels[y * 20 + x][3];
    assert!(at(0, 10) < 0.6);
    assert!(at(10, 10) > 0.99);
    assert!(at(0, 0) < at(0, 10));
}

#[test]
fn a_blur_conserves_the_mass_of_an_impulse_away_from_the_edges() {
    let mut pixels = vec![[0.0; 4]; 21 * 21];
    pixels[10 * 21 + 10] = [1.0; 4];
    let img = Image::new(21, 21, pixels);
    let plan = plan_blur(21, 21, 1.0, 1.5, 1.0);
    let out = img.blur(&plan, Edge::Transparent, Precision::Half);
    let sum: f32 = out.pixels.iter().map(|p| p[3]).sum();
    assert!((sum - 1.0).abs() < 0.01, "{sum}");
    let peak = out.pixels[10 * 21 + 10][3];
    assert!(out.pixels.iter().all(|p| p[3] <= peak));
    // symmetric about the impulse
    assert!((out.pixels[10 * 21 + 8][3] - out.pixels[10 * 21 + 12][3]).abs() < 1e-4);
    assert!((out.pixels[8 * 21 + 10][3] - out.pixels[12 * 21 + 10][3]).abs() < 1e-4);
}

#[test]
fn convolving_with_a_single_tap_is_the_identity() {
    let img = Image::new(2, 1, vec![[0.25, 0.5, 0.75, 1.0], [0.0, 0.1, 0.2, 0.3]]);
    let out = img.convolve(&[1.0], (1, 0), Edge::Clamp, Precision::Half);
    assert!(out
        .pixels
        .iter()
        .zip(&img.pixels)
        .all(|(a, b)| (0..4).all(|c| (a[c] - b[c]).abs() < 1e-3)));
}

#[test]
fn erf_is_odd_and_close_to_the_error_function() {
    assert_eq!(erf(0.0), 0.0);
    for x in [0.1f32, 0.5, 1.0, 2.0, 3.0] {
        assert_eq!(erf(-x), -erf(x));
    }
    // reference values of erf
    for (x, e) in [(0.5f32, 0.5204999), (1.0, 0.8427008), (2.0, 0.9953223)] {
        assert!((erf(x) - e).abs() < 5e-4, "erf({x}) = {}", erf(x));
    }
    assert!(erf(10.0) <= 1.0 && erf(10.0) > 0.9999);
}

#[test]
fn a_blurred_box_is_near_one_inside_and_near_zero_far_outside() {
    let half = [60.0, 40.0];
    assert!((rounded_box_blur([0.0, 0.0], half, 3.0, 0.0) - blur_mass(3.0)).abs() < 1e-3);
    assert!(rounded_box_blur([100.0, 0.0], half, 3.0, 0.0) < 1e-3);
    assert!(rounded_box_blur([0.0, 80.0], half, 3.0, 0.0) < 1e-3);
}

#[test]
fn a_blurred_box_is_half_as_dense_at_a_straight_edge() {
    let half = [60.0, 40.0];
    let at_edge = rounded_box_blur([60.0, 0.0], half, 4.0, 0.0);
    assert!((at_edge - 0.5 * blur_mass(4.0)).abs() < 0.01, "{at_edge}");
}

#[test]
fn a_blurred_box_is_symmetric_and_falls_off_outwards() {
    let half = [30.0, 20.0];
    let mut last = f32::MAX;
    for i in 0..12 {
        let x = 20.0 + 3.0 * i as f32;
        let v = rounded_box_blur([x, 0.0], half, 4.0, 5.0);
        assert!(v <= last + 1e-6, "{x}: {v} after {last}");
        assert!((v - rounded_box_blur([-x, 0.0], half, 4.0, 5.0)).abs() < 1e-5);
        last = v;
    }
}

#[test]
fn a_corner_radius_lightens_the_blur_at_the_corner() {
    let half = [30.0, 20.0];
    let square = rounded_box_blur([28.0, 18.0], half, 3.0, 0.0);
    let round = rounded_box_blur([28.0, 18.0], half, 3.0, 12.0);
    assert!(round < square);
}

#[test]
fn the_blur_mass_is_just_under_one() {
    for sigma in [1.0, 4.0, 20.0] {
        let m = blur_mass(sigma);
        assert!(m < 1.0 && m > 0.97, "{sigma}: {m}");
    }
    assert_eq!(SHADOW_STEPS, 8);
}

fn caster() -> Caster {
    Caster {
        half: [60.0, 40.0],
        radius: 10.0,
        extent: 10.0,
        exponent: 2.0,
    }
}

#[test]
fn a_drop_shadow_follows_its_offset() {
    let shadow = Shadow {
        sigma: 4.0,
        offset: [0.0, 12.0],
        density: 1.0,
    };
    let below = drop_shadow([0.0, 50.0], &caster(), &shadow, false, 1.0);
    let above = drop_shadow([0.0, -50.0], &caster(), &shadow, false, 1.0);
    assert!(below > 0.3, "{below}");
    assert!(above < 0.01, "{above}");
}

#[test]
fn a_drop_shadow_scales_with_density_and_holds_at_one() {
    let mut shadow = Shadow {
        sigma: 4.0,
        offset: [0.0; 2],
        density: 0.5,
    };
    let half = drop_shadow([0.0, 0.0], &caster(), &shadow, false, 1.0);
    shadow.density = 5.0;
    let over = drop_shadow([0.0, 0.0], &caster(), &shadow, false, 1.0);
    assert!((half - 0.5 * blur_mass(4.0)).abs() < 1e-3, "{half}");
    assert_eq!(over, 1.0);
    shadow.density = 0.0;
    assert_eq!(drop_shadow([0.0, 0.0], &caster(), &shadow, false, 1.0), 0.0);
}

#[test]
fn a_cut_out_leaves_the_casters_own_area_clear() {
    let shadow = Shadow {
        sigma: 4.0,
        offset: [0.0, 2.0],
        density: 1.0,
    };
    assert_eq!(drop_shadow([0.0, 0.0], &caster(), &shadow, true, 1.0), 0.0);
    assert!(drop_shadow([0.0, 0.0], &caster(), &shadow, false, 1.0) > 0.9);
    // outside the caster the cut-out changes nothing
    let outside = [75.0, 0.0];
    assert_eq!(
        drop_shadow(outside, &caster(), &shadow, true, 1.0),
        drop_shadow(outside, &caster(), &shadow, false, 1.0)
    );
}

#[test]
fn an_oversized_radius_is_held_to_the_casters_half_size() {
    let shadow = Shadow {
        sigma: 3.0,
        offset: [0.0; 2],
        density: 1.0,
    };
    let big = Caster {
        radius: 500.0,
        extent: 500.0,
        ..caster()
    };
    let held = Caster {
        radius: 40.0,
        extent: 40.0,
        ..caster()
    };
    for p in [[0.0, 0.0], [55.0, 30.0], [70.0, 0.0]] {
        assert_eq!(
            drop_shadow(p, &big, &shadow, true, 1.0),
            drop_shadow(p, &held, &shadow, true, 1.0)
        );
        assert_eq!(
            inner_shadow(p, &big, &shadow, 1.0),
            inner_shadow(p, &held, &shadow, 1.0)
        );
    }
}

#[test]
fn an_inner_shadow_hugs_the_edges_and_leaves_the_middle_and_outside_clear() {
    let shadow = Shadow {
        sigma: 4.0,
        offset: [0.0, 3.0],
        density: 1.0,
    };
    assert_eq!(inner_shadow([0.0, 0.0], &caster(), &shadow, 1.0), 0.0);
    assert_eq!(inner_shadow([90.0, 0.0], &caster(), &shadow, 1.0), 0.0);
    // the offset pushes the shadow in from the top edge
    let top = inner_shadow([0.0, -38.0], &caster(), &shadow, 1.0);
    let bottom = inner_shadow([0.0, 38.0], &caster(), &shadow, 1.0);
    assert!(top > 0.2, "{top}");
    assert!(top > bottom);
}

#[test]
fn the_footprint_of_a_pixel_is_its_span_in_the_larger_image() {
    assert_eq!(footprint(0, 0.75), (0.0, 4.0 / 3.0));
    assert_eq!(footprint(3, 0.5), (6.0, 8.0));
}

#[test]
fn area_weights_sum_to_one_and_are_never_negative() {
    for (len, out, ratio) in [
        (8u32, 6u32, 0.75f32),
        (10, 5, 0.5),
        (9, 3, 1.0 / 3.0),
        (7, 7, 1.0),
    ] {
        for i in 0..out {
            let w = weights(i, ratio, len);
            let sum: f32 = w.iter().map(|(_, w)| w).sum();
            assert!((sum - 1.0).abs() < 1e-5, "{len} {out} pixel {i}: {sum}");
            assert!(w.iter().all(|&(t, w)| t < len && w > 0.0));
        }
    }
}

#[test]
fn area_weights_follow_the_covered_fraction_of_each_texel() {
    // 0.75: pixel 0 covers texels 0 (whole) and 1 (a third), of 4/3 total
    let w = weights(0, 0.75, 8);
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].0, 0);
    assert!((w[0].1 - 0.75).abs() < 1e-5);
    assert!((w[1].1 - 0.25).abs() < 1e-5);
}

#[test]
fn every_texel_gives_the_same_total_to_the_pixels_it_reaches() {
    let (len, out, ratio) = (12u32, 9u32, 0.75f32);
    let mut given = vec![0.0f32; len as usize];
    for i in 0..out {
        for (t, w) in weights(i, ratio, len) {
            // the weights are normalised: a pixel covers 1 / ratio texels in all
            given[t as usize] += w / ratio;
        }
    }
    for (t, g) in given.iter().enumerate() {
        assert!((g - 1.0).abs() < 1e-4, "texel {t}: {g}");
    }
}

#[test]
fn texels_past_the_edge_count_as_the_edge() {
    // 3 texels at ratio 0.4: the last pixel's footprint runs past the end
    let w = weights(1, 0.4, 3);
    assert!(w.iter().all(|&(t, _)| t < 3));
    let row = shrink_row(&[1.0, 1.0, 1.0], 1, 0.4);
    assert!((row[0] - 1.0).abs() < 1e-5);
}

#[test]
fn shrinking_a_flat_row_keeps_it_flat() {
    let out = shrink_row(&[0.7; 8], 6, 0.75);
    assert_eq!(out.len(), 6);
    assert!(out.iter().all(|v| (v - 0.7).abs() < 1e-5));
}

#[test]
fn the_area_filter_never_overshoots_where_catmull_rom_rings() {
    let mut row = vec![0.0f32; 24];
    row[12..].fill(1.0);
    let area = shrink_row(&row, 18, 0.75);
    assert!(area.iter().all(|&v| (0.0..=1.0).contains(&v)));
    let cubic = catmull_rom_row(&row, 18, 0.75);
    assert!(
        cubic.iter().any(|&v| !(0.0..=1.0).contains(&v)),
        "the sharper filter rings at a step"
    );
}

#[test]
fn a_thin_line_keeps_its_weight_under_the_area_filter_but_not_under_bilinear() {
    let spread = |f: &dyn Fn(&[f32]) -> Vec<f32>| {
        let sums: Vec<f32> = (0..3)
            .map(|shift| {
                let mut row = vec![0.0f32; 24];
                row[9 + shift] = 1.0;
                f(&row).iter().sum()
            })
            .collect();
        sums.iter().cloned().fold(f32::MIN, f32::max)
            - sums.iter().cloned().fold(f32::MAX, f32::min)
    };
    let area = spread(&|row| shrink_row(row, 18, 0.75));
    let bilinear = spread(&|row| bilinear_row(row, 18, 0.75));
    assert!(area < 1e-4, "{area}");
    assert!(bilinear > 0.05, "{bilinear}");
}

#[test]
fn shrinking_an_image_multiplies_the_axes() {
    // a 4 x 4 image of 2 x 2 blocks halves to its block colours
    let block = |x: u32, y: u32| {
        let v = (x / 2 + 2 * (y / 2)) as f32 / 4.0;
        [v, v, v, 1.0]
    };
    let image: Vec<[f32; 4]> = (0..4)
        .flat_map(|y| (0..4).map(move |x| block(x, y)))
        .collect();
    let out = shrink(&image, (4, 4), (2, 2));
    assert_eq!(out.len(), 4);
    for (i, px) in out.iter().enumerate() {
        let v = i as f32 / 4.0;
        for c in 0..3 {
            assert!((px[c] - v).abs() < 1e-6, "pixel {i}: {px:?}");
        }
        assert!((px[3] - 1.0).abs() < 1e-6);
    }
}

#[test]
fn shrinking_to_the_same_size_is_the_identity() {
    let image: Vec<[f32; 4]> = (0..6).map(|i| [i as f32 / 6.0, 0.0, 0.5, 1.0]).collect();
    let out = shrink(&image, (3, 2), (3, 2));
    for (a, b) in out.iter().zip(&image) {
        for c in 0..4 {
            assert!((a[c] - b[c]).abs() < 1e-6);
        }
    }
}

#[test]
fn the_shader_taps_declare_the_functions_the_compositor_calls() {
    assert!(GLSL_TAPS
        .contains("vec4 area_shrink(sampler2D image, vec2 centre, vec2 footprint, vec2 texels)"));
    assert!(GLSL_TAPS.contains("void area_taps("));
}
