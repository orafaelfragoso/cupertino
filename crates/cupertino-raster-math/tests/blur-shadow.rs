//! The blur, shadows and the shrinking filter.

use cupertino_raster_math::blur::{
    gaussian_weights, plan_blur, sigma_for_radius, Edge, Image, Precision, MAX_BLUR_RADIUS,
};
use cupertino_raster_math::resample::{
    bilinear_row, catmull_rom_row, footprint, shrink, shrink_row, weights,
};
use cupertino_raster_math::shadow::{
    blur_mass, drop_shadow, erf, inner_shadow, rounded_box_blur, Caster, Shadow,
};

fn solid(w: usize, h: usize, px: [f32; 4]) -> Image {
    Image::new(w, h, vec![px; w * h])
}

fn near(a: [f32; 4], b: [f32; 4], tolerance: f32) -> bool {
    (0..4).all(|c| (a[c] - b[c]).abs() < tolerance)
}

#[test]
fn the_kernel_is_a_normalised_falling_half_gaussian_three_sigma_wide_up_to_the_shader_limit() {
    for sigma in [0.3, 0.7, 1.0, 2.5, 5.0, 11.0] {
        let w = gaussian_weights(sigma);
        let sum = w[0] + 2.0 * w[1..].iter().sum::<f32>();
        assert!((sum - 1.0).abs() < 1e-5, "sigma {sigma}: {sum}");
        assert!(
            w.windows(2).all(|p| p[0] > p[1]),
            "sigma {sigma}: weights fall"
        );
    }
    for (sigma, radius) in [
        (0.0, 0),
        (0.29, 0),
        (0.3, 1),
        (2.0, 6),
        (2.1, 7),
        (100.0, MAX_BLUR_RADIUS),
        (1e6, MAX_BLUR_RADIUS),
    ] {
        assert_eq!(gaussian_weights(sigma).len() - 1, radius, "sigma {sigma}");
    }
    assert_eq!(gaussian_weights(0.0), vec![1.0]);
    // a toolkit radius names half as much deviation
    assert_eq!(sigma_for_radius(20.0), 10.0);
    assert_eq!(sigma_for_radius(0.0), 0.0);
}

#[test]
fn a_plan_halves_to_the_backdrop_scale_and_further_for_a_wide_blur() {
    let plan = plan_blur(100, 60, 0.5, 0.0, 1.0);
    assert_eq!(plan.levels, vec![(50, 30)]);
    assert_eq!(plan.final_size(100, 60), (50, 30));
    let full = plan_blur(100, 60, 1.0, 0.0, 1.0);
    assert!(full.levels.is_empty());
    assert_eq!(full.final_size(100, 60), (100, 60));
    assert_eq!(full.weights, vec![1.0]);
    // halving never makes a side empty
    assert_eq!(plan_blur(1, 3, 0.25, 0.0, 1.0).levels, vec![(1, 1), (1, 1)]);
    // a wide blur keeps halving until the variance is small or the level is
    let wide = plan_blur(1920, 1080, 0.5, 180.0, 1.0);
    assert!(wide.levels.len() > 2);
    assert!(wide.sigma * wide.sigma <= 64.0);
    let tiny = plan_blur(32, 32, 1.0, 500.0, 1.0);
    assert!(tiny.levels.iter().all(|&(w, h)| w.min(h) >= 8));
    assert!(tiny.sigma > 8.0);
    // the display scale adds a halving
    let one = plan_blur(800, 600, 0.5, 10.0, 1.0);
    let two = plan_blur(800, 600, 0.5, 10.0, 2.0);
    assert!(two.levels.len() > one.levels.len());
}

#[test]
fn precision_stores_to_the_levels_format() {
    for (precision, v, expected) in [
        (Precision::Byte, 0.5, 128.0 / 255.0),
        (Precision::Byte, -1.0, 0.0),
        (Precision::Byte, 2.0, 1.0),
        (Precision::Half, 0.0, 0.0),
        (Precision::Half, 1.0, 1.0),
        (Precision::Half, 0.5, 0.5),
    ] {
        assert_eq!(precision.store(v), expected, "{precision:?} {v}");
    }
    // 1/3 has no half-float: the nearest one differs, within 2^-11 relative
    let third = Precision::Half.store(1.0 / 3.0);
    assert_ne!(third, 1.0 / 3.0);
    assert!((third - 1.0 / 3.0).abs() < 1.0 / 3.0 / 2048.0);
}

