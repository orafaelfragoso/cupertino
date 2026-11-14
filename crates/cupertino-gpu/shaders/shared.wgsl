// The shared module: the drawing maths of cupertino_raster_math in WGSL, concatenated ahead of every program by the
// build step (cupertino-gpu-shaders). Each function computes what its namesake there computes, in f32, and the
// crate's tests check the generated programs against that crate's vectors.
//
// Conventions of every program: clip y = -1 is a target's first row (the top of an image handed on), texture
// coordinate v = 0 is a texture's first row, and the fragment position counts pixels from the first row and column.
// Uniform blocks only, std140 layout, no 2 x 2 matrices in them.

// Exponents up to this are circular corners.
const CIRCULAR_EXPONENT: f32 = 2.001;
// Midpoint steps of a shadow's integral across y.
const SHADOW_STEPS: i32 = 8;
// Most stops a gradient has.
const MAX_STOPS: i32 = 8;

// A triangle that covers the whole target from the vertex index alone (0, 1, 2).
fn fullscreen_position(index: u32) -> vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// Signed distance from p (relative to the centre) to the rounded rectangle of half size `half`, corner extent `e`
// and corner exponent `n`: circular corners up to CIRCULAR_EXPONENT, the closed-form first-order distance of the
// superellipse above it, held at least at the bounding-box distance.
fn rounded_distance(p: vec2<f32>, half: vec2<f32>, e: f32, n: f32) -> f32 {
    let qx = abs(p.x) - (half.x - e);
    let qy = abs(p.y) - (half.y - e);
    if (qx <= 0.0 && qy <= 0.0) {
        return max(qx, qy) - e;
    }
    if (qx <= 0.0) {
        return qy - e;
    }
    if (qy <= 0.0) {
        return qx - e;
    }
    if (e <= 0.0) {
        return sqrt(qx * qx + qy * qy);
    }
    if (n <= CIRCULAR_EXPONENT) {
        return sqrt(qx * qx + qy * qy) - e;
    }
    let u = max(qx / e, 1e-6);
    let v = max(qy / e, 1e-6);
    let g = pow(pow(u, n) + pow(v, n), 1.0 / n);
    let grad = sqrt(pow(u, 2.0 * n - 2.0) + pow(v, 2.0 * n - 2.0)) / pow(g, n - 1.0);
    let d = (g - 1.0) / grad * e;
    return max(d, max(qx, qy) - e);
}

fn rect_distance(p: vec2<f32>, half: vec2<f32>) -> f32 {
    return rounded_distance(p, half, 0.0, 2.0);
}

// Coverage of a pixel whose centre lies at signed distance d from an outline, at px pixels per unit.
fn coverage(d: f32, px: f32) -> f32 {
    return clamp(0.5 - d * px, 0.0, 1.0);
}

// Coverage of a stroke of `width` inside the outline.
fn stroke_coverage(d: f32, width: f32, px: f32) -> f32 {
    if (width <= 0.0) {
        return 0.0;
    }
    return coverage(d, px) - coverage(d + width, px);
}

// The error function, four-term rational approximation.
fn erf_approx(x: f32) -> f32 {
    let a = abs(x);
    var t = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    t = t * t;
    return sign(x) * (1.0 - 1.0 / (t * t));
}

// The normal density of deviation sigma at x.
fn gauss(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.5066283 * sigma);
}

// The blurred box across x at height y, its half width curved by a circular corner.
fn box_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(corner * corner - delta * delta, 0.0));
    let k = 0.70710677 / sigma;
    let low = 0.5 + 0.5 * erf_approx((x - curved) * k);
    let high = 0.5 + 0.5 * erf_approx((x + curved) * k);
    return high - low;
}

// The Gaussian blur of a rounded box at p relative to its centre: the blurred silhouette's coverage.
fn rounded_box_blur(p: vec2<f32>, half: vec2<f32>, sigma: f32, corner: f32) -> f32 {
    let low = p.y - half.y;
    let high = p.y + half.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / f32(SHADOW_STEPS);
    var y = start + step * 0.5;
    var value = 0.0;
    for (var i = 0; i < SHADOW_STEPS; i = i + 1) {
        value = value + box_x(p.x, p.y - y, sigma, corner, half) * gauss(y, sigma) * step;
        y = y + step;
    }
    return value;
}

// What rounded_box_blur reaches deep inside a box.
fn blur_mass(sigma: f32) -> f32 {
    let step = 6.0 * sigma / f32(SHADOW_STEPS);
    var mass = 0.0;
    for (var i = 0; i < SHADOW_STEPS; i = i + 1) {
        mass = mass + gauss(-3.0 * sigma + step * (f32(i) + 0.5), sigma) * step;
    }
    return mass;
}

