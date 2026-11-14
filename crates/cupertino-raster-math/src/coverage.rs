//! Distance and coverage of rectangles and rounded rectangles.
//!
//! A shape is given by its centre-relative point `p`, its half size, its corner extent `e` (the length along each
//! edge the corner takes) and its corner exponent `n`: 2 for a circular corner, more for a continuous one, whose
//! outline is the superellipse `|u|ⁿ + |v|ⁿ = 1` scaled by the extent. Distances are signed, negative inside, in the
//! units of `p`. The continuous corner's distance is the closed-form first-order one, `(G − 1) / |∇G|` with `G` the
//! superellipse norm, held at least at the bounding-box distance so it stays finite and monotonic.
//!
//! Coverage of a pixel is `clamp(0.5 − d · px, 0, 1)` with `d` the distance at the pixel centre and `px` the pixels
//! per unit: a one-pixel ramp centred on the outline. A stroke covers what lies inside the outline and outside the
//! outline moved in by its width. Nested clips multiply their coverage.

/// Exponents up to this are circular corners.
pub const CIRCULAR_EXPONENT: f32 = 2.001;

/// Signed distance from `p` (relative to the centre) to the rectangle of half size `half`.
pub fn rect_distance(p: [f32; 2], half: [f32; 2]) -> f32 {
    rounded_distance(p, half, 0.0, 2.0)
}

/// Signed distance from `p` (relative to the centre) to the rounded rectangle of half size `half`, corner extent
/// `extent` and corner exponent `exponent`.
pub fn rounded_distance(p: [f32; 2], half: [f32; 2], extent: f32, exponent: f32) -> f32 {
    let e = extent;
    let qx = p[0].abs() - (half[0] - e);
    let qy = p[1].abs() - (half[1] - e);
    if qx <= 0.0 && qy <= 0.0 {
        return qx.max(qy) - e;
    }
    if qx <= 0.0 {
        return qy - e;
    }
    if qy <= 0.0 {
        return qx - e;
    }
    if e <= 0.0 {
        return (qx * qx + qy * qy).sqrt();
    }
    if exponent <= CIRCULAR_EXPONENT {
        return (qx * qx + qy * qy).sqrt() - e;
    }
    let n = exponent;
    let u = (qx / e).max(1e-6);
    let v = (qy / e).max(1e-6);
    let g = (u.powf(n) + v.powf(n)).powf(1.0 / n);
    let grad = (u.powf(2.0 * n - 2.0) + v.powf(2.0 * n - 2.0)).sqrt() / g.powf(n - 1.0);
    let d = (g - 1.0) / grad * e;
    d.max(qx.max(qy) - e)
}

/// Coverage of a pixel whose centre lies at signed distance `distance` from an outline, at `px` pixels per unit.
pub fn coverage(distance: f32, px: f32) -> f32 {
    (0.5 - distance * px).clamp(0.0, 1.0)
}

/// Coverage of a stroke of `width` inside the outline: the outline's coverage less that of the outline moved in.
pub fn stroke_coverage(distance: f32, width: f32, px: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    coverage(distance, px) - coverage(distance + width, px)
}

/// Coverage under nested clips: the product of each clip's coverage.
pub fn clipped(coverages: &[f32]) -> f32 {
    coverages.iter().product()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_is_a_box_distance() {
        assert_eq!(rect_distance([0.0, 0.0], [10.0, 5.0]), -5.0);
        assert_eq!(rect_distance([12.0, 0.0], [10.0, 5.0]), 2.0);
        assert_eq!(rect_distance([13.0, 9.0], [10.0, 5.0]), 5.0);
    }

    #[test]
    fn a_circular_corner_is_a_circle_and_a_continuous_one_lies_inside_it() {
        let half = [20.0, 20.0];
        // on the diagonal through the corner centre (10, 10), radius 10
        let p = [10.0 + 10.0 / 2f32.sqrt(), 10.0 + 10.0 / 2f32.sqrt()];
        assert!(rounded_distance(p, half, 10.0, 2.0).abs() < 1e-4);
        // the continuous corner of the same extent bulges out further on its diagonal: that point is inside it
        assert!(rounded_distance(p, half, 10.0, 3.0) < -0.5);
        // straight edges are the same whatever the corner
        assert_eq!(rounded_distance([0.0, 21.0], half, 10.0, 3.0), 1.0);
    }

    #[test]
    fn coverage_is_a_pixel_wide_ramp_and_a_stroke_the_difference_of_two() {
        assert_eq!(coverage(0.0, 1.0), 0.5);
        assert_eq!(coverage(-0.25, 1.0), 0.75);
        assert_eq!(coverage(-1.0, 1.0), 1.0);
        assert_eq!(coverage(0.25, 2.0), 0.0);
        assert_eq!(stroke_coverage(-0.5, 1.0, 1.0), 1.0);
        assert_eq!(stroke_coverage(-3.0, 1.0, 1.0), 0.0);
        assert_eq!(stroke_coverage(-1.0, 1.0, 1.0), 0.5);
        assert_eq!(clipped(&[0.5, 0.5, 1.0]), 0.25);
        assert_eq!(clipped(&[]), 1.0);
    }
}