#[test]
fn an_edge_reads_as_the_edge_texel_or_as_transparent() {
    let red = [1.0, 0.0, 0.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let img = Image::new(2, 1, vec![red, green]);
    for ((x, y), edge, expected) in [
        ((-5, 0), Edge::Clamp, red),
        ((9, 3), Edge::Clamp, green),
        ((-1, 0), Edge::Transparent, [0.0; 4]),
        ((0, 1), Edge::Transparent, [0.0; 4]),
        ((1, 0), Edge::Transparent, green),
    ] {
        assert_eq!(img.read(x, y, edge), expected, "{x},{y} {edge:?}");
    }
}

#[test]
#[should_panic(expected = "2 x 2 pixels")]
fn an_image_needs_one_pixel_per_texel() {
    Image::new(2, 2, vec![[0.0; 4]; 3]);
}

#[test]
fn a_halving_averages_each_2_by_2_block_reading_past_an_odd_edge_by_the_edge_rule() {
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
    assert!(
        near(half.pixels[0], [0.2, 0.0, 0.0, 0.2], 1e-3),
        "{:?}",
        half.pixels
    );

    let img = solid(3, 1, [1.0; 4]);
    assert_eq!(
        img.halve((2, 1), Edge::Clamp, Precision::Half).pixels[1],
        [1.0; 4]
    );
    assert_eq!(
        img.halve((2, 1), Edge::Transparent, Precision::Half).pixels[1],
        [0.25; 4]
    );
}

#[test]
fn blurring_keeps_a_flat_image_and_a_single_tap_unchanged() {
    let colour = [0.2, 0.4, 0.6, 0.8];
    let out = solid(20, 12, colour).blur(
        &plan_blur(20, 12, 1.0, 3.0, 1.0),
        Edge::Clamp,
        Precision::Half,
    );
    assert!(out.pixels.iter().all(|&p| near(p, colour, 2e-3)));

    let img = Image::new(2, 1, vec![[0.25, 0.5, 0.75, 1.0], [0.0, 0.1, 0.2, 0.3]]);
    let out = img.convolve(&[1.0], (1, 0), Edge::Clamp, Precision::Half);
    assert!(out
        .pixels
        .iter()
        .zip(&img.pixels)
        .all(|(&a, &b)| near(a, b, 1e-3)));
}

#[test]
fn blurring_with_transparent_edges_fades_the_borders() {
    let out = solid(20, 20, [1.0; 4]).blur(
        &plan_blur(20, 20, 1.0, 3.0, 1.0),
        Edge::Transparent,
        Precision::Half,
    );
    let at = |x: usize, y: usize| out.pixels[y * 20 + x][3];
    assert!(at(0, 10) < 0.6);
    assert!(at(10, 10) > 0.99);
    assert!(at(0, 0) < at(0, 10));
}

#[test]
fn a_blur_conserves_the_mass_of_an_impulse_and_spreads_it_symmetrically() {
    let mut pixels = vec![[0.0; 4]; 21 * 21];
    pixels[10 * 21 + 10] = [1.0; 4];
    let out = Image::new(21, 21, pixels).blur(
        &plan_blur(21, 21, 1.0, 1.5, 1.0),
        Edge::Transparent,
        Precision::Half,
    );
    let alpha = |x: usize, y: usize| out.pixels[y * 21 + x][3];
    let sum: f32 = out.pixels.iter().map(|p| p[3]).sum();
    assert!((sum - 1.0).abs() < 0.01, "{sum}");
    assert!(out.pixels.iter().all(|p| p[3] <= alpha(10, 10)));
    assert!((alpha(8, 10) - alpha(12, 10)).abs() < 1e-4);
    assert!((alpha(10, 8) - alpha(10, 12)).abs() < 1e-4);
}

#[test]
fn erf_is_odd_and_close_to_the_error_function() {
    assert_eq!(erf(0.0), 0.0);
    for x in [0.1f32, 0.5, 1.0, 2.0, 3.0] {
        assert_eq!(erf(-x), -erf(x));
    }
    for (x, e) in [(0.5f32, 0.5204999), (1.0, 0.8427008), (2.0, 0.9953223)] {
        assert!((erf(x) - e).abs() < 5e-4, "erf({x}) = {}", erf(x));
    }
    assert!(erf(10.0) <= 1.0 && erf(10.0) > 0.9999);
}

#[test]
fn a_blurred_box_holds_its_mass_inside_half_at_an_edge_and_none_far_outside() {
    for sigma in [1.0, 4.0, 20.0] {
        let m = blur_mass(sigma);
        assert!(m < 1.0 && m > 0.97, "{sigma}: {m}");
    }
    let half = [60.0, 40.0];
    for (p, sigma, expected, tolerance) in [
        ([0.0, 0.0], 3.0, blur_mass(3.0), 1e-3),
        ([60.0, 0.0], 4.0, 0.5 * blur_mass(4.0), 0.01),
        ([100.0, 0.0], 3.0, 0.0, 1e-3),
        ([0.0, 80.0], 3.0, 0.0, 1e-3),
    ] {
        let v = rounded_box_blur(p, half, sigma, 0.0);
        assert!((v - expected).abs() < tolerance, "{p:?}: {v}");
    }
}

#[test]
fn a_blurred_box_is_symmetric_falls_off_outwards_and_is_lighter_at_a_round_corner() {
    let half = [30.0, 20.0];
    let mut last = f32::MAX;
    for i in 0..12 {
        let x = 20.0 + 3.0 * i as f32;
        let v = rounded_box_blur([x, 0.0], half, 4.0, 5.0);
        assert!(v <= last + 1e-6, "{x}: {v} after {last}");
        assert!((v - rounded_box_blur([-x, 0.0], half, 4.0, 5.0)).abs() < 1e-5);
        last = v;
    }
    let square = rounded_box_blur([28.0, 18.0], half, 3.0, 0.0);
    let round = rounded_box_blur([28.0, 18.0], half, 3.0, 12.0);
    assert!(round < square);
}

fn caster() -> Caster {
    Caster {
        half: [60.0, 40.0],
        radius: 10.0,
        extent: 10.0,
        exponent: 2.0,
    }
}

fn shadow(sigma: f32, offset: [f32; 2], density: f32) -> Shadow {
    Shadow {
        sigma,
        offset,
        density,
    }
}

#[test]
fn a_drop_shadow_follows_its_offset_and_scales_with_density_up_to_one() {
    let below = drop_shadow(
        [0.0, 50.0],
        &caster(),
        &shadow(4.0, [0.0, 12.0], 1.0),
        false,
        1.0,
    );
    let above = drop_shadow(
        [0.0, -50.0],
        &caster(),
        &shadow(4.0, [0.0, 12.0], 1.0),
        false,
        1.0,
    );
    assert!(below > 0.3, "{below}");
    assert!(above < 0.01, "{above}");
    for (density, expected) in [(0.5, 0.5 * blur_mass(4.0)), (5.0, 1.0), (0.0, 0.0)] {
        let v = drop_shadow(
            [0.0, 0.0],
            &caster(),
            &shadow(4.0, [0.0; 2], density),
            false,
            1.0,
        );
        assert!((v - expected).abs() < 1e-3, "density {density}: {v}");
    }
}

#[test]
fn a_cut_out_leaves_only_the_casters_own_area_clear() {
    let s = shadow(4.0, [0.0, 2.0], 1.0);
    assert_eq!(drop_shadow([0.0, 0.0], &caster(), &s, true, 1.0), 0.0);
    assert!(drop_shadow([0.0, 0.0], &caster(), &s, false, 1.0) > 0.9);
    let outside = [75.0, 0.0];
    assert_eq!(
        drop_shadow(outside, &caster(), &s, true, 1.0),
        drop_shadow(outside, &caster(), &s, false, 1.0)
    );
}

#[test]
fn an_oversized_radius_is_held_to_the_casters_half_size() {
    let s = shadow(3.0, [0.0; 2], 1.0);
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
            drop_shadow(p, &big, &s, true, 1.0),
            drop_shadow(p, &held, &s, true, 1.0)
        );
        assert_eq!(
            inner_shadow(p, &big, &s, 1.0),
            inner_shadow(p, &held, &s, 1.0)
        );
    }
}

