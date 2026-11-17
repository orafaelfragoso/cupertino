//! Glyph rasterisation: stem darkening and exact-area scan conversion, on hand-made outlines (the toolkit's font is
//! tested in `cupertino-ui-text`).

use cupertino_raster_math::glyph::{
    declarative_size, mask_bounds, quantization, quantized_luminance, rasterize, Mask, Smoothing,
    MAX_MASK_SIDE,
};
use swash::zeno::{Point, Verb};

fn pt(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

/// A closed polygon outline, y up.
fn polygon(points: &[(f32, f32)]) -> (Vec<Point>, Vec<Verb>) {
    let pts: Vec<Point> = points.iter().map(|&(x, y)| pt(x, y)).collect();
    let mut verbs = vec![Verb::MoveTo];
    verbs.extend(std::iter::repeat_n(Verb::LineTo, points.len() - 1));
    verbs.push(Verb::Close);
    (pts, verbs)
}

fn join(parts: &[(Vec<Point>, Vec<Verb>)]) -> (Vec<Point>, Vec<Verb>) {
    let points = parts.iter().flat_map(|p| p.0.clone()).collect();
    let verbs = parts.iter().flat_map(|p| p.1.clone()).collect();
    (points, verbs)
}

/// Counter-clockwise (y up) rectangle.
fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> (Vec<Point>, Vec<Verb>) {
    polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

/// The same rectangle, clockwise.
fn rect_cw(x0: f32, y0: f32, x1: f32, y1: f32) -> (Vec<Point>, Vec<Verb>) {
    polygon(&[(x0, y0), (x0, y1), (x1, y1), (x1, y0)])
}

fn raster(
    outline: &(Vec<Point>, Vec<Verb>),
    offset: (f32, f32),
    dilation: (f32, f32),
) -> Option<Mask> {
    rasterize(&outline.0, &outline.1, offset, dilation)
}

fn ink(mask: &Mask) -> f32 {
    mask.data.iter().map(|&v| f32::from(v) / 255.0).sum()
}

fn row(mask: &Mask, y: usize) -> &[u8] {
    &mask.data[y * mask.width as usize..(y + 1) * mask.width as usize]
}

#[test]
fn luminance_is_linear_rec_709_in_quarters() {
    assert_eq!(quantized_luminance([0.0, 0.0, 0.0]), 0.0);
    assert_eq!(quantized_luminance([1.0, 1.0, 1.0]), 1.0);
    // sRGB 0.5 is 21 % linear: one quarter
    assert_eq!(quantized_luminance([0.5, 0.5, 0.5]), 0.25);
    // green carries most of the weight, blue the least
    assert_eq!(quantized_luminance([0.0, 1.0, 0.0]), 0.75);
    assert_eq!(quantized_luminance([1.0, 0.0, 0.0]), 0.25);
    assert_eq!(quantized_luminance([0.0, 0.0, 1.0]), 0.0);
    for v in [0.1, 0.3, 0.7, 0.9] {
        let q = quantized_luminance([v, v, v]);
        assert_eq!(q * 4.0, (q * 4.0).round(), "{q}");
    }
}

#[test]
fn the_declarative_size_floors_to_a_power_of_1_25() {
    for s in [1.0f32, 9.0, 12.0, 13.0, 16.0, 17.0, 24.0, 100.0] {
        let d = declarative_size(s);
        assert!(d <= s * 1.0001 && d > s / 1.25 * 0.9999, "{s} -> {d}");
        let steps = (d.ln() / 1.25f32.ln()).round();
        assert!(
            (1.25f32.powf(steps) - d).abs() < 1e-4 * d,
            "{d} is not a power of 1.25"
        );
    }
    assert_eq!(declarative_size(1.0), 1.0);
}

#[test]
fn smoothing_off_never_dilates() {
    assert_eq!(Smoothing::Off.dilation(16.0, 1.0), 0.0);
    assert_eq!(Smoothing::Off.dilation(500.0, 0.0), 0.0);
}

#[test]
fn smoothing_on_dilates_light_text_more_than_dark_and_larger_text_more_than_small() {
    let dark = Smoothing::On.dilation(16.0, 0.0);
    let light = Smoothing::On.dilation(16.0, 1.0);
    assert!((dark - 0.0058 * 16.0).abs() < 1e-6, "{dark}");
    assert!((light - 0.0158 * 16.0).abs() < 1e-6, "{light}");
    assert!(Smoothing::On.dilation(32.0, 0.5) > Smoothing::On.dilation(16.0, 0.5));
}

#[test]
fn the_dilation_is_capped_at_a_third_of_a_pixel() {
    assert_eq!(Smoothing::On.dilation(400.0, 1.0), 0.3);
    assert_eq!(Smoothing::On.dilation(1e6, 0.0), 0.3);
}

#[test]
fn subpixel_positions_are_at_most_five_a_axis_and_fewer_for_larger_text() {
    assert_eq!(quantization(1.0), (5, 5));
    assert_eq!(quantization(8.0), (5, 2));
    assert_eq!(quantization(16.0), (3, 1));
    assert_eq!(quantization(100.0), (1, 1));
    let (mut x, mut y) = (5, 5);
    for px in [2.0, 6.0, 10.0, 20.0, 40.0, 80.0, 200.0] {
        let (qx, qy) = quantization(px);
        assert!(qx <= x && qy <= y && qx >= 1 && qy >= 1);
        // y has fewer positions than x: text moves less across than along the line
        assert!(qy <= qx);
        (x, y) = (qx, qy);
    }
}

#[test]
fn mask_bounds_are_whole_pixels_around_the_bounds() {
    // left and top are as the outline sees them: x from the left edge, top the highest row above the baseline
    assert_eq!(mask_bounds((1.2, -0.5), (3.1, 4.0)), Some((1, 4, 3, 5)));
    assert_eq!(mask_bounds((-2.0, 0.0), (0.0, 3.0)), Some((-2, 3, 2, 3)));
}

#[test]
fn mask_bounds_are_at_least_one_pixel() {
    assert_eq!(mask_bounds((2.0, 2.0), (2.0, 2.0)), Some((2, 2, 1, 1)));
    assert_eq!(mask_bounds((2.3, 2.3), (2.4, 2.4)), Some((2, 3, 1, 1)));
}

#[test]
fn mask_bounds_refuse_what_is_not_finite_or_too_large() {
    assert_eq!(mask_bounds((f32::NAN, 0.0), (1.0, 1.0)), None);
    // inverted corners, on either axis or both
    assert_eq!(mask_bounds((1.0, 1.0), (5.0, 5.0)), Some((1, 5, 4, 4)));
    assert_eq!(mask_bounds((5.0, 5.0), (1.0, 1.0)), None);
    assert_eq!(mask_bounds((5.0, 1.0), (1.0, 5.0)), None);
    assert_eq!(mask_bounds((1.0, 5.0), (5.0, 1.0)), None);
    assert_eq!(mask_bounds((0.0, 0.0), (f32::INFINITY, 1.0)), None);
    assert_eq!(mask_bounds((0.0, f32::NEG_INFINITY), (1.0, 1.0)), None);
    let max = MAX_MASK_SIDE as f32;
    assert!(mask_bounds((0.0, 0.0), (max, 1.0)).is_some());
    assert_eq!(mask_bounds((0.0, 0.0), (max + 1.0, 1.0)), None);
    assert_eq!(mask_bounds((0.0, 0.0), (1.0, max + 1.0)), None);
    assert_eq!(mask_bounds((-1e12, 0.0), (1e12, 1.0)), None);
}

#[test]
fn a_pixel_aligned_rectangle_is_fully_covered() {
    let m = raster(&rect(2.0, 0.0, 6.0, 4.0), (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!((m.left, m.top, m.width, m.height), (2, 4, 4, 4));
    assert!(m.data.iter().all(|&v| v == 255), "{:?}", m.data);
}

#[test]
fn a_clockwise_outline_rasterises_like_a_counter_clockwise_one() {
    let a = raster(&rect(1.0, 0.0, 5.5, 3.25), (0.0, 0.0), (0.0, 0.0)).unwrap();
    let b = raster(&rect_cw(1.0, 0.0, 5.5, 3.25), (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_subpixel_offset_moves_the_edge_by_its_fraction() {
    let m = raster(&rect(2.0, 0.0, 6.0, 4.0), (0.5, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!((m.left, m.width), (2, 5));
    assert_eq!(row(&m, 0), &[128, 255, 255, 255, 128]);
    let m = raster(&rect(2.0, 0.0, 6.0, 4.0), (0.0, 0.25), (0.0, 0.0)).unwrap();
    assert_eq!((m.top, m.height), (5, 5));
    // the top row has a quarter of its height covered, the bottom row the rest of the shifted bottom edge
    assert_eq!(row(&m, 0)[1], 64);
    assert_eq!(row(&m, 1)[1], 255);
    assert_eq!(row(&m, 4)[1], 191);
}

#[test]
fn a_covered_area_is_the_sum_of_the_coverage() {
    let m = raster(&rect(1.3, 0.2, 4.9, 2.7), (0.0, 0.0), (0.0, 0.0)).unwrap();
    let area = (4.9 - 1.3) * (2.7 - 0.2);
    assert!((ink(&m) - area).abs() < 0.03, "{} against {area}", ink(&m));
}

#[test]
fn a_diagonal_edge_covers_the_fraction_of_each_pixel_it_cuts() {
    // a right triangle with the legs along the axes: area 8 over a 4 x 4 square
    let m = raster(
        &polygon(&[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)]),
        (0.0, 0.0),
        (0.0, 0.0),
    )
    .unwrap();
    assert!((ink(&m) - 8.0).abs() < 0.05, "{}", ink(&m));
    // the hypotenuse x + y = 4 cuts each pixel it crosses corner to corner in half
    assert_eq!(row(&m, 0), &[128, 0, 0, 0]);
    assert_eq!(row(&m, 1), &[255, 128, 0, 0]);
    assert_eq!(row(&m, 3), &[255, 255, 255, 128]);
}

#[test]
fn contours_that_overlap_add_and_clamp_to_one() {
    let both = join(&[rect(0.0, 0.0, 4.0, 4.0), rect(2.0, 0.0, 6.0, 4.0)]);
    let m = raster(&both, (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!(m.width, 6);
    assert!(row(&m, 1).iter().all(|&v| v == 255), "{:?}", row(&m, 1));
}

#[test]
fn an_inner_contour_of_the_opposite_direction_is_a_hole() {
    let ring = join(&[rect(0.0, 0.0, 6.0, 6.0), rect_cw(2.0, 2.0, 4.0, 4.0)]);
    let m = raster(&ring, (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!(row(&m, 0), &[255; 6]);
    assert_eq!(row(&m, 2), &[255, 255, 0, 0, 255, 255]);
    assert_eq!(row(&m, 3), &[255, 255, 0, 0, 255, 255]);
    assert!((ink(&m) - 32.0).abs() < 1e-3);
}

#[test]
fn a_quadratic_curve_is_flattened_to_the_right_area() {
    // a bump from (0,0) to (8,0) with the control point at (4, 8)
    let pts = vec![pt(0.0, 0.0), pt(4.0, 8.0), pt(8.0, 0.0)];
    let verbs = vec![Verb::MoveTo, Verb::QuadTo, Verb::Close];
    // the parabola's area under the chord is 2/3 of base times height (4): 21.333; the flattened polygon lies inside the
    // curve by well under the 0.02 pixel tolerance times the base
    let m = rasterize(&pts, &verbs, (0.0, 0.0), (0.0, 0.0)).unwrap();
    let area = 64.0 / 3.0;
    assert!(
        ink(&m) <= area + 0.01 && ink(&m) > area - 0.02 * 8.0 * 1.5,
        "{}",
        ink(&m)
    );
}

#[test]
fn a_cubic_curve_is_flattened_to_the_right_area() {
    // a cubic that is a quadratic raised in degree: the same parabola
    let pts = vec![
        pt(0.0, 0.0),
        pt(8.0 / 3.0, 16.0 / 3.0),
        pt(16.0 / 3.0, 16.0 / 3.0),
        pt(8.0, 0.0),
    ];
    let verbs = vec![Verb::MoveTo, Verb::CurveTo, Verb::Close];
    let m = rasterize(&pts, &verbs, (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert!((ink(&m) - 64.0 / 3.0).abs() < 0.1, "{}", ink(&m));
}

#[test]
fn a_curve_rasterises_inside_the_bounds_of_its_flattened_edges() {
    let pts = vec![pt(0.0, 0.0), pt(4.0, 8.0), pt(8.0, 0.0)];
    let verbs = vec![Verb::MoveTo, Verb::QuadTo, Verb::Close];
    let m = rasterize(&pts, &verbs, (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!((m.left, m.width), (0, 8));
    // the curve peaks at height 4
    assert_eq!((m.top, m.height), (4, 4));
}

#[test]
fn an_empty_or_degenerate_outline_has_no_mask() {
    assert!(rasterize(&[], &[], (0.0, 0.0), (0.0, 0.0)).is_none());
    let two = polygon(&[(0.0, 0.0), (5.0, 5.0)]);
    assert!(raster(&two, (0.0, 0.0), (0.0, 0.0)).is_none());
    let flat = polygon(&[(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)]);
    let m = raster(&flat, (0.0, 0.0), (0.0, 0.0));
    // a zero-area contour has bounds of at least one pixel, so it makes a mask that covers nothing
    let m = m.expect("a flat contour still gets a one pixel mask");
    assert_eq!((m.width, m.height), (10, 1));
    assert!(m.data.iter().all(|&v| v == 0));
}

#[test]
fn an_outline_that_is_not_finite_or_too_large_has_no_mask() {
    let nan = rect(0.0, 0.0, f32::NAN, 4.0);
    assert!(raster(&nan, (0.0, 0.0), (0.0, 0.0)).is_none());
    let huge = rect(0.0, 0.0, 5000.0, 4.0);
    assert!(raster(&huge, (0.0, 0.0), (0.0, 0.0)).is_none());
}

#[test]
fn dilation_moves_side_edges_out_and_the_top_edge_up_by_twice_as_much() {
    let m = raster(&rect(2.0, 0.0, 6.0, 4.0), (0.0, 0.0), (0.25, 0.25)).unwrap();
    // x from 1.75 to 6.25, y from 0 to 4.5
    assert_eq!((m.left, m.width), (1, 6));
    assert_eq!((m.top, m.height), (5, 5));
    // the top edge rose to 4.5: the top row is half covered between the sides
    assert_eq!(&row(&m, 0)[1..5], &[128; 4]);
    assert_eq!(&row(&m, 1)[1..5], &[255; 4]);
    // the bottom edge stays at 0: the last row is full between the sides
    assert_eq!(&row(&m, 4)[1..5], &[255; 4]);
    // the sides rise by the dilation too, and each corner is joined by a slanted segment: a 4.5 wide body 4 tall,
    // with a trapezoid of 4.5 to 4 wide by a quarter on top and below
    let expected = 4.5 * 4.0 + 2.0 * (4.5 + 4.0) / 2.0 * 0.25;
    assert!(
        (ink(&m) - expected).abs() < 0.05,
        "{} against {expected}",
        ink(&m)
    );
}

#[test]
fn dilation_is_the_same_for_either_winding() {
    let a = raster(&rect(2.0, 0.0, 6.0, 4.0), (0.0, 0.0), (0.2, 0.3)).unwrap();
    let b = raster(&rect_cw(2.0, 0.0, 6.0, 4.0), (0.0, 0.0), (0.2, 0.3)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn dilation_only_adds_ink() {
    let outline = join(&[rect(0.0, 0.0, 6.0, 6.0), rect_cw(2.0, 2.0, 4.0, 4.0)]);
    let plain = raster(&outline, (0.0, 0.0), (0.0, 0.0)).unwrap();
    let mut last = ink(&plain);
    for d in [0.05, 0.1, 0.2, 0.3] {
        let thick = raster(&outline, (0.0, 0.0), (d, d)).unwrap();
        assert!(ink(&thick) > last, "d = {d}");
        last = ink(&thick);
    }
}
