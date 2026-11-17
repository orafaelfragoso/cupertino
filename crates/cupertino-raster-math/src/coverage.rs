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