#[test]
fn an_inner_shadow_hugs_the_edges_and_leaves_the_middle_and_outside_clear() {
    let s = shadow(4.0, [0.0, 3.0], 1.0);
    assert_eq!(inner_shadow([0.0, 0.0], &caster(), &s, 1.0), 0.0);
    assert_eq!(inner_shadow([90.0, 0.0], &caster(), &s, 1.0), 0.0);
    // the offset pushes the shadow in from the top edge
    let top = inner_shadow([0.0, -38.0], &caster(), &s, 1.0);
    let bottom = inner_shadow([0.0, 38.0], &caster(), &s, 1.0);
    assert!(top > 0.2, "{top}");
    assert!(top > bottom);
}

#[test]
fn area_weights_are_the_covered_fraction_of_each_texel_in_the_footprint() {
    assert_eq!(footprint(0, 0.75), (0.0, 4.0 / 3.0));
    assert_eq!(footprint(3, 0.5), (6.0, 8.0));
    // 0.75: pixel 0 covers texels 0 (whole) and 1 (a third), of 4/3 total
    let w = weights(0, 0.75, 8);
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].0, 0);
    assert!(
        (w[0].1 - 0.75).abs() < 1e-5 && (w[1].1 - 0.25).abs() < 1e-5,
        "{w:?}"
    );
    for (len, out, ratio) in [
        (8u32, 6u32, 0.75f32),
        (10, 5, 0.5),
        (9, 3, 1.0 / 3.0),
        (7, 7, 1.0),
        (3, 2, 0.4),
    ] {
        for i in 0..out {
            let w = weights(i, ratio, len);
            let sum: f32 = w.iter().map(|(_, w)| w).sum();
            assert!((sum - 1.0).abs() < 1e-5, "{len} {out} pixel {i}: {sum}");
            // texels past the edge count as the edge
            assert!(
                w.iter().all(|&(t, w)| t < len && w > 0.0),
                "{len} {out} pixel {i}"
            );
        }
    }
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
fn the_area_filter_keeps_flat_rows_flat_never_overshoots_and_keeps_a_thin_lines_weight() {
    let out = shrink_row(&[0.7; 8], 6, 0.75);
    assert_eq!(out.len(), 6);
    assert!(out.iter().all(|v| (v - 0.7).abs() < 1e-5));
    assert!((shrink_row(&[1.0, 1.0, 1.0], 1, 0.4)[0] - 1.0).abs() < 1e-5);

    let mut step = vec![0.0f32; 24];
    step[12..].fill(1.0);
    assert!(shrink_row(&step, 18, 0.75)
        .iter()
        .all(|&v| (0.0..=1.0).contains(&v)));
    assert!(
        catmull_rom_row(&step, 18, 0.75)
            .iter()
            .any(|&v| !(0.0..=1.0).contains(&v)),
        "the sharper filter rings at a step"
    );

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
fn shrinking_an_image_filters_both_axes_and_the_same_size_is_the_identity() {
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
    for (i, &px) in out.iter().enumerate() {
        let v = i as f32 / 4.0;
        assert!(near(px, [v, v, v, 1.0], 1e-6), "pixel {i}: {px:?}");
    }

    let image: Vec<[f32; 4]> = (0..6).map(|i| [i as f32 / 6.0, 0.0, 0.5, 1.0]).collect();
    let out = shrink(&image, (3, 2), (3, 2));
    assert!(out.iter().zip(&image).all(|(&a, &b)| near(a, b, 1e-6)));
}