// A rounded-rectangle caster: half size, corner radius of the blurred box, extent and exponent of its outline.
struct Caster {
    half: vec2<f32>,
    radius: f32,
    extent: f32,
    exponent: f32,
}

// A shadow: deviation, offset and density.
struct Shadow {
    sigma: f32,
    offset: vec2<f32>,
    density: f32,
}

// Alpha of a drop shadow at p; with `cut_out` the caster's own area is left clear.
fn drop_shadow(p: vec2<f32>, caster: Caster, shadow: Shadow, cut_out: bool, px: f32) -> f32 {
    let radius = min(caster.radius, min(caster.half.x, caster.half.y));
    let q = p - shadow.offset;
    var a = clamp(rounded_box_blur(q, caster.half, shadow.sigma, radius) * shadow.density, 0.0, 1.0);
    if (cut_out) {
        let extent = min(caster.extent, min(caster.half.x, caster.half.y));
        let d = rounded_distance(p, caster.half, extent, caster.exponent);
        a = a * clamp(0.5 + d * px, 0.0, 1.0);
    }
    return a;
}

// Alpha of an inner shadow at p.
fn inner_shadow(p: vec2<f32>, caster: Caster, shadow: Shadow, px: f32) -> f32 {
    let radius = min(caster.radius, min(caster.half.x, caster.half.y));
    let q = p - shadow.offset;
    let blurred = rounded_box_blur(q, caster.half, shadow.sigma, radius) / blur_mass(shadow.sigma);
    let outside = max(1.0 - blurred, 0.0);
    let extent = min(caster.extent, min(caster.half.x, caster.half.y));
    let inside = coverage(rounded_distance(p, caster.half, extent, caster.exponent), px);
    return clamp(outside * shadow.density, 0.0, 1.0) * inside;
}

// The premultiplied colour at t of `count` stops (offsets packed four to a vector, straight encoded sRGB colours).
fn gradient_at(offsets: array<vec4<f32>, 2>, colors: array<vec4<f32>, 8>, count: i32, t_in: f32) -> vec4<f32> {
    var t = t_in;
    if (t != t) {
        t = 0.0;
    }
    let last = count - 1;
    let first_offset = offsets[0].x;
    let last_offset = offsets[last / 4][last % 4];
    var straight = colors[0];
    if (t <= first_offset) {
        straight = colors[0];
    } else if (t >= last_offset) {
        straight = colors[last];
    } else {
        var i = 0;
        for (var j = 0; j < MAX_STOPS - 1; j = j + 1) {
            if (j >= last) {
                break;
            }
            if (t < offsets[(j + 1) / 4][(j + 1) % 4]) {
                i = j;
                break;
            }
            i = j;
        }
        let a = offsets[i / 4][i % 4];
        let b = offsets[(i + 1) / 4][(i + 1) % 4];
        let span = b - a;
        var k = 1.0;
        if (span > 0.0) {
            k = (t - a) / span;
        }
        straight = colors[i] + (colors[i + 1] - colors[i]) * k;
    }
    return vec4<f32>(straight.rgb * straight.a, straight.a);
}

// The 2 x 2 ordered matrix at bit k of a pixel's position.
fn dither_pair(p: vec2<u32>, k: u32) -> u32 {
    let a = (p.x >> k) & 1u;
    let b = (p.y >> k) & 1u;
    return 2u * (a ^ b) + b;
}

// The 8 x 8 ordered matrix entry, 0 to 63, of the pixel whose centre is `frag`.
fn dither_matrix(frag: vec2<f32>) -> u32 {
    let p = vec2<u32>(floor(frag));
    return 16u * dither_pair(p, 0u) + 4u * dither_pair(p, 1u) + dither_pair(p, 2u);
}

// The offset of the pixel whose centre is `frag`, in 8-bit steps, inside (-0.5, 0.5).
fn dither_offset(frag: vec2<f32>) -> f32 {
    return (f32(dither_matrix(frag)) + 0.5) / 64.0 - 0.5;
}

// v with its pixel's offset added, before an 8-bit store.
fn dither(v: f32, frag: vec2<f32>) -> f32 {
    return v + dither_offset(frag) / 255.0;
}

fn dither_rgb(c: vec3<f32>, frag: vec2<f32>) -> vec3<f32> {
    return c + vec3<f32>(dither_offset(frag) / 255.0);
}
