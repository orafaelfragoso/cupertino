//! Glyph rasterisation: stem darkening and exact-area scan conversion, on hand-made outlines (the toolkit's font is
//! tested in `cupertino-ui-text`).

use cupertino_raster_math::glyph::{
    declarative_size, mask_bounds, quantization, quantized_luminance, rasterize, Mask, Smoothing,
    MAX_MASK_SIDE,
};
use swash::zeno::{Point, Verb};

type Outline = (Vec<Point>, Vec<Verb>);

fn pt(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

/// A closed polygon outline, y up.
fn polygon(points: &[(f32, f32)]) -> Outline {
    let pts: Vec<Point> = points.iter().map(|&(x, y)| pt(x, y)).collect();
    let mut verbs = vec![Verb::MoveTo];
    verbs.extend(std::iter::repeat_n(Verb::LineTo, points.len() - 1));
    verbs.push(Verb::Close);
    (pts, verbs)
}

fn join(parts: &[Outline]) -> Outline {
    let points = parts.iter().flat_map(|p| p.0.clone()).collect();
    let verbs = parts.iter().flat_map(|p| p.1.clone()).collect();
    (points, verbs)
}

/// Counter-clockwise (y up) rectangle.
fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Outline {
    polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

/// The same rectangle, clockwise.
fn rect_cw(x0: f32, y0: f32, x1: f32, y1: f32) -> Outline {
    polygon(&[(x0, y0), (x0, y1), (x1, y1), (x1, y0)])
}

fn raster(outline: &Outline, offset: (f32, f32), dilation: (f32, f32)) -> Option<Mask> {
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
    for (rgb, expected) in [
        ([0.0, 0.0, 0.0], 0.0),
        ([1.0, 1.0, 1.0], 1.0),
        // sRGB 0.5 is 21 % linear: one quarter
        ([0.5, 0.5, 0.5], 0.25),
        // green carries most of the weight, blue the least
        ([0.0, 1.0, 0.0], 0.75),
        ([1.0, 0.0, 0.0], 0.25),
        ([0.0, 0.0, 1.0], 0.0),
    ] {
        assert_eq!(quantized_luminance(rgb), expected, "{rgb:?}");
    }
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
fn stem_darkening_grows_with_size_and_lightness_up_to_a_third_of_a_pixel() {
    for (smoothing, size, luminance, expected) in [
        (Smoothing::Off, 16.0, 1.0, 0.0),
        (Smoothing::Off, 500.0, 0.0, 0.0),
        (Smoothing::On, 16.0, 0.0, 0.0058 * 16.0),
        (Smoothing::On, 16.0, 1.0, 0.0158 * 16.0),
        (Smoothing::On, 400.0, 1.0, 0.3),
        (Smoothing::On, 1e6, 0.0, 0.3),
    ] {
        let d = smoothing.dilation(size, luminance);
        assert!(
            (d - expected).abs() < 1e-6,
            "{smoothing:?} {size} {luminance}: {d}"
        );
    }
    assert!(Smoothing::On.dilation(32.0, 0.5) > Smoothing::On.dilation(16.0, 0.5));
}

#[test]
fn subpixel_positions_are_at_most_five_a_axis_and_fewer_for_larger_text() {
    for (px, expected) in [
        (1.0, (5, 5)),
        (8.0, (5, 2)),
        (16.0, (3, 1)),
        (100.0, (1, 1)),
    ] {
        assert_eq!(quantization(px), expected, "{px}");
    }
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
fn mask_bounds_are_whole_pixels_and_refuse_what_is_inverted_not_finite_or_too_large() {
    let max = MAX_MASK_SIDE as f32;
    for (min, bound, expected) in [
        // left and top are as the outline sees them: x from the left edge, top the highest row above the baseline
        ((1.2, -0.5), (3.1, 4.0), Some((1, 4, 3, 5))),
        ((-2.0, 0.0), (0.0, 3.0), Some((-2, 3, 2, 3))),
        ((1.0, 1.0), (5.0, 5.0), Some((1, 5, 4, 4))),
        // at least one pixel
        ((2.0, 2.0), (2.0, 2.0), Some((2, 2, 1, 1))),
        ((2.3, 2.3), (2.4, 2.4), Some((2, 3, 1, 1))),
        ((0.0, 0.0), (max, 1.0), Some((0, 1, MAX_MASK_SIDE, 1))),
        // inverted corners, on either axis or both
        ((5.0, 5.0), (1.0, 1.0), None),
        ((5.0, 1.0), (1.0, 5.0), None),
        ((1.0, 5.0), (5.0, 1.0), None),
        ((f32::NAN, 0.0), (1.0, 1.0), None),
        ((0.0, 0.0), (f32::INFINITY, 1.0), None),
        ((0.0, f32::NEG_INFINITY), (1.0, 1.0), None),
        ((0.0, 0.0), (max + 1.0, 1.0), None),
        ((0.0, 0.0), (1.0, max + 1.0), None),
        ((-1e12, 0.0), (1e12, 1.0), None),
    ] {
        assert_eq!(mask_bounds(min, bound), expected, "{min:?} to {bound:?}");
    }
}

#[test]
fn coverage_is_the_exact_area_inside_the_outline() {
    let m = raster(&rect(2.0, 0.0, 6.0, 4.0), (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!((m.left, m.top, m.width, m.height), (2, 4, 4, 4));
    assert!(m.data.iter().all(|&v| v == 255), "{:?}", m.data);

    let m = raster(&rect(1.3, 0.2, 4.9, 2.7), (0.0, 0.0), (0.0, 0.0)).unwrap();
    let area = (4.9 - 1.3) * (2.7 - 0.2);
    assert!((ink(&m) - area).abs() < 0.03, "{} against {area}", ink(&m));

    // a right triangle with the legs along the axes: area 8 over a 4 x 4 square, the hypotenuse x + y = 4 cutting
    // each pixel it crosses corner to corner in half
    let m = raster(
        &polygon(&[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)]),
        (0.0, 0.0),
        (0.0, 0.0),
    )
    .unwrap();
    assert!((ink(&m) - 8.0).abs() < 0.05, "{}", ink(&m));
    assert_eq!(row(&m, 0), &[128, 0, 0, 0]);
    assert_eq!(row(&m, 1), &[255, 128, 0, 0]);
    assert_eq!(row(&m, 3), &[255, 255, 255, 128]);
}

#[test]
fn the_winding_does_not_change_the_mask_with_or_without_dilation() {
    for (outline, dilation) in [
        ((1.0, 0.0, 5.5, 3.25), (0.0, 0.0)),
        ((2.0, 0.0, 6.0, 4.0), (0.2, 0.3)),
    ] {
        let (x0, y0, x1, y1) = outline;
        let a = raster(&rect(x0, y0, x1, y1), (0.0, 0.0), dilation).unwrap();
        let b = raster(&rect_cw(x0, y0, x1, y1), (0.0, 0.0), dilation).unwrap();
        assert_eq!(a, b, "{outline:?} dilated {dilation:?}");
    }
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
fn overlapping_contours_clamp_to_one_and_an_opposite_inner_contour_is_a_hole() {
    let both = join(&[rect(0.0, 0.0, 4.0, 4.0), rect(2.0, 0.0, 6.0, 4.0)]);
    let m = raster(&both, (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!(m.width, 6);
    assert!(row(&m, 1).iter().all(|&v| v == 255), "{:?}", row(&m, 1));

    let ring = join(&[rect(0.0, 0.0, 6.0, 6.0), rect_cw(2.0, 2.0, 4.0, 4.0)]);
    let m = raster(&ring, (0.0, 0.0), (0.0, 0.0)).unwrap();
    assert_eq!(row(&m, 0), &[255; 6]);
    assert_eq!(row(&m, 2), &[255, 255, 0, 0, 255, 255]);
    assert_eq!(row(&m, 3), &[255, 255, 0, 0, 255, 255]);
    assert!((ink(&m) - 32.0).abs() < 1e-3);
}

#[test]
fn curves_are_flattened_to_their_area_inside_their_bounds() {
    // a bump from (0,0) to (8,0) peaking at height 4; the parabola's area under the chord is 64 / 3. The flattened
    // quadratic lies inside the curve by well under the 0.02 pixel tolerance times the base.
    let area = 64.0 / 3.0;
    let quad = rasterize(
        &[pt(0.0, 0.0), pt(4.0, 8.0), pt(8.0, 0.0)],
        &[Verb::MoveTo, Verb::QuadTo, Verb::Close],
        (0.0, 0.0),
        (0.0, 0.0),
    )
    .unwrap();
    assert!(
        ink(&quad) <= area + 0.01 && ink(&quad) > area - 0.02 * 8.0 * 1.5,
        "{}",
        ink(&quad)
    );
    assert_eq!((quad.left, quad.width, quad.top, quad.height), (0, 8, 4, 4));
    // a cubic that is the same quadratic raised in degree
    let cubic = rasterize(
        &[
            pt(0.0, 0.0),
            pt(8.0 / 3.0, 16.0 / 3.0),
            pt(16.0 / 3.0, 16.0 / 3.0),
            pt(8.0, 0.0),
        ],
        &[Verb::MoveTo, Verb::CurveTo, Verb::Close],
        (0.0, 0.0),
        (0.0, 0.0),
    )
    .unwrap();
    assert!((ink(&cubic) - area).abs() < 0.1, "{}", ink(&cubic));
}

#[test]
fn an_outline_that_is_empty_not_finite_or_too_large_has_no_mask() {
    for (name, outline) in [
        ("empty", (vec![], vec![])),
        ("two points", polygon(&[(0.0, 0.0), (5.0, 5.0)])),
        ("not a number", rect(0.0, 0.0, f32::NAN, 4.0)),
        ("too large", rect(0.0, 0.0, 5000.0, 4.0)),
    ] {
        assert!(raster(&outline, (0.0, 0.0), (0.0, 0.0)).is_none(), "{name}");
    }
    // a zero-area contour has bounds of at least one pixel, so it makes a mask that covers nothing
    let flat = polygon(&[(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)]);
    let m = raster(&flat, (0.0, 0.0), (0.0, 0.0)).expect("a flat contour still gets a mask");
    assert_eq!((m.width, m.height), (10, 1));
    assert!(m.data.iter().all(|&v| v == 0));
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
fn dilation_only_adds_ink() {
    let outline = join(&[rect(0.0, 0.0, 6.0, 6.0), rect_cw(2.0, 2.0, 4.0, 4.0)]);
    let mut last = ink(&raster(&outline, (0.0, 0.0), (0.0, 0.0)).unwrap());
    for d in [0.05, 0.1, 0.2, 0.3] {
        let thick = raster(&outline, (0.0, 0.0), (d, d)).unwrap();
        assert!(ink(&thick) > last, "d = {d}");
        last = ink(&thick);
    }
}
