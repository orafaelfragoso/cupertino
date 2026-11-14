//! Glass: the CPU model of the background pass and of the foreground colour matrix, and the uniform block the
//! shaders read (`shaders/glass.wgsl`, `shaders/gles/glass.frag`).
//!
//! A glass *container* holds up to [`MAX_SHAPES`] shapes (rounded rectangles or capsules). Its backdrop is captured
//! once, at half the output's scale, over the bounds of every shape plus the reach of the blur and the refraction, gaps
//! included. The shapes join into one signed distance field: a smooth union whose blend width is the container's
//! spacing (0 is a hard union). Every pixel of the field is then shaded in this order:
//!
//! 1. **Blur.** One Gaussian of the backdrop per shape, its deviation twice the radius scaled by the first level's
//!    opacity ([`GlassParams::blur_sigma`]). The container blurs the backdrop once per distinct deviation of its shapes
//!    (at most [`BLUR_LEVELS`]); a shape between two of them mixes the two ([`level_mix`]). Then the blur fill (a
//!    Gaussian of twice the fill radius) is blended in darken, lighten and normal mode at their opacities.
//! 2. **Colour matrix.** On encoded values: saturation about the Rec. 709 luma `L`; the luma compression
//!    `c − (1 − max_luma) · L · (0.75 c + 0.25 L)` (a grey `x` becomes `x − (1 − max_luma) · x²`); the levels, one shift
//!    `black · (1 − L) − (1 − white) · L` added to every channel, `L` the luma after the compression; a limit at
//!    `clamp`; then the fill colour over it.
//! 3. **Inner refraction.** Inside a band `height` wide along the edge, the face is read `amount · g(t)` points along
//!    the field's outward normal, where `t` runs from 0 at the edge to 1 at the band's inner side and
//!    `g(t) = 0.9 · e^(−4.2 t) · (1 − t⁴)` ([`refraction_profile`]): a negative strength reads it further inside, so the
//!    edge magnifies what lies just inside it.
//! 4. **Bleed.** The blur fill, mapped to the encoded range `[black, white]` after its own saturation, blends
//!    into the face over a band `height` wide at `amount` percent: lighten on dark glass, darken on light glass. It
//!    shows at the bleed's opacity, which is 0 at rest.
//! 5. **Soft band and rim.** Where a surface asks for it, a soft band of the light `matrix · face` along the arcs of
//!    edge facing either end of the axis through `angle` ([`highlight_lobe`]). Then the rim ([`rim_shift`]): every
//!    channel shifted by `amount · color_bias` (darker for a negative bias), scaled by the face's brightness
//!    ([`rim_brightness`]) and by the edge's direction ([`rim_direction`], strongest on diagonals), in a band
//!    `RIM_DEPTH_PER_HEIGHT · height` deep.
//! 6. **Ring shadow.** Inside the shape, a stroke `stroke` wide along the inside of the outline, moved up by
//!    `ring_offset` and blurred with a Gaussian of `ring_blur`, darkens the face by its opacity ([`ring_shadow`]).
//! 7. **Drop shadow.** Outside the shape: a drop shadow offset by `offset`, blurred by `height` at `amount` percent
//!    of its fill's alpha, and an outline hairline. Ring and drop shadow are drawn at their opacity, which is 0 at rest.
//!    A pixel the shape's edge crosses shows the glass over this in the part the glass leaves uncovered.
//!
//! The foreground of text and symbols on glass is [`foreground`]: the face under the glyph through the mode's 4×5 matrix,
//! mixed over the face by the glyph's coverage.
//!
//! Colours are straight sRGB-encoded `f32` unless a function says otherwise.

use cupertino_theme::glass::{
    apply_matrix, refraction_profile, rim_brightness, rim_direction, ColorMatrix, GlassParams,
    BLUR_LEVELS, RIM_DEPTH_PER_HEIGHT,
};

/// Deviations closer than this, in points, are blurred once.
const SAME_SIGMA: f32 = 0.05;

pub use cupertino_theme::glass::{GlassMode, GlassStyle, GLASS_BACKDROP_SCALE};

/// Most shapes one container joins.
pub const MAX_SHAPES: usize = 16;

/// Widest blend between the shapes of a container, in points. The union bulges up to a quarter of the blend width
/// past the shapes, so this bounds how far a container's glass reaches beyond them.
pub const MAX_SPACING: f32 = 64.0;

/// Rec. 709 luminance weights of linear light.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub type Rgb = [f32; 3];

/// How a glass shape's corners curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassCorner {
    /// A quarter circle of the radius.
    Circular,
    /// A superellipse quadrant over [`CONTINUOUS_EXTENT`] times the radius of each side, of exponent
    /// [`CONTINUOUS_EXPONENT`]: the curve leaves the straight edge without a kink in its curvature.
    #[default]
    Continuous,
}

/// How far a continuous corner reaches along each side, per point of its radius.
pub const CONTINUOUS_EXTENT: f32 = 1.53;
/// The exponent of a continuous corner's superellipse (2 is a circle).
pub const CONTINUOUS_EXPONENT: f32 = 3.33;

/// One shape of a container: a rectangle `[x, y, w, h]` with corners of `radius` (clamped to half the short side; a
/// capsule has exactly that), continuous unless set otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassShape {
    pub rect: [f32; 4],
    pub radius: f32,
    pub corner: GlassCorner,
}

impl GlassShape {
    pub fn new(rect: [f32; 4], radius: f32) -> Self {
        Self {
            rect,
            radius,
            corner: GlassCorner::Continuous,
        }
    }

    /// The shape with corners curving as `corner` says.
    pub fn with_corner(self, corner: GlassCorner) -> Self {
        Self { corner, ..self }
    }

    /// How far each corner reaches along the sides, in points, and the exponent of its superellipse. A continuous
    /// corner reaches [`CONTINUOUS_EXTENT`] times the radius; once that would pass half the short side it stays there,
    /// and its exponent eases from [`CONTINUOUS_EXPONENT`] to 2 as the radius grows to half the short side, so a
    /// capsule has round ends.
    pub fn corner_shape(&self) -> (f32, f32) {
        let r = self.fitted_radius();
        let half = self.short_side() * 0.5;
        if r <= 0.0 {
            return (0.0, 2.0);
        }
        match self.corner {
            GlassCorner::Circular => (r, 2.0),
            GlassCorner::Continuous => {
                let extent = CONTINUOUS_EXTENT * r;
                if extent <= half {
                    return (extent, CONTINUOUS_EXPONENT);
                }
                let from = half / CONTINUOUS_EXTENT;
                let t = ((r - from) / (half - from).max(1e-6)).clamp(0.0, 1.0);
                (half, CONTINUOUS_EXPONENT + (2.0 - CONTINUOUS_EXPONENT) * t)
            }
        }
    }

    /// A capsule: the corners are half the short side.
    pub fn capsule(rect: [f32; 4]) -> Self {
        Self::new(rect, f32::INFINITY)
    }

    pub fn short_side(&self) -> f32 {
        self.rect[2].min(self.rect[3]).max(0.0)
    }

    /// The corner radius after fitting to the rectangle.
    pub fn fitted_radius(&self) -> f32 {
        self.radius.clamp(0.0, self.short_side() * 0.5)
    }

    /// Signed distance from `p` to the shape's outline, negative inside: exact along the straight edges and for
    /// circular corners, to first order in a continuous corner (the superellipse norm's excess over its slope).
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        let [x, y, w, h] = self.rect;
        let (e, n) = self.corner_shape();
        rounded_box_sdf(
            [p[0] - (x + w * 0.5), p[1] - (y + h * 0.5)],
            [w * 0.5, h * 0.5],
            e,
            n,
        )
    }
}

/// Signed distance from `p` (relative to the centre) to a box of half size `half` whose corners reach `e` along each
/// side with a superellipse of exponent `n` (2 is a circle).
pub fn rounded_box_sdf(p: [f32; 2], half: [f32; 2], e: f32, n: f32) -> f32 {
    let e = e.clamp(0.0, half[0].min(half[1]));
    let q = [p[0].abs() - (half[0] - e), p[1].abs() - (half[1] - e)];
    if q[0] <= 0.0 || q[1] <= 0.0 || e <= 0.0 {
        let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt();
        return outside + q[0].max(q[1]).min(0.0) - e;
    }
    if n <= 2.0 {
        return (q[0] * q[0] + q[1] * q[1]).sqrt() - e;
    }
    let u = [(q[0] / e).max(1e-6), (q[1] / e).max(1e-6)];
    let un = [u[0].powf(n), u[1].powf(n)];
    let g = (un[0] + un[1]).powf(1.0 / n);
    // the norm's gradient: u^(n − 1) / g^(n − 1) on each axis
    let gn = g.powf(n - 1.0);
    let slope = ((un[0] / u[0] / gn).powi(2) + (un[1] / u[1] / gn).powi(2)).sqrt();
    // never nearer than the corner's square, which holds the curve
    ((g - 1.0) * e / slope.max(1e-6)).max(q[0].max(q[1]) - e)
}

/// A smooth union of two distances with blend width `k` (0 is a hard union), and the weight of `a` in the blend, for
/// mixing per-shape values the same way.
pub fn smooth_union(a: f32, b: f32, k: f32) -> (f32, f32) {
    if k <= 0.0 {
        return if a <= b { (a, 1.0) } else { (b, 0.0) };
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    (b + (a - b) * h - k * h * (1.0 - h), h)
}

/// The union field of `shapes` at `p` with blend width `k`, and each shape's weight in it.
pub fn union_field(shapes: &[GlassShape], k: f32, p: [f32; 2]) -> (f32, Vec<f32>) {
    let mut weights = vec![0.0; shapes.len()];
    let Some(first) = shapes.first() else {
        return (f32::INFINITY, weights);
    };
    let mut d = first.sdf(p);
    weights[0] = 1.0;
    for (i, s) in shapes.iter().enumerate().skip(1) {
        let (u, h) = smooth_union(s.sdf(p), d, k);
        for w in &mut weights[..i] {
            *w *= 1.0 - h;
        }
        weights[i] = h;
        d = u;
    }
    (d, weights)
}

/// Outward unit normal of the field at `p` (central differences one unit apart); zero where the field is flat.
pub fn field_normal(shapes: &[GlassShape], k: f32, p: [f32; 2]) -> [f32; 2] {
    let g = field_gradient(shapes, k, p);
    let len = (g[0] * g[0] + g[1] * g[1]).sqrt();
    if len < 1e-6 {
        [0.0, 0.0]
    } else {
        [g[0] / len, g[1] / len]
    }
}

/// The central difference of the union's field over one unit at `p`: about a unit long near a real edge, and short
/// along the seam where two joined shapes meet, where the fields of both sides cancel.
pub fn field_gradient(shapes: &[GlassShape], k: f32, p: [f32; 2]) -> [f32; 2] {
    let f = |dx: f32, dy: f32| union_field(shapes, k, [p[0] + dx, p[1] + dy]).0;
    [f(0.5, 0.0) - f(-0.5, 0.0), f(0.0, 0.5) - f(0.0, -0.5)]
}

/// The shortest field slope [`edge_distance`] divides by.
pub const MIN_FIELD_SLOPE: f32 = 0.05;

/// How far inside the edge a point with field value `d` lies, from the field's `slope` there (the length of
/// [`field_gradient`]): `d` itself near a real edge, and far inside along a seam between joined shapes, whose shallow
/// field would otherwise put a rim light across the join.
pub fn edge_distance(d: f32, slope: f32) -> f32 {
    d / slope.max(MIN_FIELD_SLOPE)
}

pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

pub fn srgb_encode(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn luma(c: Rgb) -> f32 {
    c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// Saturation `s` about the Rec. 709 luma of the encoded colour `c`: the colour moves away from (or towards) the grey of
/// its luma by `s`, on encoded values, so the weaker channels of a strong colour fall off as they look to, rather than
/// as fast as in linear light, where a small encoded value is a much smaller share of the light.
pub fn saturate_encoded(c: Rgb, s: f32) -> Rgb {
    let y = luma(c);
    c.map(|v| (y + (v - y) * s).max(0.0))
}

/// The weight of each blurred copy for a shape at `position` among them: the copy itself at a whole position, the two
/// on either side mixed between.
pub fn level_weights(position: f32) -> [f32; BLUR_LEVELS] {
    let p = position.clamp(0.0, (BLUR_LEVELS - 1) as f32);
    std::array::from_fn(|i| (1.0 - (p - i as f32).abs()).max(0.0))
}

/// Stage 1: the blurred copies `levels` mixed for a shape at `position` among them ([`level_weights`]).
pub fn level_mix(levels: [Rgb; BLUR_LEVELS], position: f32) -> Rgb {
    let w = level_weights(position);
    std::array::from_fn(|c| (0..BLUR_LEVELS).map(|i| levels[i][c] * w[i]).sum())
}

/// The deviations (points, ascending) a container blurs its backdrop with for shapes blurring with `sigmas`: each
/// distinct one when there are at most [`BLUR_LEVELS`], else that many spread evenly from the narrowest to the widest.
pub fn blur_levels(sigmas: &[f32]) -> Vec<f32> {
    let mut d: Vec<f32> = sigmas.iter().map(|s| s.max(0.0)).collect();
    d.sort_by(f32::total_cmp);
    d.dedup_by(|a, b| (*a - *b).abs() < SAME_SIGMA);
    if d.len() <= BLUR_LEVELS {
        return d;
    }
    let (lo, hi) = (d[0], d[d.len() - 1]);
    (0..BLUR_LEVELS)
        .map(|i| lo + (hi - lo) * i as f32 / (BLUR_LEVELS - 1) as f32)
        .collect()
}

/// Where a shape blurring with `sigma` lies among `levels` (ascending): the index of the copy it reads, with the share
/// of the next one as its fraction.
pub fn level_position(levels: &[f32], sigma: f32) -> f32 {
    let Some(last) = levels.len().checked_sub(1) else {
        return 0.0;
    };
    for i in 0..last {
        let (a, b) = (levels[i], levels[i + 1]);
        if sigma <= b + SAME_SIGMA * 0.5 {
            let t = if b - a > 1e-6 {
                (sigma - a) / (b - a)
            } else {
                0.0
            };
            return i as f32 + t.clamp(0.0, 1.0);
        }
    }
    last as f32
}

/// Stage 1, second part: the blur fill `fill` blended over the blurred backdrop `c`.
pub fn blur_fill(c: Rgb, fill: Rgb, p: &GlassParams) -> Rgb {
    blur_fill_weighted(c, fill, p, 1.0)
}

/// [`blur_fill`] with each of its blends weighed by `w`, the share of its blur the glass keeps there
/// ([`Container::blur_weight`]).
pub fn blur_fill_weighted(c: Rgb, fill: Rgb, p: &GlassParams, w: f32) -> Rgb {
    let b = &p.blur;
    let darker: Rgb = std::array::from_fn(|i| c[i].min(fill[i]));
    let c = mix(c, darker, b.fill_darken * w);
    let lighter: Rgb = std::array::from_fn(|i| c[i].max(fill[i]));
    let c = mix(c, lighter, b.fill_lighten * w);
    mix(c, fill, b.fill_normal * w)
}

/// The luma compression of the face: `c − (1 − max_luma) · L · (0.75 c + 0.25 L)`, `L` the luma of `c`. A grey `x`
/// becomes `x − (1 − max_luma) · x²`: white comes out at `max_luma`, and below 0.5 the brightest greys come out darker
/// than slightly dimmer ones.
pub fn compress_luma(c: Rgb, max_luma: f32) -> Rgb {
    let (k, l) = (1.0 - max_luma, luma(c));
    c.map(|v| v - k * l * (0.75 * v + 0.25 * l))
}

/// The levels of the face: `black · (1 − L) − (1 − white) · L` added to every channel, `L` the luma of `c`.
pub fn face_levels(c: Rgb, black: f32, white: f32) -> Rgb {
    let l = luma(c);
    let shift = black * (1.0 - l) - (1.0 - white) * l;
    c.map(|v| v + shift)
}

/// Stage 2: the face colour of the blurred backdrop `c`, with the face's `max_luma` replaced by `max_luma` (it depends
/// on the shape's size): saturation, luma compression and levels on encoded values, then the fill over it.
pub fn face_color(c: Rgb, p: &GlassParams, max_luma: f32) -> Rgb {
    let f = &p.face;
    let c = compress_luma(saturate_encoded(c, f.saturation), max_luma);
    let c = face_levels(c, f.black, f.white).map(|v| v.clamp(0.0, f.clamp.min(1.0)));
    mix(c, [f.fill[0], f.fill[1], f.fill[2]], f.fill[3])
}

/// Stage 3: where the face is read for a pixel `d` from the edge (negative inside) with outward `normal`, as an
/// offset in points: `amount · g(−d / height)` along the normal ([`refraction_profile`]), further inside for a negative
/// strength. Zero outside the shape, past the band and without refraction.
pub fn refraction_offset(d: f32, normal: [f32; 2], amount: f32, height: f32) -> [f32; 2] {
    if height <= 0.0 || amount == 0.0 || d > 0.0 {
        return [0.0, 0.0];
    }
    let reach = amount * refraction_profile(-d / height);
    [normal[0] * reach, normal[1] * reach]
}

/// Stage 4: the bleed of the backdrop colour `wide` (the blur fill) into the face `c`, `d` from the edge, at the
/// bleed's opacity (none at rest).
pub fn bleed(c: Rgb, wide: Rgb, d: f32, p: &GlassParams, amount: f32, height: f32) -> Rgb {
    let b = &p.bleed;
    if amount <= 0.0 || height <= 0.0 || b.opacity <= 0.0 {
        return c;
    }
    let t = (-d / height).clamp(0.0, 1.0);
    let w = (amount / 100.0).clamp(0.0, 1.0) * b.opacity.min(1.0) * (1.0 - t) * (1.0 - t);
    let tone =
        saturate_encoded(wide, b.saturation).map(|v| b.black + (b.white - b.black) * v.min(1.0));
    let blended: Rgb = std::array::from_fn(|i| {
        if b.darken {
            c[i].min(tone[i])
        } else {
            c[i].max(tone[i])
        }
    });
    mix(c, blended, w)
}

/// Angle between two directions in radians, in `[0, π]`.
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d)
}

/// How strongly an edge whose outward normal points at `normal_angle` is lit from `light`: a raised cosine of the angle
/// between them, full where they agree and falling to nothing [`LOBE_REACH`] of `spread` away, with exponent
/// `curvature`. For a light from the top and a spread of 120° the lobe ends at the sides, still lighting much of the
/// corners on its way.
pub fn highlight_lobe(normal_angle: f32, light: f32, spread: f32, curvature: f32) -> f32 {
    let reach = spread * LOBE_REACH;
    if reach <= 0.0 {
        return 0.0;
    }
    let a = angle_between(normal_angle, light);
    if a >= reach {
        return 0.0;
    }
    (0.5 + 0.5 * (a / reach * std::f32::consts::PI).cos()).powf(curvature.max(1e-3))
}

/// How far from its end of the axis a highlight arc reaches, as a share of the spread.
pub const LOBE_REACH: f32 = 0.75;

/// Stage 5, first part: the soft band of light on the face `c` at distance `d` (negative inside, points) from the edge,
/// with outward `normal` in y-down coordinates: the colour `matrix · face` added along the arcs facing either end of the
/// axis through the light's angle, the far end at `glow_fill` of the near one; under it the diffuse band, as wide,
/// on arcs as wide and as strong as the soft band's times its scales. Nothing where the recipe has no band.
pub fn glow(c: Rgb, d: f32, normal: [f32; 2], p: &GlassParams) -> Rgb {
    let h = &p.highlight;
    if h.glow_amount <= 0.0 || h.glow_height <= 0.0 || (normal[0] == 0.0 && normal[1] == 0.0) {
        return c;
    }
    // y up for the angle
    let normal_angle = (-normal[1]).atan2(normal[0]);
    let ends = |spread: f32| {
        let key = highlight_lobe(normal_angle, h.angle, spread, h.curvature);
        let fill = highlight_lobe(
            normal_angle,
            h.angle + std::f32::consts::PI,
            spread,
            h.curvature,
        );
        key + fill * h.glow_fill.max(0.0)
    };
    let band = |height: f32| {
        let x = (d - h.glow_offset) / height;
        (-x * x).exp()
    };
    let mut w = h.glow_amount * band(h.glow_height) * ends(h.spread);
    let diffuse = &h.diffuse;
    if diffuse.amount_scale > 0.0 && diffuse.height_scale > 0.0 {
        w += h.glow_amount
            * diffuse.amount_scale
            * band(h.glow_height * diffuse.height_scale)
            * ends(h.spread * diffuse.spread_scale);
    }
    let light = matrix_rgb(&h.matrix, c);
    std::array::from_fn(|i| (c[i] + light[i] * w).min(1.0))
}

/// How much of the rim a pixel whose centre lies `depth` units inside the edge gets (the antialiased edge counts as on
/// it), for a rim `reach` units deep: all of it up to half a unit short of the reach, fading over the unit across it.
pub fn rim_band(depth: f32, reach: f32) -> f32 {
    (reach + 0.5 - depth.max(0.0)).clamp(0.0, 1.0)
}

/// Stage 5, second part: the shift the rim adds to every channel of the face `c` at distance `d` (negative inside,
/// points) from the edge, with outward `normal`: `amount · color_bias`, times the face's brightness response and the
/// edge's direction response, inside the rim's depth.
pub fn rim_shift(c: Rgb, d: f32, normal: [f32; 2], p: &GlassParams) -> f32 {
    let h = &p.highlight;
    if h.amount == 0.0 || h.height <= 0.0 || (normal[0] == 0.0 && normal[1] == 0.0) {
        return 0.0;
    }
    let band = rim_band(-d, RIM_DEPTH_PER_HEIGHT * h.height);
    if band <= 0.0 {
        return 0.0;
    }
    let angle = normal[1].atan2(normal[0]);
    h.amount * h.color_bias * rim_brightness(luma(c)) * rim_direction(angle) * band
}

/// Stage 5: the soft band, then the rim, on the face `c`.
pub fn highlight(c: Rgb, d: f32, normal: [f32; 2], p: &GlassParams) -> Rgb {
    let c = glow(c, d, normal, p);
    let shift = rim_shift(c, d, normal, p);
    c.map(|v| (v + shift).clamp(0.0, 1.0))
}

fn matrix_rgb(m: &ColorMatrix, c: Rgb) -> Rgb {
    let o = apply_matrix(m, [c[0], c[1], c[2], 1.0]);
    [o[0], o[1], o[2]].map(|v| v.clamp(0.0, 1.0))
}

/// Coverage of a Gaussian-blurred edge at signed distance `d` for blur radius `r` (a smooth step three radii wide).
fn blurred_coverage(d: f32, r: f32) -> f32 {
    if r <= 0.0 {
        return if d <= 0.0 { 1.0 } else { 0.0 };
    }
    let t = (0.5 - d / (3.0 * r)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The share of a Gaussian of deviation 1 below `x` (a logistic fit of the normal distribution, within 0.01).
pub fn normal_cdf(x: f32) -> f32 {
    1.0 / (1.0 + (-1.702 * x).exp())
}

/// Stage 6: how much the ring shadow darkens the face at a pixel whose field, with the ring moved up by its offset, is
/// `d_ring` (negative inside): the stroke from the outline to `stroke` inside it, blurred with a Gaussian of
/// `ring_blur`, at the ring's opacity. The face is multiplied by one minus it.
pub fn ring_shadow(d_ring: f32, p: &GlassParams) -> f32 {
    let s = &p.shadow;
    if s.ring_opacity <= 0.0 || s.ring_stroke <= 0.0 {
        return 0.0;
    }
    // the share of the blurred stroke [-stroke, 0] at the pixel's depth
    let mask = if s.ring_blur > 0.0 {
        normal_cdf(-d_ring / s.ring_blur) - normal_cdf((-s.ring_stroke - d_ring) / s.ring_blur)
    } else if (-s.ring_stroke..=0.0).contains(&d_ring) {
        1.0
    } else {
        0.0
    };
    s.ring_opacity.min(1.0) * mask.max(0.0)
}

/// Stage 7: the alpha of the drop shadow at a pixel outside the shape, whose field minus the drop shadow's offset is
/// `d_shadow`.
pub fn shadow_alpha(d_shadow: f32, p: &GlassParams) -> f32 {
    let s = &p.shadow;
    s.opacity
        * (s.amount / 100.0).clamp(0.0, 1.0)
        * s.fill[3]
        * blurred_coverage(d_shadow, s.height)
}

/// The alpha of the outline hairline at a pixel `d` points outside the shape: full within its width, fading over the
/// last pixel.
pub fn outline_alpha(d: f32, p: &GlassParams) -> f32 {
    let s = &p.shadow;
    if s.outline_opacity <= 0.0 || s.outline_width <= 0.0 {
        return 0.0;
    }
    s.outline_opacity * (s.outline_width + 0.5 - d).clamp(0.0, 1.0)
}

/// Stage 7, the edge line: the alpha of black a pixel `d` from the edge (a unit wide band just outside it, one device
/// pixel on the GPU) takes over a backdrop of colour `backdrop`, where the outline's outward normal is `normal`: the
/// recipe's `edge_line` times the rim's response to the backdrop's luma ([`rim_brightness`]), so the line shows along
/// light content and not at all over black (unless the recipe keeps it even over any backdrop), and times the share its
/// direction keeps ([`edge_line_side_share`]).
pub fn edge_line_alpha(d: f32, backdrop: Rgb, normal: [f32; 2], p: &GlassParams) -> f32 {
    let k = p.shadow.edge_line;
    if k <= 0.0 {
        return 0.0;
    }
    let response = if p.shadow.edge_line_even {
        1.0
    } else {
        rim_brightness(luma(backdrop))
    };
    k.min(1.0)
        * response
        * (1.5 - d).clamp(0.0, 1.0)
        * edge_line_side_share(normal, p.shadow.edge_line_sides)
}

/// How much of the edge line shows where the outline's outward normal is `normal` (any length, y down), for arcs
/// `sides` radians wide facing left and right ([`GlassShadow::edge_line_sides`]): a raised cosine of the normal's angle
/// from the horizontal, full along a straight side and nothing [`LOBE_REACH`] of `sides` away from it, so a top or
/// bottom edge draws none. All of it everywhere when `sides` is 0, and where the normal is unknown.
pub fn edge_line_side_share(normal: [f32; 2], sides: f32) -> f32 {
    let len = (normal[0] * normal[0] + normal[1] * normal[1]).sqrt();
    if sides <= 0.0 || len < 1e-6 {
        return 1.0;
    }
    let from_horizontal = (normal[0].abs() / len).clamp(0.0, 1.0).acos();
    let reach = sides * LOBE_REACH;
    if from_horizontal >= reach {
        return 0.0;
    }
    0.5 + 0.5 * (from_horizontal / reach * std::f32::consts::PI).cos()
}

/// The foreground of a glyph on glass: the face `face` under it through `matrix`, mixed over the face by `coverage`
/// (the glyph's coverage times its alpha).
pub fn foreground(face: Rgb, matrix: &ColorMatrix, coverage: f32) -> Rgb {
    mix(face, matrix_rgb(matrix, face), coverage.clamp(0.0, 1.0))
}

/// Size-dependent values of one shape, as the shaders take them per shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeValues {
    pub refraction_amount: f32,
    pub refraction_height: f32,
    pub bleed_amount: f32,
    pub bleed_height: f32,
    pub max_luma: f32,
    /// The deviation of the shape's blur in points.
    pub blur_sigma: f32,
    /// Where that blur lies among the container's blurred copies ([`level_position`]).
    pub blur_level: f32,
}

impl ShapeValues {
    pub fn of(p: &GlassParams) -> Self {
        Self {
            refraction_amount: p.refraction.amount,
            refraction_height: p.refraction.height,
            bleed_amount: p.bleed.amount,
            bleed_height: p.bleed.height,
            max_luma: p.face.max_luma,
            blur_sigma: p.blur_sigma(),
            blur_level: 0.0,
        }
    }

    fn mix(a: Self, b: Self, t: f32) -> Self {
        let m = |x: f32, y: f32| x + (y - x) * t;
        Self {
            refraction_amount: m(a.refraction_amount, b.refraction_amount),
            refraction_height: m(a.refraction_height, b.refraction_height),
            bleed_amount: m(a.bleed_amount, b.bleed_amount),
            bleed_height: m(a.bleed_height, b.bleed_height),
            max_luma: m(a.max_luma, b.max_luma),
            blur_sigma: m(a.blur_sigma, b.blur_sigma),
            blur_level: m(a.blur_level, b.blur_level),
        }
    }
}

/// An outline a container is cut to besides its own shapes: a rectangle `[x, y, w, h]` in points whose corners reach
/// `extent` along each side with a superellipse of exponent `exponent` (2 is a circle). Nothing the container draws
/// shows outside it, its shadow and edge line included; a shape inside it is left as it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub rect: [f32; 4],
    pub extent: f32,
    pub exponent: f32,
}

impl Cut {
    /// Signed distance from `p` to the outline, negative inside.
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        let [x, y, w, h] = self.rect;
        rounded_box_sdf(
            [p[0] - (x + w * 0.5), p[1] - (y + h * 0.5)],
            [w * 0.5, h * 0.5],
            self.extent,
            self.exponent,
        )
    }

    /// The share of the pixel at `p` inside the outline, over an antialiased edge one pixel wide.
    pub fn coverage(&self, p: [f32; 2]) -> f32 {
        (0.5 - self.sdf(p)).clamp(0.0, 1.0)
    }
}

/// A container ready to shade: its shapes (points), blend width, the mode, each shape's recipe and the outline it is
/// cut to, if any.
#[derive(Clone, Debug)]
pub struct Container {
    pub shapes: Vec<GlassShape>,
    pub spacing: f32,
    pub mode: GlassMode,
    /// The recipe of the container's values that do not depend on size (from its largest shape).
    pub params: GlassParams,
    pub values: Vec<ShapeValues>,
    /// The deviations in points the backdrop is blurred with, ascending ([`blur_levels`]).
    pub sigmas: Vec<f32>,
    /// The outline the container is cut to besides its shapes ([`Cut`]).
    pub cut: Option<Cut>,
    /// How much of its blur the glass keeps at the top and at the bottom of its first shape, smoothly between: `[1, 1]`
    /// blurs evenly, a lower bottom fades the blur out towards it (a variable blur).
    pub blur_ramp: [f32; 2],
    /// How much the glass magnifies what lies behind it about its first shape's centre: 1 for none, more for a lens.
    pub lens: f32,
}

impl Container {
    /// The container of `shapes` (at most [`MAX_SHAPES`]) joined with blend width `spacing` (at most [`MAX_SPACING`];
    /// negative or not finite is 0).
    pub fn new(shapes: Vec<GlassShape>, spacing: f32, mode: GlassMode) -> Self {
        let mut shapes = shapes;
        shapes.truncate(MAX_SHAPES);
        let largest = shapes
            .iter()
            .map(GlassShape::short_side)
            .fold(0.0f32, f32::max);
        let mut values: Vec<ShapeValues> = shapes
            .iter()
            .map(|s| ShapeValues::of(&GlassParams::new(mode, s.short_side())))
            .collect();
        let sigmas = blur_levels(&values.iter().map(|v| v.blur_sigma).collect::<Vec<_>>());
        for v in &mut values {
            v.blur_level = level_position(&sigmas, v.blur_sigma);
        }
        Self {
            shapes,
            spacing: if spacing.is_finite() {
                spacing.clamp(0.0, MAX_SPACING)
            } else {
                0.0
            },
            mode,
            params: GlassParams::new(mode, largest),
            values,
            sigmas,
            cut: None,
            blur_ramp: [1.0, 1.0],
            lens: 1.0,
        }
    }

    /// The container whose blur fades by `ramp` from its top to its bottom ([`Container::blur_ramp`]): a ramp below 1
    /// adds an unblurred copy as the first level, so where the weight reaches 0 the glass shows what lies behind it
    /// sharp.
    pub fn with_blur_ramp(mut self, ramp: [f32; 2]) -> Self {
        self.blur_ramp = ramp.map(|w| w.clamp(0.0, 1.0));
        if ramp[0] < 1.0 || ramp[1] < 1.0 {
            let mut all: Vec<f32> = self.values.iter().map(|v| v.blur_sigma).collect();
            all.push(0.0);
            self.sigmas = blur_levels(&all);
            for v in &mut self.values {
                v.blur_level = level_position(&self.sigmas, v.blur_sigma);
            }
        }
        self
    }

    /// Where the glass reads what lies behind `q`: magnified by [`Container::lens`] about its first shape's centre.
    pub fn lensed(&self, q: [f32; 2]) -> [f32; 2] {
        match self.shapes.first() {
            Some(s) if self.lens > 1.0 => {
                let c = [s.rect[0] + 0.5 * s.rect[2], s.rect[1] + 0.5 * s.rect[3]];
                [
                    c[0] + (q[0] - c[0]) / self.lens,
                    c[1] + (q[1] - c[1]) / self.lens,
                ]
            }
            _ => q,
        }
    }

    /// The share of its blur the glass keeps at `p` ([`Container::blur_ramp`]): from the top weight at the top of its
    /// first shape to the bottom weight at its bottom, along a smoothstep.
    pub fn blur_weight(&self, p: [f32; 2]) -> f32 {
        let [top, bottom] = self.blur_ramp;
        match self.shapes.first() {
            Some(s) if top < 1.0 || bottom < 1.0 => {
                let t = ((p[1] - s.rect[1]) / s.rect[3].max(1e-3)).clamp(0.0, 1.0);
                let t = t * t * (3.0 - 2.0 * t);
                top + (bottom - top) * t
            }
            _ => 1.0,
        }
    }

    /// The container cut to `cut` as well as to its shapes.
    pub fn cut_to(mut self, cut: Cut) -> Self {
        self.cut = Some(cut);
        self
    }

    /// The bounds of every shape, gaps included.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let first = self.shapes.first()?;
        let mut b = [
            first.rect[0],
            first.rect[1],
            first.rect[0] + first.rect[2],
            first.rect[1] + first.rect[3],
        ];
        for s in &self.shapes[1..] {
            b[0] = b[0].min(s.rect[0]);
            b[1] = b[1].min(s.rect[1]);
            b[2] = b[2].max(s.rect[0] + s.rect[2]);
            b[3] = b[3].max(s.rect[1] + s.rect[3]);
        }
        Some([b[0], b[1], b[2] - b[0], b[3] - b[1]])
    }

    /// What of the backdrop the container reads: its bounds grown by the reach of its blur and refraction, and by how
    /// far the smooth union bulges past the shapes (a quarter of the blend width).
    pub fn sample_area(&self) -> Option<[f32; 4]> {
        let [x, y, w, h] = self.bounds()?;
        let bulge = if self.shapes.len() > 1 {
            self.spacing * 0.25
        } else {
            0.0
        };
        let r = self.params.reach() + bulge;
        Some([x - r, y - r, w + 2.0 * r, h + 2.0 * r])
    }

    /// The field and the shape values blended like the field at `p`.
    pub fn field(&self, p: [f32; 2]) -> (f32, ShapeValues) {
        let (d, weights) = union_field(&self.shapes, self.spacing, p);
        let mut v = self.values[0];
        let mut total = weights[0];
        for (i, w) in weights.iter().enumerate().skip(1) {
            if total + w > 0.0 {
                v = ShapeValues::mix(v, self.values[i], w / (total + w));
            }
            total += w;
        }
        (d, v)
    }

    /// Shade the pixel at `p` (points): `sample(level, q)` reads blur level `level` (0..4, and 4 for the blur fill) of
    /// the backdrop at `q`. Returns the colour and its coverage (glass inside, shadow outside), none outside the
    /// outline the container is cut to.
    pub fn shade(&self, p: [f32; 2], sample: &dyn Fn(usize, [f32; 2]) -> Rgb) -> (Rgb, f32) {
        let (c, a) = self.shade_shapes(p, sample);
        (c, a * self.cut.map_or(1.0, |cut| cut.coverage(p)))
    }

    /// The colour and coverage of the shapes at `p`, before the cut.
    fn shade_shapes(&self, p: [f32; 2], sample: &dyn Fn(usize, [f32; 2]) -> Rgb) -> (Rgb, f32) {
        let params = &self.params;
        let (d, v) = self.field(p);
        let gradient = field_gradient(&self.shapes, self.spacing, p);
        let slope = (gradient[0] * gradient[0] + gradient[1] * gradient[1]).sqrt();
        let normal = if slope < 1e-6 {
            [0.0, 0.0]
        } else {
            [gradient[0] / slope, gradient[1] / slope]
        };
        // what lies under the shape at p, as its blur reads it: the edge line darkens it by its brightness
        let backdrop = || {
            let levels: [Rgb; BLUR_LEVELS] = std::array::from_fn(|i| sample(i, p));
            level_mix(levels, v.blur_level)
        };
        if d > 0.5 {
            return self.outside(p, d, normal, backdrop());
        }
        let weight = self.blur_weight(p);
        let face_at = |q: [f32; 2]| {
            let levels: [Rgb; BLUR_LEVELS] = std::array::from_fn(|i| sample(i, q));
            let c = blur_fill_weighted(
                level_mix(levels, v.blur_level * weight),
                sample(BLUR_LEVELS, q),
                params,
                weight,
            );
            face_color(c, params, v.max_luma)
        };
        let off = refraction_offset(d, normal, v.refraction_amount, v.refraction_height);
        let mut c = face_at(self.lensed([p[0] + off[0], p[1] + off[1]]));
        c = bleed(
            c,
            sample(BLUR_LEVELS, p),
            d,
            params,
            v.bleed_amount,
            v.bleed_height,
        );
        c = highlight(c, edge_distance(d, slope), normal, params);
        c = self.veil(p, c);
        // the ring is moved up: at p it is what the shape's field is a ring offset below p
        let ring = ring_shadow(
            self.field([p[0], p[1] + params.shadow.ring_offset]).0,
            params,
        );
        c = c.map(|v| v * (1.0 - ring));
        // an edge pixel: the glass over what lies outside it in the part it leaves uncovered
        let cov = (0.5 - d).clamp(0.0, 1.0);
        let (outside, alpha) = self.outside(p, d, normal, backdrop());
        let under = (1.0 - cov) * alpha;
        let a = cov + under;
        if a > 0.0 {
            c = std::array::from_fn(|i| (c[i].clamp(0.0, 1.0) * cov + outside[i] * under) / a);
        }
        (c, a)
    }

    /// The black veil over the face `c` at `p`: its opacity runs from the recipe's top value at the top of the shapes'
    /// bounds to its bottom value at their bottom.
    pub fn veil(&self, p: [f32; 2], c: Rgb) -> Rgb {
        let [top, bottom] = self.params.veil;
        if top <= 0.0 && bottom <= 0.0 {
            return c;
        }
        let Some(b) = self.bounds() else { return c };
        // the bounds are x, y, width, height
        let t = ((p[1] - b[1]) / b[3].max(1e-3)).clamp(0.0, 1.0);
        let k = 1.0 - (top + (bottom - top) * t).clamp(0.0, 1.0);
        c.map(|v| v * k)
    }

    /// The alpha of what the shape draws outside itself at `p`, `d` from its edge: the drop shadow and the outline.
    pub fn outside_alpha(&self, p: [f32; 2], d: f32) -> f32 {
        let s = &self.params.shadow;
        let d_shadow = self.field([p[0] - s.offset[0], p[1] - s.offset[1]]).0;
        (shadow_alpha(d_shadow, &self.params) + outline_alpha(d, &self.params)).min(1.0)
    }

    /// What the shape draws outside itself at `p`, `d` from its edge whose outward normal is `normal`, over a
    /// `backdrop` of that colour: the drop shadow and the outline in the shadow's colour, and the edge line over them in
    /// black ([`edge_line_alpha`]). Returns the colour and its alpha.
    pub fn outside(&self, p: [f32; 2], d: f32, normal: [f32; 2], backdrop: Rgb) -> (Rgb, f32) {
        let fill = self.params.shadow.fill;
        let shadow = self.outside_alpha(p, d);
        let line = edge_line_alpha(d, backdrop, normal, &self.params);
        let a = line + shadow * (1.0 - line);
        if a <= 0.0 {
            return ([fill[0], fill[1], fill[2]], 0.0);
        }
        let k = shadow * (1.0 - line) / a;
        ([fill[0] * k, fill[1] * k, fill[2] * k], a)
    }
}

/// Uniform block of the glass composite (`Glass` in `shaders/glass.wgsl` and `shaders/gles/glass.frag`), std140
/// with 16-byte members only. Lengths are in target pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlassUniform {
    /// Where the composite draws: x, y, w, h in target pixels (y down).
    pub quad: [f32; 4],
    /// Affine map from target pixels to the capture's texture coordinates: `u = a·x + b·y + c` (`[a, b, c, 0]`).
    pub uv_x: [f32; 4],
    /// The same for `v`.
    pub uv_y: [f32; 4],
    /// Number of shapes, blend width, antialiasing width, overall opacity.
    pub info: [f32; 4],
    /// The edge line's strength, the veil's opacity at the bottom of the shapes, and the target's width and height.
    pub levels: [f32; 4],
    /// Blur fill opacities: darken, lighten, normal, and the veil's opacity at the top of the shapes.
    pub fill: [f32; 4],
    /// Face: saturation, white, black, clamp.
    pub face: [f32; 4],
    /// Face fill, straight sRGB.
    pub face_fill: [f32; 4],
    /// Bleed: white, black, saturation, darken (0/1).
    pub bleed: [f32; 4],
    /// Rim strength, the soft band's light angle and spread, the rim's depth.
    pub highlight: [f32; 4],
    /// Highlight: offset, colour bias, curvature, the soft band's strength at the far end of its axis.
    pub highlight2: [f32; 4],
    /// The soft band of light: strength, half-width, offset, and the opacity of the outline outside the shape.
    pub glow: [f32; 4],
    /// The diffuse band under it: half-width, arcs' width and strength as multiples of the soft band's, and the
    /// angular width of the arcs facing left and right that keep the edge line (0: all round).
    pub diffuse: [f32; 4],
    /// Key colour matrix rows: `[r, g, b coefficients, offset (+ alpha coefficient)]`.
    pub highlight_matrix: [[f32; 4]; 3],
    /// Drop shadow: offset x, offset y, alpha, blur radius.
    pub shadow: [f32; 4],
    /// Shadow colour (rgb) and how much the ring darkens the face.
    pub shadow_color: [f32; 4],
    /// Ring inside the shape: how far up it is moved, its stroke, its Gaussian deviation, and the width of the outline
    /// outside the shape.
    pub ring: [f32; 4],
    /// Each shape: x, y, w, h.
    pub shapes: [[f32; 4]; MAX_SHAPES],
    /// Each shape: how far its corners reach along the sides, refraction strength, refraction band, bleed fraction.
    pub shape_a: [[f32; 4]; MAX_SHAPES],
    /// Each shape: bleed band, largest luminance, where its blur lies among the blurred copies, the exponent of its
    /// corners' superellipse.
    pub shape_b: [[f32; 4]; MAX_SHAPES],
    /// The outline the container is cut to: x, y, w, h.
    pub cut: [f32; 4],
    /// How far its corners reach along the sides, the exponent of their superellipse, 1 when the container is cut and
    /// 0 when not, and 1 when the edge line keeps its strength over any backdrop (0 when it follows its brightness).
    pub cut_shape: [f32; 4],
    /// The blur's weight at the top and at the bottom of the shapes ([`Container::blur_ramp`]), the magnification
    /// about the first shape's centre ([`Container::lens`]), unused.
    pub lens: [f32; 4],
}

impl GlassUniform {
    /// The uniform of `container` (output-local points) drawn at `scale` pixels per point: `uv` maps target pixels to
    /// the capture's texture coordinates (`[[a, b, c], [d, e, f]]`), `quad` is where it draws in a `target` that size, and
    /// `opacity` fades it.
    pub fn new(
        container: &Container,
        scale: f32,
        uv: [[f32; 3]; 2],
        quad: [f32; 4],
        target: [f32; 2],
        opacity: f32,
    ) -> Self {
        let p = &container.params;
        let mut u: Self = bytemuck::Zeroable::zeroed();
        u.quad = quad;
        u.uv_x = [uv[0][0], uv[0][1], uv[0][2], 0.0];
        u.uv_y = [uv[1][0], uv[1][1], uv[1][2], 0.0];
        u.info = [
            container.shapes.len() as f32,
            container.spacing * scale,
            1.0,
            opacity.clamp(0.0, 1.0),
        ];
        u.levels = [p.shadow.edge_line, p.veil[1], target[0], target[1]];
        u.fill = [
            p.blur.fill_darken,
            p.blur.fill_lighten,
            p.blur.fill_normal,
            p.veil[0],
        ];
        u.face = [p.face.saturation, p.face.white, p.face.black, p.face.clamp];
        u.face_fill = p.face.fill;
        u.bleed = [
            p.bleed.white,
            p.bleed.black,
            p.bleed.saturation,
            if p.bleed.darken { 1.0 } else { 0.0 },
        ];
        let h = &p.highlight;
        u.highlight = [
            h.amount,
            h.angle,
            h.spread,
            RIM_DEPTH_PER_HEIGHT * h.height * scale,
        ];
        u.highlight2 = [h.offset * scale, h.color_bias, h.curvature, h.glow_fill];
        u.glow = [
            h.glow_amount,
            h.glow_height * scale,
            h.glow_offset * scale,
            p.shadow.outline_opacity,
        ];
        u.diffuse = [
            h.diffuse.height_scale,
            h.diffuse.spread_scale,
            h.diffuse.amount_scale,
            p.shadow.edge_line_sides,
        ];
        for r in 0..3 {
            let m = &h.matrix[r * 5..r * 5 + 5];
            u.highlight_matrix[r] = [m[0], m[1], m[2], m[3] + m[4]];
        }
        let s = &p.shadow;
        u.shadow = [
            s.offset[0] * scale,
            s.offset[1] * scale,
            s.opacity * (s.amount / 100.0).clamp(0.0, 1.0) * s.fill[3],
            s.height * scale,
        ];
        u.shadow_color = [s.fill[0], s.fill[1], s.fill[2], s.ring_opacity];
        u.ring = [
            s.ring_offset * scale,
            s.ring_stroke * scale,
            s.ring_blur * scale,
            s.outline_width * scale,
        ];
        for (i, (shape, v)) in container.shapes.iter().zip(&container.values).enumerate() {
            u.shapes[i] = shape.rect.map(|x| x * scale);
            let (extent, exponent) = shape.corner_shape();
            u.shape_a[i] = [
                extent * scale,
                v.refraction_amount * scale,
                v.refraction_height * scale,
                (v.bleed_amount / 100.0).clamp(0.0, 1.0) * p.bleed.opacity.clamp(0.0, 1.0),
            ];
            u.shape_b[i] = [v.bleed_height * scale, v.max_luma, v.blur_level, exponent];
        }
        if let Some(cut) = container.cut {
            u.cut = cut.rect.map(|x| x * scale);
            u.cut_shape = [cut.extent * scale, cut.exponent, 1.0, 0.0];
        }
        u.cut_shape[3] = if p.shadow.edge_line_even { 1.0 } else { 0.0 };
        u.lens = [
            container.blur_ramp[0].clamp(0.0, 1.0),
            container.blur_ramp[1].clamp(0.0, 1.0),
            container.lens.max(1.0),
            0.0,
        ];
        u
    }

    /// The uniform with the edges of its square shapes and of its cut on the nearest whole pixel: a square edge is then
    /// hard and meets what is drawn beside it on the same pixel, with no column of partial coverage between them.
    /// Rounded shapes keep their edges where they are.
    pub fn on_pixels(mut self) -> Self {
        let n = (self.info[0].max(0.0) as usize).min(MAX_SHAPES);
        for i in 0..n {
            if self.shape_a[i][0] <= 0.0 {
                self.shapes[i] = whole_pixels(self.shapes[i]);
            }
        }
        if self.cut_shape[2] > 0.0 {
            self.cut = whole_pixels(self.cut);
        }
        self
    }
}

/// `[x, y, w, h]` with each edge on the nearest whole pixel.
fn whole_pixels([x, y, w, h]: [f32; 4]) -> [f32; 4] {
    let (x0, y0) = (x.round(), y.round());
    [x0, y0, (x + w).round() - x0, (y + h).round() - y0]
}

impl Container {
    /// The deviation in points of each blurred copy of the backdrop, and whether a shape reads it.
    pub fn levels(&self) -> [(f32, bool); BLUR_LEVELS] {
        let mut used = [false; BLUR_LEVELS];
        // a blur ramp reads every level from the sharp one up to the shape's
        let ramp = self.blur_ramp[0] < 1.0 || self.blur_ramp[1] < 1.0;
        for v in &self.values {
            for (i, (u, w)) in used.iter_mut().zip(level_weights(v.blur_level)).enumerate() {
                *u |= w > 1e-3 || (ramp && i as f32 <= v.blur_level.ceil());
            }
        }
        let last = self.sigmas.last().copied().unwrap_or(0.0);
        std::array::from_fn(|i| (self.sigmas.get(i).copied().unwrap_or(last), used[i]))
    }

    /// Whether a shape bleeds the backdrop into its edge.
    fn bleeds(&self) -> bool {
        self.params.bleed.opacity > 0.0
            && self
                .values
                .iter()
                .any(|v| v.bleed_amount > 0.0 && v.bleed_height > 0.0)
    }

    /// The deviation in points of the blur fill, if it shows or the bleed reads it.
    pub fn fill_sigma(&self) -> Option<f32> {
        let sigma = self.params.fill_sigma();
        ((self.params.fill_shows() || self.bleeds()) && sigma > 0.0).then_some(sigma)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_theme::glass::{foreground_matrix, GlowDiffuse};

    fn params(dark: bool) -> GlassParams {
        GlassParams::new(
            GlassMode {
                dark,
                ..GlassMode::default()
            },
            100.0,
        )
    }

    fn close(a: f32, b: f32, e: f32) -> bool {
        (a - b).abs() <= e
    }

    #[test]
    fn a_continuous_corner_reaches_further_than_its_radius_and_rounds_a_capsule() {
        // 12 on a 40 pt side: the curve starts 1.53 · 12 from the corner, exponent 3.33
        let s = GlassShape::new([0.0, 0.0, 100.0, 40.0], 12.0);
        let (e, n) = s.corner_shape();
        assert!(close(e, 18.36, 1e-4) && n == CONTINUOUS_EXPONENT, "{e} {n}");
        // once 1.53 · r passes half the short side the reach stays there and the exponent eases to 2
        let (e, n) = GlassShape::new([0.0, 0.0, 100.0, 40.0], 16.0).corner_shape();
        let t = (16.0 - 20.0 / CONTINUOUS_EXTENT) / (20.0 - 20.0 / CONTINUOUS_EXTENT);
        assert!(
            close(e, 20.0, 1e-5) && close(n, 3.33 - 1.33 * t, 1e-4),
            "{e} {n}"
        );
        // and a capsule's ends are half circles: the same distances as with circular corners
        let capsule = GlassShape::new([0.0, 0.0, 100.0, 40.0], 20.0);
        assert_eq!(capsule.corner_shape(), (20.0, 2.0));
        let round = capsule.with_corner(GlassCorner::Circular);
        for p in [
            [3.0, 4.0],
            [10.0, -2.0],
            [96.0, 37.0],
            [50.0, 20.0],
            [1.0, 20.0],
        ] {
            assert!(close(capsule.sdf(p), round.sdf(p), 1e-4), "{p:?}");
        }
        // the reach grows without a jump as the radius passes the point where it is capped
        let at = |r: f32| GlassShape::new([0.0, 0.0, 100.0, 40.0], r).corner_shape();
        let edge = 20.0 / CONTINUOUS_EXTENT;
        let (below, above) = (at(edge - 1e-3), at(edge + 1e-3));
        assert!(close(below.0, above.0, 1e-2) && close(below.1, above.1, 1e-2));
        assert_eq!(GlassShape::new([0.0; 4], 0.0).corner_shape(), (0.0, 2.0));
    }

    #[test]
    fn the_continuous_outline_runs_through_its_superellipse() {
        let s = GlassShape::new([10.0, 20.0, 120.0, 60.0], 14.0);
        let (e, n) = s.corner_shape();
        // the bottom right corner's centre: the curve is (cos θ)^(2/n), (sin θ)^(2/n) of its reach from there
        let centre = [130.0 - e, 80.0 - e];
        for i in 0..=16 {
            let a = std::f32::consts::FRAC_PI_2 * i as f32 / 16.0;
            let (u, v) = (a.cos().powf(2.0 / n), a.sin().powf(2.0 / n));
            let p = [centre[0] + e * u, centre[1] + e * v];
            assert!(s.sdf(p).abs() < 1e-3, "θ {a}: {}", s.sdf(p));
        }
        // it lies outside the circle of the radius at the diagonal: the corner is fuller where it meets the straight
        // edge, and still cut well inside the square
        let circle = s.with_corner(GlassCorner::Circular);
        let diagonal = [130.0 - 1.5, 80.0 - 1.5];
        assert!(s.sdf(diagonal) > 0.0 && circle.sdf(diagonal) > 0.0);
        let near_edge = [130.0 - 0.4, 80.0 - e + 4.0];
        assert!(s.sdf(near_edge) < 0.0 && circle.sdf(near_edge) < 0.0);
        // on the straight edge's line, 16 from the corner, the circle has not started bending and the continuous corner
        // has
        let curve_start = [130.0, 80.0 - 16.0];
        assert!(
            s.sdf(curve_start) > 0.03 && circle.sdf(curve_start).abs() < 1e-5,
            "a continuous corner starts bending before the radius does: {}",
            s.sdf(curve_start)
        );
    }

    #[test]
    fn the_corner_distance_is_close_to_the_exact_distance_near_the_outline() {
        let s = GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0);
        let (e, n) = s.corner_shape();
        let mut worst = 0.0f32;
        for i in 0..=40 {
            for j in 0..=40 {
                // around the top left corner, in the square the curve lies in and a band outside it
                let p = [
                    -4.0 + (e + 4.0) * i as f32 / 40.0,
                    -4.0 + (e + 4.0) * j as f32 / 40.0,
                ];
                let q = [e - p[0], e - p[1]];
                if q[0] <= 0.0 || q[1] <= 0.0 {
                    continue;
                }
                let exact = cupertino_geometry::rounded::corner_sdf_exact(
                    q[0] as f64,
                    q[1] as f64,
                    e as f64,
                    n as f64,
                ) as f32;
                let ours = s.sdf(p);
                if exact.abs() < 2.0 {
                    worst = worst.max((ours - exact).abs());
                }
                assert_eq!(ours > 0.0, exact > 0.0, "{p:?}: {ours} {exact}");
            }
        }
        assert!(
            worst < 0.1,
            "within a tenth of a point of the outline's two points: {worst}"
        );
    }

    #[test]
    fn straight_pixel_aligned_edges_cover_whole_pixels() {
        for corner in [GlassCorner::Continuous, GlassCorner::Circular] {
            let c = Container::new(
                vec![GlassShape::new([10.0, 10.0, 100.0, 50.0], 12.0).with_corner(corner)],
                0.0,
                GlassMode::default(),
            );
            let grey = |_: usize, _: [f32; 2]| [0.5; 3];
            // pixel centres just inside and just outside each straight edge, away from the corners
            for (inside, outside) in [
                ([10.5, 35.5], [9.5, 35.5]),
                ([109.5, 35.5], [110.5, 35.5]),
                ([60.5, 10.5], [60.5, 9.5]),
                ([60.5, 59.5], [60.5, 60.5]),
            ] {
                assert_eq!(c.shade(inside, &grey).1, 1.0, "{corner:?} {inside:?}");
                // half a pixel out: no glass, only the edge line, and the drop shadow starts past it
                assert_eq!(c.field(outside).0, 0.5, "{corner:?} {outside:?}");
                let (col, a) = c.shade(outside, &grey);
                assert_eq!(
                    (col, a),
                    ([0.0; 3], c.params.shadow.edge_line),
                    "{corner:?} {outside:?}"
                );
            }
        }
    }

    #[test]
    fn a_square_shape_cut_to_a_window_s_rounded_outline_is_clear_past_its_corners_and_unchanged_inside(
    ) {
        // a square sidebar over the left of a window of 400 × 300 points with continuous corners of radius 16
        let window = [20.0, 20.0, 400.0, 300.0];
        let outline = Cut {
            rect: window,
            extent: 16.0 * CONTINUOUS_EXTENT,
            exponent: CONTINUOUS_EXPONENT,
        };
        let sidebar = || {
            Container::new(
                vec![GlassShape::new([20.0, 20.0, 160.0, 300.0], 0.0)],
                0.0,
                GlassMode::default(),
            )
        };
        let (plain, cut) = (sidebar(), sidebar().cut_to(outline));
        let wall = |_: usize, q: [f32; 2]| [q[0] / 500.0, q[1] / 400.0, 0.5];
        // the window's top-left and bottom-left corner pixels, and a few points in along the diagonal
        for p in [[20.5, 20.5], [22.5, 22.5], [20.5, 319.5], [22.5, 317.5]] {
            assert_eq!(
                plain.shade(p, &wall).1,
                1.0,
                "the square shape covers {p:?}"
            );
            assert_eq!(
                cut.shade(p, &wall).1,
                0.0,
                "past the rounded corner {p:?} is clear"
            );
        }
        // inside the outline, along the edges, past the corners and in the middle, it is the shape as it was
        for y in (0..300).step_by(7) {
            for x in (0..170).step_by(7) {
                let p = [20.5 + x as f32, 20.5 + y as f32];
                if outline.sdf(p) > -0.5 {
                    continue;
                }
                assert_eq!(cut.shade(p, &wall), plain.shade(p, &wall), "{p:?}");
            }
        }
        // a shape rounded inside the outline is left as it is
        let button = || {
            Container::new(
                vec![GlassShape::capsule([40.0, 40.0, 80.0, 28.0])],
                0.0,
                GlassMode::default(),
            )
        };
        let (plain, cut) = (button(), button().cut_to(outline));
        for y in 30..80 {
            for x in 30..130 {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                assert_eq!(cut.shade(p, &wall), plain.shade(p, &wall), "{p:?}");
            }
        }
        // the uniform carries the outline in pixels, and an uncut container leaves it off
        let u = |c: &Container| GlassUniform::new(c, 2.0, [[0.0; 3]; 2], [0.0; 4], [1.0; 2], 1.0);
        assert_eq!(u(&cut).cut, window.map(|v| v * 2.0));
        assert_eq!(
            u(&cut).cut_shape,
            [
                16.0 * CONTINUOUS_EXTENT * 2.0,
                CONTINUOUS_EXPONENT,
                1.0,
                0.0
            ]
        );
        assert_eq!(u(&plain).cut_shape[2], 0.0);
    }

    #[test]
    fn on_pixels_puts_square_edges_and_the_cut_on_whole_pixels_and_leaves_rounded_shapes() {
        // at 1.5 a sidebar 161 points wide ends half way across a pixel
        let c = Container::new(
            vec![
                GlassShape::new([0.0, 0.0, 161.0, 301.0], 0.0),
                GlassShape::new([200.0, 10.0, 81.0, 29.0], 8.0),
            ],
            0.0,
            GlassMode::default(),
        )
        .cut_to(Cut {
            rect: [0.0, 0.0, 401.0, 301.0],
            extent: 16.0,
            exponent: 2.0,
        });
        let u = GlassUniform::new(&c, 1.5, [[0.0; 3]; 2], [0.0; 4], [1.0; 2], 1.0);
        let snapped = u.on_pixels();
        assert_eq!(u.shapes[0], [0.0, 0.0, 241.5, 451.5]);
        assert_eq!(snapped.shapes[0], [0.0, 0.0, 242.0, 452.0]);
        assert_eq!(
            snapped.shapes[1], u.shapes[1],
            "a rounded shape stays where it is"
        );
        assert_eq!(snapped.cut, [0.0, 0.0, 602.0, 452.0]);
        // its right edge is hard: the pixel left of it is all glass and the one right of it none
        let field = |p: [f32; 2]| {
            let [x, y, w, h] = snapped.shapes[0];
            rounded_box_sdf(
                [p[0] - (x + w * 0.5), p[1] - (y + h * 0.5)],
                [w * 0.5, h * 0.5],
                0.0,
                2.0,
            )
        };
        assert_eq!((0.5 - field([241.5, 200.5])).clamp(0.0, 1.0), 1.0);
        assert_eq!((0.5 - field([242.5, 200.5])).clamp(0.0, 1.0), 0.0);
    }

    #[test]
    fn the_shaded_face_reads_the_backdrop_its_profile_deep_inside_the_edge() {
        let mut c = Container::new(
            vec![GlassShape::new([0.0, 0.0, 200.0, 120.0], 10.0)],
            0.0,
            GlassMode::default(),
        );
        // an identity face over a backdrop whose red and green channels are its coordinates
        let p = &mut c.params;
        p.blur.fill_darken = 0.0;
        p.blur.fill_lighten = 0.0;
        p.blur.fill_normal = 0.0;
        p.face.saturation = 1.0;
        p.face.white = 1.0;
        p.face.black = 0.0;
        p.face.clamp = 1.0;
        p.face.fill[3] = 0.0;
        p.bleed.opacity = 0.0;
        p.highlight.amount = 0.0;
        p.highlight.glow_amount = 0.0;
        p.shadow.ring_opacity = 0.0;
        p.shadow.edge_line = 0.0;
        for (amount, height) in [(-20.0, 40.0), (-10.0, 10.0), (-40.0, 20.0)] {
            c.values[0].max_luma = 1.0;
            c.values[0].refraction_amount = amount;
            c.values[0].refraction_height = height;
            let coords = |_: usize, q: [f32; 2]| [q[0] / 256.0, q[1] / 256.0, 0.5];
            for i in 0..80 {
                let x = 0.5 + i as f32 * 0.5;
                let (rgb, _) = c.shade([x, 60.0], &coords);
                let read = [rgb[0] * 256.0, rgb[1] * 256.0];
                // the left edge's normal points to −x: a negative strength reads further right, inside
                let expected = -amount * refraction_profile(x / height);
                assert!(
                    (read[0] - x - expected).abs() < 1e-3 && (read[1] - 60.0).abs() < 1e-3,
                    "{amount} over {height} at {x}: read {read:?}, {expected} in"
                );
            }
        }
        // at the edge nine tenths of the strength; a quarter of the band in, the share e^(−1.05) · (1 − 1/256) of that
        c.values[0].refraction_amount = -20.0;
        c.values[0].refraction_height = 40.0;
        let at = |x: f32| {
            c.shade([x, 60.0], &|_, q| [q[0] / 256.0, q[1] / 256.0, 0.5])
                .0[0]
                * 256.0
                - x
        };
        assert!((at(1e-3) - 18.0).abs() < 1e-2, "{}", at(1e-3));
        assert!((at(10.0) - 18.0 * (-1.05f32).exp() * (1.0 - 1.0 / 256.0)).abs() < 1e-3);
        assert!(at(40.5).abs() < 1e-4, "nothing past the band");
    }

    #[test]
    fn shape_distance_is_exact_for_rounded_rectangles() {
        let s = GlassShape::new([0.0, 0.0, 100.0, 40.0], 10.0).with_corner(GlassCorner::Circular);
        assert!(
            close(s.sdf([50.0, 20.0]), -20.0, 1e-5),
            "centre: half the short side inside"
        );
        assert!(close(s.sdf([110.0, 20.0]), 10.0, 1e-5));
        // the corner arc: 10 past the corner's centre along the diagonal
        let c = 10.0 - 10.0 / 2f32.sqrt();
        assert!(close(s.sdf([c, c]), 0.0, 1e-4));
        let capsule = GlassShape::capsule([0.0, 0.0, 100.0, 40.0]);
        assert_eq!(capsule.fitted_radius(), 20.0);
    }

    #[test]
    fn a_union_without_spacing_is_hard_and_spacing_bridges_the_gap() {
        // two 150 × 100 shapes 5 apart, as in a container of two buttons: a point in the gap
        let a = GlassShape::new([0.0, 0.0, 150.0, 100.0], 18.0);
        let b = GlassShape::new([155.0, 0.0, 150.0, 100.0], 18.0);
        let gap = [152.5, 50.0];
        let (hard, _) = union_field(&[a, b], 0.0, gap);
        assert!(
            close(hard, 2.5, 1e-4),
            "hard union: the gap stays open ({hard})"
        );
        let (smooth, w) = union_field(&[a, b], 20.0, gap);
        assert!(
            smooth < 0.0,
            "spacing 20 joins the shapes across a 5 point gap ({smooth})"
        );
        assert!(close(w[0], 0.5, 1e-4) && close(w[1], 0.5, 1e-4));
        // far from the gap the field is the nearer shape's
        let (far, w) = union_field(&[a, b], 20.0, [10.0, 50.0]);
        assert!(close(far, -10.0, 1e-4) && w[0] == 1.0);
    }

    #[test]
    fn a_shape_reads_the_copy_of_its_own_blur_or_mixes_the_two_around_it() {
        let levels = [[1.0; 3], [0.5; 3], [0.25; 3], [0.0; 3]];
        assert_eq!(level_mix(levels, 0.0), [1.0; 3]);
        assert_eq!(level_mix(levels, 2.0), [0.25; 3]);
        assert!(close(
            level_mix(levels, 1.25)[0],
            0.75 * 0.5 + 0.25 * 0.25,
            1e-6
        ));
        assert_eq!(level_mix(levels, 7.0), [0.0; 3], "past the last copy");
        // each distinct deviation is blurred once, ascending
        assert_eq!(blur_levels(&[8.0, 4.0, 4.0, 8.01]), vec![4.0, 8.0]);
        // past four, four spread from the narrowest to the widest, and a shape between two mixes them
        let many = blur_levels(&[4.0, 5.0, 6.0, 7.0, 10.0]);
        assert_eq!(many, vec![4.0, 6.0, 8.0, 10.0]);
        assert!(close(level_position(&many, 7.0), 1.5, 1e-6));
        assert_eq!(level_position(&many, 10.0), 3.0);
        assert_eq!(level_position(&[4.0], 4.0), 0.0);
        // a container of a small control and a large panel blurs twice, each shape with its own deviation
        let c = Container::new(
            vec![
                GlassShape::new([0.0, 0.0, 44.0, 44.0], 22.0),
                GlassShape::new([0.0, 60.0, 300.0, 160.0], 30.0),
            ],
            0.0,
            GlassMode::default(),
        );
        assert_eq!(c.sigmas, vec![4.0, 8.0]);
        assert_eq!((c.values[0].blur_level, c.values[1].blur_level), (0.0, 1.0));
        assert_eq!(c.levels().map(|l| l.1), [true, true, false, false]);
    }

    /// A row of a hard black-to-white edge (at texel 1001 of 2048) blurred with a Gaussian of `sigma` texels as the
    /// GPU passes make it ([`cupertino_raster_math::blur::plan_blur`]: 2× box halvings, then the separable weights on the last level,
    /// reads clamped to the row), read back at every full-resolution texel with linear filtering.
    fn blurred_edge(sigma: f32) -> Vec<f32> {
        const N: usize = 2048;
        let plan = cupertino_raster_math::blur::plan_blur(N as u32, 64, 1.0, sigma, 1.0);
        let mut row: Vec<f32> = (0..N).map(|j| if j > 1000 { 1.0 } else { 0.0 }).collect();
        for _ in &plan.levels {
            row = row.chunks(2).map(|p| (p[0] + p[1]) * 0.5).collect();
        }
        let n = row.len() as i64;
        let at = |i: i64| row[i.clamp(0, n - 1) as usize];
        let r = plan.weights.len() as i64 - 1;
        let blur: Vec<f32> = (0..n)
            .map(|i| {
                (-r..=r)
                    .map(|j| plan.weights[j.unsigned_abs() as usize] * at(i + j))
                    .sum()
            })
            .collect();
        let k = (N as i64 / n) as f32;
        (0..N)
            .map(|x| {
                let u = (x as f32 + 0.5) / k - 0.5;
                let (i, t) = (u.floor(), u - u.floor());
                let get = |i: f32| blur[(i as i64).clamp(0, n - 1) as usize];
                get(i) * (1.0 - t) + get(i + 1.0) * t
            })
            .collect()
    }

    /// The 16 % to 84 % width of a blurred edge is two deviations, for each deviation glass blurs with: the backdrop
    /// blurs as one Gaussian of the deviation the recipe asks for, however many halvings the passes take to reach it.
    #[test]
    fn the_blur_passes_give_an_edge_two_deviations_wide_between_16_and_84_percent() {
        for sigma in [4.0f32, 8.0, 16.0, 20.0, 40.0] {
            let row = blurred_edge(sigma);
            // where the row first reaches `level`, between texels
            let find = |level: f32| {
                let i = row.iter().position(|&v| v >= level).unwrap();
                let (a, b) = (row[i - 1], row[i]);
                i as f32 - 1.0 + (level - a) / (b - a)
            };
            let width = find(0.841_344_7) - find(0.158_655_3);
            assert!(
                (width - 2.0 * sigma).abs() < 0.08 * sigma + 0.5,
                "σ {sigma}: {width}"
            );
        }
    }

    #[test]
    fn blur_fill_drops_thin_bright_detail_on_dark_glass_and_dark_detail_on_light_glass() {
        // a bright line whose wider blur is darker
        let (line, around) = ([0.9; 3], [0.3; 3]);
        let dark = blur_fill(line, around, &params(true));
        assert!(dark[0] < 0.4, "darken 0.9 then normal 0.5: {dark:?}");
        let light = blur_fill(around, line, &params(false));
        assert!(light[0] > 0.8, "lighten 0.9 then normal 0.5: {light:?}");
        // flat backdrops pass unchanged
        assert_eq!(blur_fill([0.6; 3], [0.6; 3], &params(true)), [0.6; 3]);
    }

    #[test]
    fn dark_face_compresses_bright_backdrops_and_keeps_saturated_ones() {
        let p = params(true);
        // a near-white grey x = 0.953 is compressed to x − 0.625·x², then the levels add 0.08·(1 − L) + 0.125·L
        let x = 0.953f32;
        let l = x - (1.0 - 0.375) * x * x;
        let expect = l + 0.08 * (1.0 - l) + 0.125 * l;
        let white = face_color([x; 3], &p, p.face.max_luma);
        assert!(close(white[0], expect, 1e-5), "{white:?} vs {expect}");
        assert!(close(white[0] * 255.0, 123.1, 0.2), "{white:?}");
        // a saturated blue keeps its hue and grows more saturated: about (35, 78, 255)
        let blue = face_color([0.12, 0.27, 0.93], &p, p.face.max_luma).map(|v| v * 255.0);
        assert!(
            close(blue[0], 35.4, 0.5) && close(blue[1], 78.4, 0.5),
            "{blue:?}"
        );
        assert_eq!(blue[2], 255.0);
        // black is lifted by the black level alone
        let black = face_color([0.0; 3], &p, p.face.max_luma);
        assert!(close(black[0], 0.08, 1e-6));
    }

    /// The face's steps on known inputs, each worked out by hand from its formula.
    #[test]
    fn the_face_compresses_luma_then_shifts_every_channel_by_its_levels() {
        // compression: a grey x becomes x − (1 − m)·x², non-monotonic below m = 0.5
        for (x, m) in [(1.0f32, 0.6f32), (0.5, 0.6), (0.8, 0.358), (1.0, 0.358)] {
            let c = compress_luma([x; 3], m);
            assert!(close(c[0], x - (1.0 - m) * x * x, 1e-6), "{x} {m}");
        }
        assert!(compress_luma([1.0; 3], 0.358)[0] < compress_luma([0.8; 3], 0.358)[0]);
        // a colour: c − (1 − m)·L·(0.75c + 0.25L), each channel against the luma of the whole
        let red = [0.9f32, 0.2, 0.1];
        let l = 0.2126 * 0.9 + 0.7152 * 0.2 + 0.0722 * 0.1;
        let c = compress_luma(red, 0.5);
        for i in 0..3 {
            assert!(close(
                c[i],
                red[i] - 0.5 * l * (0.75 * red[i] + 0.25 * l),
                1e-6
            ));
        }
        // the levels: one shift black·(1 − L) − (1 − white)·L on every channel
        let shifted = face_levels([0.6, 0.4, 0.2], 0.1, 0.8);
        let l = 0.2126 * 0.6 + 0.7152 * 0.4 + 0.0722 * 0.2;
        let shift = 0.1 * (1.0 - l) - 0.2 * l;
        assert!(close(shifted[0], 0.6 + shift, 1e-6) && close(shifted[2], 0.2 + shift, 1e-6));
        // and in order, with the dark regular values: white comes out at 180 over a control up to 60 points high,
        // and at 116 over a panel whose recipe compresses to 0.358
        let mut p = params(true);
        let white = |p: &GlassParams, m: f32| face_color([1.0; 3], p, m)[0] * 255.0;
        assert!(close(white(&p, 0.6), 180.3, 0.2), "{}", white(&p, 0.6));
        assert!(close(white(&p, 0.358), 115.8, 0.2), "{}", white(&p, 0.358));
        let small = GlassParams::new(
            GlassMode {
                dark: true,
                ..GlassMode::default()
            },
            44.0,
        );
        assert!(close(white(&small, small.face.max_luma), 180.3, 0.2));
        // the fill lies over it all
        p.face.fill = [1.0, 1.0, 1.0, 0.5];
        assert!(close(white(&p, 0.6), (180.28 + 255.0) * 0.5, 0.2));
    }

    #[test]
    fn saturation_works_on_encoded_values_about_their_luma() {
        // greys and saturation 1 pass unchanged, and the luma of the encoded colour is kept
        assert_eq!(saturate_encoded([0.4; 3], 1.3), [0.4; 3]);
        let c = [0.157, 0.094, 0.655];
        assert_eq!(saturate_encoded(c, 1.0), c);
        let s = saturate_encoded(c, 1.3);
        assert!(close(luma(s), luma(c), 1e-6), "{s:?}");
        // each channel moves away from the luma by the factor
        let y = luma(c);
        for i in 0..3 {
            assert!(close(s[i] - y, (c[i] - y) * 1.3, 1e-6), "{s:?}");
        }
        // a channel pushed below black stops there
        assert_eq!(saturate_encoded([0.0, 0.0, 1.0], 3.0)[0], 0.0);
    }

    #[test]
    fn dark_face_keeps_a_milky_floor_over_a_strong_blue() {
        // a deep blue wallpaper behind a control tile (encoded 40, 24, 167 of 255)
        let p = GlassParams::new(
            GlassMode {
                dark: true,
                ..GlassMode::default()
            },
            64.0,
        );
        let face = face_color(
            [40.0 / 255.0, 24.0 / 255.0, 167.0 / 255.0],
            &p,
            p.face.max_luma,
        )
        .map(|v| v * 255.0);
        // about (60, 40, 218): brighter and bluer than the wallpaper, and the weak green stays well above the lift
        // alone (0.08, 20 of 255), which is where saturating linear light would leave it
        assert!(close(face[0], 60.2, 1.0), "{face:?}");
        assert!(close(face[1], 40.4, 1.0), "{face:?}");
        assert!(close(face[2], 217.6, 1.0), "{face:?}");
        // a strong red keeps green and blue near what the backdrop had (54 and 59 of 255), not lifted black
        let red = face_color(
            [227.0 / 255.0, 54.0 / 255.0, 59.0 / 255.0],
            &p,
            p.face.max_luma,
        );
        assert!(red[1] > 0.18 && red[2] > 0.2, "{red:?}");
    }

    #[test]
    fn the_dark_large_panel_compresses_white_and_black_and_keeps_strong_colours() {
        let face = |dark: bool, style: GlassStyle, c: [f32; 3]| {
            let p = GlassParams::new(
                GlassMode {
                    dark,
                    style,
                    ..GlassMode::default()
                },
                400.0,
            );
            face_color(c.map(|v| v / 255.0), &p, p.face.max_luma).map(|v| v * 255.0)
        };
        let large = |c| face(true, GlassStyle::LargePanel, c);
        // dark: a white page behind comes out a mid grey (about 125) and black a dark grey (about 53); regular glass
        // of the same size takes white a little darker (about 118) and black much darker (about 35)
        let white = large([245.0; 3]);
        assert!(
            close(white[0], 125.0, 3.0) && close(white[0], white[2], 0.5),
            "{white:?}"
        );
        let black = large([15.0; 3]);
        assert!(close(black[0], 53.0, 3.0), "{black:?}");
        let regular_white = face(true, GlassStyle::Regular, [245.0; 3]);
        assert!(close(regular_white[0], 118.5, 1.0), "{regular_white:?}");
        let regular_black = face(true, GlassStyle::Regular, [15.0; 3]);
        assert!(close(regular_black[0], 35.5, 1.0), "{regular_black:?}");
        // a saturated red band keeps its colour and much of its brightness: about (205, 69, 73)
        let red = large([229.0, 55.0, 60.0]);
        assert!(close(red[0], 205.0, 4.0), "{red:?}");
        assert!(
            close(red[1], 69.0, 4.0) && close(red[2], 73.0, 4.0),
            "{red:?}"
        );
        assert!(
            red[0] > white[0] + 60.0,
            "brighter than the grey of white: {red:?}"
        );
    }

    /// The light large panel is a bright sheet: black behind comes through as a light grey (about 133 of 255 over a
    /// near-black 15), anything from a mid tone up clips to white, and a dark warm scene leaves a light tint of its own
    /// hue rather than grey.
    #[test]
    fn the_light_large_panel_lifts_black_to_a_light_grey_and_keeps_tints() {
        let p = GlassParams::new(
            GlassMode {
                dark: false,
                style: GlassStyle::LargePanel,
                ..GlassMode::default()
            },
            400.0,
        );
        let face =
            |c: [f32; 3]| face_color(c.map(|v| v / 255.0), &p, p.face.max_luma).map(|v| v * 255.0);
        let black = face([15.0; 3]);
        assert!(
            close(black[0], 133.0, 2.0) && close(black[0], black[2], 0.5),
            "{black:?}"
        );
        let grey = face([128.0; 3]);
        assert!(grey[0] > 200.0, "{grey:?}");
        let white = face([245.0; 3]);
        assert!(white.iter().all(|&v| v > 254.0), "{white:?}");
        // a dark warm brown: red stays well above blue
        let warm = face([90.0, 50.0, 30.0]);
        assert!(
            warm[0] > warm[1] && warm[1] > warm[2] && warm[0] - warm[2] > 30.0,
            "{warm:?}"
        );
        // the regular light face takes black only to about 130 and greys the brown more
        let regular = GlassParams::new(GlassMode::default(), 400.0);
        let r = face_color(
            [90.0 / 255.0, 50.0 / 255.0, 30.0 / 255.0],
            &regular,
            regular.face.max_luma,
        );
        assert!(warm[0] - warm[2] > (r[0] - r[2]) * 255.0, "{warm:?} {r:?}");
    }

    /// With Reduce Transparency the dark large panel is near black (about 20 of 255) whatever grey lies behind, and a
    /// strong colour behind leaves only a faint tint, under half of what regular glass keeps: a saturated blue comes
    /// through as about (16, 20, 36).
    #[test]
    fn the_dark_large_panel_with_reduce_transparency_is_near_black_with_a_faint_tint() {
        let p = |style| {
            GlassParams::new(
                GlassMode {
                    dark: true,
                    style,
                    reduce_transparency: true,
                    ..GlassMode::default()
                },
                400.0,
            )
        };
        let (large, regular) = (p(GlassStyle::LargePanel), p(GlassStyle::Regular));
        let face = |g: &GlassParams, c: [f32; 3]| {
            face_color(c.map(|v| v / 255.0), g, g.face.max_luma).map(|v| v * 255.0)
        };
        for grey in [10.0, 128.0, 245.0] {
            let c = face(&large, [grey; 3]);
            assert!(c.iter().all(|&v| close(v, 20.4, 0.5)), "{grey}: {c:?}");
        }
        let blue = [35.0, 80.0, 235.0];
        let tint = face(&large, blue);
        assert!(
            tint[2] - tint[0] > 8.0 && tint[2] - tint[0] < 25.0,
            "{tint:?}"
        );
        assert!(tint.iter().all(|&v| v < 40.0), "{tint:?}");
        let strong = face(&regular, blue);
        assert!(
            strong[2] - strong[0] > 2.0 * (tint[2] - tint[0]),
            "{strong:?} {tint:?}"
        );
    }

    /// The blurred backdrop `x` points along a backdrop that is `source(t)` at `t` points, through `p`: each blur level
    /// and the blur fill as a Gaussian of its radius, laid over each other as the shader does.
    fn blurred(p: &GlassParams, source: &impl Fn(f32) -> Rgb, x: f32) -> Rgb {
        let gauss = |sigma: f32| -> Rgb {
            let (step, mut sum, mut acc) = (0.125f32, 0.0f32, [0.0f32; 3]);
            let reach = (4.0 * sigma / step).ceil() as i32;
            for i in -reach..=reach {
                let t = i as f32 * step;
                let w = (-0.5 * (t / sigma).powi(2)).exp();
                let c = source(x - t);
                sum += w;
                for k in 0..3 {
                    acc[k] += w * c[k];
                }
            }
            acc.map(|v| v / sum)
        };
        blur_fill(gauss(p.blur_sigma()), gauss(p.fill_sigma()), p)
    }

    /// The large panel blurs twice as wide as regular glass of its size: a step from white to black behind it is
    /// blurred over about twice the distance (10 % to 90 % of the way), and a hairline bright stripe over black, like the
    /// lines of a busy picture, comes through much fainter, so fine detail melts into the colour around it instead of
    /// rippling through.
    #[test]
    fn the_large_panel_blurs_twice_as_wide_as_regular_glass() {
        for dark in [true, false] {
            let p = |style| {
                GlassParams::new(
                    GlassMode {
                        dark,
                        style,
                        ..GlassMode::default()
                    },
                    400.0,
                )
            };
            let (regular, large) = (p(GlassStyle::Regular), p(GlassStyle::LargePanel));
            let step = |t: f32| if t < 0.0 { [0.96; 3] } else { [0.06; 3] };
            // the distance over which the blurred step goes from 10 % to 90 % of its way, in points
            let width = |g: &GlassParams| {
                let xs: Vec<f32> = (-400..=400).map(|i| i as f32 * 0.25).collect();
                let ys: Vec<f32> = xs.iter().map(|&x| luma(blurred(g, &step, x))).collect();
                let (hi, lo) = (ys[0], ys[ys.len() - 1]);
                let at = |f: f32| {
                    let level = hi - (hi - lo) * f;
                    xs[ys.iter().position(|&y| y <= level).unwrap()]
                };
                at(0.9) - at(0.1)
            };
            let (w_regular, w_large) = (width(&regular), width(&large));
            assert!(
                w_large > 1.6 * w_regular && w_large < 2.4 * w_regular,
                "dark {dark}: {w_regular} pt and {w_large} pt"
            );
            // a bright stripe 6 points wide over black: how far its peak stands above the black around it, before the
            // face (which differs between the two in the light appearance)
            let stripe = |t: f32| if t.abs() < 3.0 { [0.96; 3] } else { [0.06; 3] };
            let ripple = |g: &GlassParams| {
                (luma(blurred(g, &stripe, 0.0)) - luma(blurred(g, &stripe, 200.0))) * 255.0
            };
            let (r_regular, r_large) = (ripple(&regular), ripple(&large));
            assert!(
                r_large < 0.6 * r_regular,
                "dark {dark}: ripple {r_regular} and {r_large}"
            );
        }
    }

    #[test]
    fn light_face_stays_light_and_takes_its_white_fill() {
        let p = params(false);
        // white: compressed to 0.94, the levels take 0.4·0.06 − 0.04·0.94 off, then white at 0.184 over it: 240
        let white = face_color([1.0; 3], &p, p.face.max_luma);
        let l = 0.94f32;
        let expect = l + 0.4 * (1.0 - l) - 0.04 * l;
        assert!(
            close(white[0], expect + (1.0 - expect) * 0.184, 1e-5),
            "{white:?}"
        );
        assert!(close(white[0] * 255.0, 239.7, 0.2), "{white:?}");
        // black: lifted by the black level 0.4, then the white fill over it: 130
        let black = face_color([0.0; 3], &p, p.face.max_luma);
        assert!(close(black[0], 0.4 + 0.6 * 0.184, 1e-5), "{black:?}");
        assert!(close(black[0] * 255.0, 130.2, 0.2), "{black:?}");
    }

    #[test]
    fn refraction_reads_further_inside_by_its_profile_across_the_band() {
        let n = [1.0, 0.0];
        // at the edge nine tenths of the strength, against the outward normal for a negative strength
        let at_edge = refraction_offset(0.0, n, -50.0, 20.0);
        assert!(close(at_edge[0], -45.0, 1e-4) && at_edge[1] == 0.0);
        let mid = refraction_offset(-10.0, n, -50.0, 20.0);
        assert!(close(mid[0], -50.0 * refraction_profile(0.5), 1e-4));
        // linear in the strength
        let half = refraction_offset(-10.0, n, -25.0, 20.0);
        assert!(close(half[0] * 2.0, mid[0], 1e-4));
        assert_eq!(
            refraction_offset(-25.0, n, -50.0, 20.0),
            [0.0, 0.0],
            "past the band"
        );
        assert_eq!(
            refraction_offset(-1.0, n, -50.0, 0.0),
            [0.0, 0.0],
            "no band, no bend"
        );
        assert_eq!(refraction_offset(-1.0, n, 0.0, 20.0), [0.0, 0.0]);
    }

    #[test]
    fn bleed_shows_nowhere_at_rest() {
        for dark in [true, false] {
            let p = params(dark);
            assert_eq!(p.bleed.opacity, 0.0);
            assert!(
                p.bleed.amount > 0.0,
                "the plumbing keeps its strength and band"
            );
            let face = [0.3, 0.4, 0.5];
            assert_eq!(bleed(face, [0.9, 0.1, 0.1], 0.0, &p, 35.0, 35.0), face);
            // and the container neither blurs for it nor tells the shader to draw it
            let c = Container::new(
                vec![GlassShape::new([0.0, 0.0, 150.0, 100.0], 18.0)],
                0.0,
                GlassMode {
                    dark,
                    ..GlassMode::default()
                },
            );
            let u = GlassUniform::new(&c, 2.0, [[0.0; 3]; 2], [0.0; 4], [1.0; 2], 1.0);
            assert_eq!(u.shape_a[0][3], 0.0);
        }
    }

    #[test]
    fn bleed_lightens_dark_glass_edges_with_the_backdrop_colour() {
        let mut p = params(true);
        p.bleed.opacity = 1.0;
        let red = [0.9, 0.1, 0.1];
        let face = [0.1; 3];
        let edge = bleed(face, red, 0.0, &p, 35.0, 35.0);
        assert!(edge[0] > face[0] && edge[1] >= face[1], "{edge:?}");
        let inside = bleed(face, red, -40.0, &p, 35.0, 35.0);
        assert_eq!(inside, face, "past the band");
        let mut light = params(false);
        light.bleed.opacity = 1.0;
        let darker = bleed([0.98; 3], [0.0, 0.0, 0.5], 0.0, &light, 35.0, 35.0);
        assert!(darker[0] < 0.98, "light glass darkens: {darker:?}");
    }

    #[test]
    fn a_seam_between_joined_shapes_is_far_from_the_edge() {
        // a wide panel with a thin strip joined under the middle of its bottom edge
        let shapes = [
            GlassShape::new([0.0, 0.0, 132.0, 39.0], 19.5),
            GlassShape::new([55.5, 39.0, 21.0, 1.4], 0.7),
        ];
        let k = 6.0;
        let slope = |p: [f32; 2]| {
            let g = field_gradient(&shapes, k, p);
            (g[0] * g[0] + g[1] * g[1]).sqrt()
        };
        let seam = [66.0, 39.0];
        let d = union_field(&shapes, k, seam).0;
        assert!(d < 0.0 && d > -3.0, "the seam is shallow: {d}");
        assert!(slope(seam) < 0.4, "{}", slope(seam));
        assert!(
            edge_distance(d, slope(seam)) < -10.0,
            "and far from an edge"
        );
        // the panel's own edge away from the strip keeps its distance
        let edge = [20.0, 38.5];
        let d_edge = union_field(&shapes, k, edge).0;
        assert!(close(edge_distance(d_edge, slope(edge)), d_edge, 1e-3));
        let p = params(true);
        let face = [0.5, 0.45, 0.6];
        let rim = |d: f32| highlight(face, d, [std::f32::consts::FRAC_1_SQRT_2; 2], &p);
        let on_edge = rim(edge_distance(d_edge, slope(edge)));
        let across = rim(edge_distance(d, slope(seam)));
        assert!(on_edge[0] < face[0] - 0.01, "{on_edge:?}");
        assert!(
            close(across[0], face[0], 1e-6),
            "no rim across the join: {across:?}"
        );
    }

    #[test]
    fn shadows_are_invisible_at_rest_and_follow_their_values_when_shown() {
        let mut p = params(true);
        assert_eq!(shadow_alpha(0.0, &p), 0.0);
        assert_eq!(ring_shadow(-1.0, &p), 0.0);
        p.shadow.opacity = 1.0;
        p.shadow.amount = 50.0;
        p.shadow.height = 10.0;
        assert!(close(shadow_alpha(0.0, &p), 0.5 * 0.3 * 0.5, 1e-4));
        assert_eq!(shadow_alpha(40.0, &p), 0.0);
    }

    /// The ring shadow is a stroke of its width inside the outline, moved up by its offset and blurred with a
    /// Gaussian of its deviation, darkening the face by its opacity; nothing outside the shape.
    #[test]
    fn the_ring_shadow_darkens_a_blurred_stroke_inside_the_outline_moved_up() {
        let mut p = params(true);
        // stroke 4, offset 8, deviation 5 at rest, with no opacity
        assert_eq!(
            (
                p.shadow.ring_stroke,
                p.shadow.ring_offset,
                p.shadow.ring_blur
            ),
            (4.0, 8.0, 5.0)
        );
        p.shadow.ring_opacity = 0.4;
        // unblurred: the stroke covers depths 0 to 4 and nothing else
        p.shadow.ring_blur = 0.0;
        assert_eq!(ring_shadow(-2.0, &p), 0.4);
        assert_eq!(ring_shadow(-5.0, &p), 0.0);
        assert_eq!(ring_shadow(1.0, &p), 0.0);
        // blurred: strongest in the middle of the stroke, the share of a Gaussian of 5 over its 4 points
        p.shadow.ring_blur = 5.0;
        let mid = ring_shadow(-2.0, &p);
        let expect = 0.4 * (normal_cdf(0.4) - normal_cdf(-0.4));
        assert!(close(mid, expect, 1e-6), "{mid} vs {expect}");
        assert!(ring_shadow(-12.0, &p) < 0.25 * mid && ring_shadow(-12.0, &p) > 0.0);
        // the logistic stands for the normal distribution within 0.01
        for (x, phi) in [
            (0.0, 0.5),
            (1.0, 0.841_344_7),
            (-2.0, 0.022_750_1),
            (0.5, 0.691_462_5),
        ] {
            assert!(close(normal_cdf(x), phi, 0.01), "{x}");
        }
        // on a shape, the ring is moved up: the face is darkened most a ring offset below the top edge, and only
        // faintly just inside the bottom edge
        p.shadow.ring_blur = 0.0;
        let mut c = Container::new(
            vec![GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0)],
            0.0,
            GlassMode {
                dark: true,
                ..GlassMode::default()
            },
        );
        c.params.shadow = p.shadow;
        let grey = |_: usize, _: [f32; 2]| [0.5f32; 3];
        let at = |y: f32| c.shade([100.0, y], &grey).0[0];
        let plain = face_color([0.5; 3], &c.params, c.values[0].max_luma)[0];
        // under the top edge the ring sits 8 points higher, outside the shape: nothing
        assert!(close(at(2.0), plain, 1e-5), "{} vs {plain}", at(2.0));
        // the ring along the bottom edge, moved up 8: depths 8 to 12 above the bottom
        assert!(close(at(100.0 - 10.0), plain * 0.6, 1e-4), "{}", at(90.0));
        assert!(close(at(100.0 - 2.0), plain, 1e-5));
        // and nothing at rest
        let rest = Container::new(c.shapes.clone(), 0.0, c.mode);
        assert!(close(rest.shade([100.0, 90.0], &grey).0[0], plain, 1e-5));
    }

    /// Regular glass keeps a line one unit wide just outside its outline: black at the edge line's strength times the
    /// The veil darkens the face by its top opacity at the top of the shapes and its bottom opacity at their bottom,
    /// linearly between, over the whole union of shapes, and the uniform carries both ends.
    #[test]
    fn the_veil_darkens_the_face_from_its_top_value_to_its_bottom_value() {
        let mut c = Container::new(
            vec![
                GlassShape::new([0.0, 0.0, 200.0, 50.0], 20.0),
                GlassShape::new([0.0, 50.0, 200.0, 50.0], 20.0),
            ],
            0.0,
            GlassMode::default(),
        );
        let face = [0.8, 0.6, 0.4];
        assert_eq!(c.veil([100.0, 50.0], face), face, "none at rest");
        c.params.veil = [0.2, 1.0];
        for (y, k) in [
            (0.0, 0.8),
            (25.0, 0.6),
            (50.0, 0.4),
            (100.0, 0.0),
            (-10.0, 0.8),
            (130.0, 0.0),
        ] {
            let v = c.veil([100.0, y], face);
            for i in 0..3 {
                assert!(close(v[i], face[i] * k, 1e-5), "{y}: {v:?}");
            }
        }
        let u = GlassUniform::new(&c, 2.0, [[0.0; 3]; 2], [0.0; 4], [400.0, 200.0], 1.0);
        assert_eq!((u.fill[3], u.levels[1]), (0.2, 1.0));
        // the veiled style shades a dark gradient: its middle is half as bright as its top over the same backdrop
        let veiled = Container::new(
            vec![GlassShape::new([0.0, 0.0, 200.0, 100.0], 30.0)],
            0.0,
            GlassMode {
                style: GlassStyle::Veiled,
                ..GlassMode::default()
            },
        );
        let flat = |_: usize, _: [f32; 2]| [0.5; 3];
        let top = veiled.shade([100.0, 30.0], &flat).0[0];
        let bottom = veiled.shade([100.0, 70.0], &flat).0[0];
        assert!(bottom < top * 0.6, "{top} {bottom}");
    }

    /// rim's brightness response to what lies there, so it shows over white and grey and not over black.
    #[test]
    fn the_edge_line_darkens_light_content_just_outside_the_outline_and_not_black() {
        let c = Container::new(
            vec![GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0)],
            0.0,
            GlassMode::default(),
        );
        let k = c.params.shadow.edge_line;
        assert_eq!(k, 0.2);
        let flat = |v: f32| move |_: usize, _: [f32; 2]| [v; 3];
        // a pixel whose centre lies half a unit outside the bottom edge: the whole line
        for (v, want) in [(1.0, k), (0.5, k), (0.25, k * 0.63), (0.0, 0.0)] {
            let (col, a) = c.shade([100.0, 100.5], &flat(v));
            assert!(close(a, want, 1e-4), "{v}: {a}");
            if a > 0.0 {
                assert_eq!(col, [0.0; 3], "black");
            }
        }
        // it fades over the next half unit and is gone past it
        let (_, half) = c.shade([100.0, 101.0], &flat(1.0));
        assert!(close(half, k * 0.5, 1e-4), "{half}");
        assert_eq!(c.shade([100.0, 101.6], &flat(1.0)).1, 0.0);
        // a pixel the edge crosses shows the line under the part the glass leaves open
        let (_, a) = c.shade([100.0, 100.0], &flat(1.0));
        assert!(close(a, 0.5 + 0.5 * k, 1e-4), "{a}");
        // clear glass draws none
        let clear = Container::new(
            vec![GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0)],
            0.0,
            GlassMode {
                style: GlassStyle::Clear,
                ..GlassMode::default()
            },
        );
        assert_eq!(clear.shade([100.0, 100.5], &flat(1.0)).1, 0.0);
        // the uniform carries it to the shaders
        let u = GlassUniform::new(&c, 2.0, [[0.0; 3]; 2], [0.0; 4], [400.0, 200.0], 1.0);
        assert_eq!(u.levels[0], k);
    }

    /// An edge line kept to the sides follows the outline's direction: all of it along a straight side, none along the
    /// top and the bottom, and a raised cosine round a corner between them; a recipe without arcs keeps it all round.
    #[test]
    fn an_edge_line_kept_to_the_sides_draws_none_along_the_top_and_the_bottom() {
        let quarter = std::f32::consts::FRAC_PI_2;
        assert_eq!(edge_line_side_share([1.0, 0.0], quarter), 1.0);
        assert_eq!(edge_line_side_share([-3.0, 0.0], quarter), 1.0);
        assert_eq!(edge_line_side_share([0.0, 1.0], quarter), 0.0);
        assert_eq!(edge_line_side_share([0.0, -1.0], quarter), 0.0);
        // 45° from the horizontal, two thirds of the way to the arc's end of 67.5°
        let corner = edge_line_side_share([1.0, 1.0], quarter);
        assert!(close(corner, 0.25, 1e-4), "{corner}");
        assert_eq!(
            edge_line_side_share([0.0, 1.0], 0.0),
            1.0,
            "no arcs: all round"
        );
        assert_eq!(edge_line_side_share([0.0, 0.0], quarter), 1.0, "no normal");

        let tile = Container::new(
            vec![GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0)],
            0.0,
            GlassMode {
                dark: true,
                style: GlassStyle::Module,
                ..GlassMode::default()
            },
        );
        let k = tile.params.shadow.edge_line;
        assert!(k > 0.0);
        let flat = |_: usize, _: [f32; 2]| [0.8; 3];
        // half a unit outside each side: the whole line, black, as dark over a dark backdrop as over a light one (the
        // module's line is even); above the top and below the bottom: nothing
        for p in [[-0.5, 50.0], [200.5, 50.0]] {
            for v in [0.8, 0.2] {
                let (col, a) = tile.shade(p, &|_, _| [v; 3]);
                assert!(close(a, k, 1e-4), "{p:?} over {v}: {a}");
                assert_eq!(col, [0.0; 3]);
            }
        }
        for p in [[100.0, -0.5], [100.0, 100.5]] {
            assert_eq!(tile.shade(p, &flat).1, 0.0, "{p:?}");
        }
        // round a corner it fades from the side to the top
        let at = |deg: f32| {
            let a = deg.to_radians();
            let (cx, cy) = (180.0, 20.0);
            tile.shade([cx + 20.5 * a.cos(), cy - 20.5 * a.sin()], &flat)
                .1
        };
        let (side, mid, top) = (at(0.0), at(45.0), at(80.0));
        assert!(side > mid && mid > top && top == 0.0, "{side} {mid} {top}");
        // the uniform carries the arcs' width and the even line to the shaders
        let u = GlassUniform::new(&tile, 2.0, [[0.0; 3]; 2], [0.0; 4], [400.0, 200.0], 1.0);
        assert_eq!((u.diffuse[3], u.cut_shape[3]), (quarter, 1.0));
        let regular = Container::new(
            vec![GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0)],
            0.0,
            GlassMode::default(),
        );
        let u = GlassUniform::new(&regular, 2.0, [[0.0; 3]; 2], [0.0; 4], [400.0, 200.0], 1.0);
        assert_eq!(
            (u.diffuse[3], u.cut_shape[3]),
            (0.0, 0.0),
            "all round, following the backdrop"
        );
    }

    /// Module tiles light a line about a point wide just inside their top and bottom edges that takes a mid-toned face
    /// to near white, as bright at the bottom as at the top, and none down their sides.
    #[test]
    fn module_tiles_light_their_top_and_bottom_near_white_and_not_their_sides() {
        let tile = Container::new(
            vec![GlassShape::new([0.0, 0.0, 140.0, 64.0], 32.0)],
            0.0,
            GlassMode {
                style: GlassStyle::Module,
                ..GlassMode::default()
            },
        );
        let warm = [204.0 / 255.0, 172.0 / 255.0, 105.0 / 255.0];
        let flat = |_: usize, _: [f32; 2]| warm;
        let face = luma(tile.shade([70.0, 32.0], &flat).0);
        // half a point inside the top and the bottom
        let top = luma(tile.shade([70.0, 0.5], &flat).0);
        let bottom = luma(tile.shade([70.0, 63.5], &flat).0);
        assert!(face < 0.55, "a mid-toned face: {face}");
        assert!(top > 0.9 && bottom > 0.9, "near white: {top} {bottom}");
        assert!((top - bottom).abs() < 0.02, "alike: {top} {bottom}");
        // a point in, the line has faded to a trace
        let inside = luma(tile.shade([70.0, 1.25], &flat).0);
        assert!(inside - face < 0.1, "{inside} vs {face}");
        // at the middle of a side there is no line of light
        let side = luma(tile.shade([0.75, 32.0], &flat).0);
        assert!((side - face).abs() < 0.05, "{side} vs {face}");
    }

    /// The dark large panel's bevel: just inside its dark edge a rim about a point wide mixes the face about a third of
    /// the way to white along the top and the bottom, and about two thirds as far along the sides.
    #[test]
    fn the_dark_large_panel_has_a_bright_bevel_inside_its_edge_all_round() {
        let panel = Container::new(
            vec![GlassShape::new([0.0, 0.0, 400.0, 300.0], 28.0)],
            0.0,
            GlassMode {
                dark: true,
                style: GlassStyle::LargePanel,
                ..GlassMode::default()
            },
        );
        let flat = |_: usize, _: [f32; 2]| [0.5; 3];
        let face = luma(panel.shade([200.0, 150.0], &flat).0);
        // how far towards white the pixel at p is
        let mix = |p: [f32; 2]| (luma(panel.shade(p, &flat).0) - face) / (1.0 - face);
        // half a point inside each edge
        let (top, bottom) = (mix([200.0, 0.5]), mix([200.0, 299.5]));
        let (left, right) = (mix([0.5, 150.0]), mix([399.5, 150.0]));
        assert!((0.28..0.42).contains(&top), "{top}");
        assert!((top - bottom).abs() < 0.03, "{top} {bottom}");
        for side in [left, right] {
            assert!((0.5..0.85).contains(&(side / top)), "{side} vs {top}");
        }
        // a point in it has faded to under half, and a point and a half in it is gone but for the soft band
        let point = mix([200.0, 1.0]);
        assert!(point < 0.5 * top, "{point} vs {top}");
        assert!(mix([200.0, 1.75]) < 0.08);
        // the light panel keeps its own inner line
        let light = GlassParams::new(
            GlassMode {
                style: GlassStyle::LargePanel,
                ..GlassMode::default()
            },
            300.0,
        );
        assert_eq!(light.highlight.diffuse, GlowDiffuse::NONE);
    }

    /// A pixel the edge crosses shows the glass over the outline in the part the glass leaves uncovered, so the
    /// edge's alpha and colour run on without a step from the pixel just inside the edge to the one just outside.
    #[test]
    fn an_edge_pixel_shows_the_outline_under_the_part_the_glass_leaves_open() {
        let mut c = Container::new(
            vec![GlassShape::new([0.0, 0.0, 200.0, 100.0], 20.0)],
            0.0,
            GlassMode::default(),
        );
        c.params.shadow.outline_opacity = 0.32;
        c.params.shadow.outline_width = 1.0;
        c.params.shadow.edge_line = 0.0;
        let white = |_: usize, _: [f32; 2]| [1.0f32; 3];
        let fill = c.params.shadow.fill;
        // over a white backdrop, as a compositor puts it
        let seen = |y: f32| {
            let (col, a) = c.shade([100.0, y], &white);
            col.map(|v| v * a + (1.0 - a))
        };
        // half the pixel is glass and half the outline: alpha ½ + ½ · 0.32
        let (_, a) = c.shade([100.0, 100.0], &white);
        assert!(close(a, 0.5 + 0.5 * 0.32, 1e-5), "{a}");
        // just inside and just outside the outer edge of the pixel row the result is the same
        let inside = seen(100.5 - 1e-3);
        let outside = seen(100.5 + 1e-3);
        for i in 0..3 {
            assert!(
                close(inside[i], outside[i], 1e-3),
                "{inside:?} vs {outside:?}"
            );
            assert!(close(outside[i], 1.0 - 0.32 * (1.0 - fill[i]), 1e-3));
        }
        // and a shape with no outline leaves the open part to the backdrop
        c.params.shadow.outline_opacity = 0.0;
        let (_, a) = c.shade([100.0, 100.0], &white);
        assert!(close(a, 0.5, 1e-5), "{a}");
    }

    /// The rim adds `amount · color_bias` (darker for a negative bias) to every channel, scaled by the face's
    /// brightness (0, 0.63, 1, 1.13, 1 at luma 0, ¼, ½, ¾, 1) and the edge's direction (0.15 + 0.85·sin²(2φ)), in a band
    /// 1.7 times its height deep.
    #[test]
    fn the_rim_darkens_most_on_diagonal_edges_of_a_mid_face() {
        let p = params(true);
        assert_eq!((p.highlight.amount, p.highlight.color_bias), (0.5, -0.1875));
        // a mid grey: on a straight edge −0.5·0.1875·1·0.15 of a level, on a diagonal all of it
        let mid = [0.5f32; 3];
        let straight = rim_shift(mid, -0.1, [0.0, -1.0], &p);
        assert!(
            close(straight * 255.0, -0.5 * 0.1875 * 0.15 * 255.0, 1e-3),
            "{straight}"
        );
        assert!(close(straight * 255.0, -3.6, 0.05));
        let d = std::f32::consts::FRAC_1_SQRT_2;
        let diagonal = rim_shift(mid, -0.1, [d, d], &p);
        assert!(close(diagonal * 255.0, -23.9, 0.05), "{}", diagonal * 255.0);
        // the four diagonals and the four sides alike
        for n in [[d, -d], [-d, d], [-d, -d]] {
            assert!(close(rim_shift(mid, -0.1, n, &p), diagonal, 1e-6));
        }
        for n in [[1.0, 0.0], [-1.0, 0.0], [0.0, 1.0]] {
            assert!(close(rim_shift(mid, -0.1, n, &p), straight, 1e-6));
        }
        // 22.5° off a side: half way in sin²(2φ)
        let (s, c) = std::f32::consts::FRAC_PI_8.sin_cos();
        let between = rim_shift(mid, -0.1, [c, s], &p);
        assert!(close(between / diagonal, 0.15 + 0.85 * 0.5, 1e-5));
        // by brightness: nothing on black, 0.63 at a quarter, 1.13 at three quarters, 1 at white
        let shift = |l: f32| rim_shift([l; 3], -0.1, [d, d], &p) / diagonal;
        assert_eq!(shift(0.0), 0.0);
        for (l, g) in [
            (0.25, 0.63),
            (0.5, 1.0),
            (0.75, 1.13),
            (1.0, 1.0),
            (0.375, 0.815),
        ] {
            assert!(close(shift(l), g, 1e-4), "{l}: {}", shift(l));
        }
        // the band: 1.7 · 0.5 = 0.85 points deep, all of it on the edge, half at its depth, none a half point past it
        assert_eq!(rim_shift(mid, 0.0, [d, d], &p), diagonal);
        assert!(close(
            rim_shift(mid, -0.85, [d, d], &p),
            0.5 * diagonal,
            1e-6
        ));
        assert_eq!(rim_shift(mid, -1.35, [d, d], &p), 0.0);
        // at twice the scale the band is 1.7 pixels deep: the first pixel all of it, the second 0.7, the third none
        assert_eq!(rim_band(0.5, 1.7), 1.0);
        assert!(close(rim_band(1.5, 1.7), 0.7, 1e-6));
        assert_eq!(rim_band(2.5, 1.7), 0.0);
        // a positive bias lightens, as an interaction state may ask
        let mut lit = p;
        lit.highlight.color_bias = 0.25;
        assert!(rim_shift(mid, -0.1, [d, d], &lit) > 0.0);
        // applied to the face, clamped
        let dark = highlight(mid, -0.1, [d, d], &p);
        assert!(close(dark[0], 0.5 + diagonal, 1e-6) && close(dark[2], 0.5 + diagonal, 1e-6));
    }

    #[test]
    fn the_soft_band_lights_the_top_and_the_bottom_edge_and_not_the_sides() {
        let mut p = params(true);
        p.highlight.amount = 0.0;
        assert_eq!(
            glow([0.2; 3], -0.5, [0.0, -1.0], &p),
            [0.2; 3],
            "none at rest"
        );
        p.highlight.glow_amount = 0.1;
        p.highlight.glow_height = 4.0;
        let face = [0.2, 0.15, 0.6];
        let top = highlight(face, -1.0, [0.0, -1.0], &p);
        let bottom = highlight(face, -1.0, [0.0, 1.0], &p);
        let side = highlight(face, -1.0, [1.0, 0.0], &p);
        for i in 0..3 {
            assert!(top[i] > face[i], "{top:?}");
            assert!(bottom[i] > face[i], "{bottom:?}");
        }
        assert!(
            bottom[0] < top[0],
            "a little less at the far end: {bottom:?} {top:?}"
        );
        assert_eq!(side, face, "nothing at the sides");
        assert_eq!(highlight_lobe(0.0, 0.0, 1.0, 0.75), 1.0);
        let spread = p.highlight.spread;
        assert_eq!(highlight_lobe(spread * LOBE_REACH, 0.0, spread, 0.75), 0.0);
    }

    #[test]
    fn the_bar_lights_a_thin_line_along_its_top_and_bottom_over_a_wide_faint_glow() {
        for dark in [false, true] {
            let mode = GlassMode {
                dark,
                style: GlassStyle::Bar,
                ..GlassMode::default()
            };
            let mut p = GlassParams::new(mode, 84.0);
            p.highlight.amount = 0.0;
            let face = [0.2, 0.16, 0.14];
            let at = |d: f32, n: [f32; 2], p: &GlassParams| glow(face, d, n, p);
            let (top, bottom, side) = ([0.0, -1.0], [0.0, 1.0], [1.0, 0.0]);
            // the top and the bottom edge alike, the ends of the long axis not at all
            assert_eq!(at(-0.5, top, &p), at(-0.5, bottom, &p));
            assert!(
                at(-0.5, top, &p)[0] > face[0] + 0.15,
                "{:?}",
                at(-0.5, top, &p)
            );
            assert_eq!(at(-0.5, side, &p), face);
            // the line is thin: three points in, only the faint glow is left, and without it nothing
            let deep = at(-3.5, top, &p)[0] - face[0];
            assert!(deep > 0.0 && deep < 0.05, "{deep}");
            let mut no_glow = p;
            no_glow.highlight.diffuse = GlowDiffuse::NONE;
            assert!(at(-3.5, top, &no_glow)[0] - face[0] < 1e-3);
            // the glow lights narrower arcs than the line: none on a corner's diagonal, which the line still lights
            let diagonal = [
                std::f32::consts::FRAC_1_SQRT_2,
                -std::f32::consts::FRAC_1_SQRT_2,
            ];
            assert_eq!(at(-6.0, diagonal, &p), face);
            assert!(at(-0.5, diagonal, &p)[0] > face[0]);
        }
    }

    #[test]
    fn foreground_follows_the_face_under_the_glyph() {
        let dark = foreground([0.3, 0.2, 0.6], &foreground_matrix(true), 1.0);
        assert_eq!(dark, [1.0, 1.0, 1.0], "lifted to white");
        let light = foreground([0.9, 0.8, 0.95], &foreground_matrix(false), 1.0);
        assert_eq!(light, [0.0; 3]);
        let half = foreground([0.4; 3], &foreground_matrix(true), 0.5);
        assert!(close(half[0], 0.7, 1e-5));
    }

    #[test]
    fn container_reads_its_bounds_plus_the_reach() {
        let c = Container::new(
            vec![
                GlassShape::new([10.0, 10.0, 150.0, 100.0], 18.0),
                GlassShape::new([165.0, 10.0, 150.0, 100.0], 18.0),
            ],
            20.0,
            GlassMode::default(),
        );
        assert_eq!(c.bounds(), Some([10.0, 10.0, 305.0, 100.0]));
        // the reach, and the bulge of the union: a quarter of the spacing
        let r = c.params.reach() + 5.0;
        assert_eq!(
            c.sample_area(),
            Some([10.0 - r, 10.0 - r, 305.0 + 2.0 * r, 100.0 + 2.0 * r])
        );
    }

    #[test]
    fn shading_a_flat_backdrop_is_flat_inside_and_bent_at_the_edge() {
        let c = Container::new(
            vec![GlassShape::new([0.0, 0.0, 100.0, 100.0], 20.0)],
            0.0,
            GlassMode {
                dark: true,
                ..GlassMode::default()
            },
        );
        let grey = |_: usize, _: [f32; 2]| [0.2f32; 3];
        let (centre, cov) = c.shade([50.0, 50.0], &grey);
        assert_eq!(cov, 1.0);
        let expect = face_color([0.2; 3], &c.params, c.values[0].max_luma);
        assert!(close(centre[0], expect[0], 1e-5));
        // a stripe inside the left edge is drawn out to the edge through refraction (−50 over a 20 point band here)
        let stripe = |_: usize, q: [f32; 2]| {
            if (30.0..45.0).contains(&q[0]) {
                [1.0, 0.0, 0.0]
            } else {
                [0.2; 3]
            }
        };
        let (rim, _) = c.shade([1.0, 50.0], &stripe);
        assert!(
            rim[0] > rim[1] + 0.1,
            "the stripe is drawn out to the rim: {rim:?}"
        );
        let (inner, _) = c.shade([50.0, 50.0], &stripe);
        assert!(close(inner[0], expect[0], 1e-4), "{inner:?}");
    }

    #[test]
    fn a_lens_magnifies_about_the_centre_and_a_ramp_fades_the_blur() {
        let mut c = Container::new(
            vec![GlassShape::capsule([0.0, 0.0, 100.0, 40.0])],
            0.0,
            GlassMode::default(),
        );
        assert_eq!(c.lensed([10.0, 5.0]), [10.0, 5.0]);
        assert_eq!(c.blur_weight([50.0, 40.0]), 1.0);
        c.lens = 2.0;
        let mut c = c.with_blur_ramp([1.0, 0.0]);
        // the sharp copy comes first and the shape's blur after it, both read
        assert_eq!(c.sigmas[0], 0.0);
        assert_eq!(c.values[0].blur_level, 1.0);
        assert!(c.levels()[0].1 && c.levels()[1].1);
        c.lens = 2.0;
        // half way from the centre (50, 20) in what it shows
        assert_eq!(c.lensed([10.0, 0.0]), [30.0, 10.0]);
        assert_eq!(c.lensed([50.0, 20.0]), [50.0, 20.0]);
        assert_eq!(c.blur_weight([50.0, 0.0]), 1.0);
        assert_eq!(c.blur_weight([50.0, 20.0]), 0.5);
        assert_eq!(c.blur_weight([50.0, 40.0]), 0.0);
        let u = GlassUniform::new(
            &c,
            2.0,
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0.0; 4],
            [200.0; 2],
            1.0,
        );
        assert_eq!(u.lens, [1.0, 0.0, 2.0, 0.0]);
        // a stripe behind the lens: the shaded colour at a point is the backdrop's nearer the centre
        let stripe = |_: usize, q: [f32; 2]| {
            if q[0] < 40.0 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 0.0, 1.0]
            }
        };
        let plain = Container::new(
            vec![GlassShape::capsule([0.0, 0.0, 100.0, 40.0])],
            0.0,
            GlassMode::default(),
        );
        let (seen_plain, _) = plain.shade([35.0, 20.0], &stripe);
        let (seen_lens, _) = c.shade([35.0, 20.0], &stripe);
        assert!(seen_plain[0] > seen_plain[2], "{seen_plain:?}");
        assert!(
            seen_lens[2] > seen_lens[0],
            "the lens shows x = 42.5 at x = 35: {seen_lens:?}"
        );
    }

    #[test]
    fn uniform_layout_and_scaling() {
        assert_eq!(
            std::mem::size_of::<GlassUniform>(),
            (22 + 3 * MAX_SHAPES) * 16
        );
        let c = Container::new(
            vec![GlassShape::capsule([10.0, 20.0, 100.0, 40.0])],
            8.0,
            GlassMode::default(),
        );
        let u = GlassUniform::new(
            &c,
            2.0,
            [[0.5, 0.0, 0.0], [0.0, 0.5, 0.0]],
            [0.0; 4],
            [100.0; 2],
            1.0,
        );
        assert_eq!(u.shapes[0], [20.0, 40.0, 200.0, 80.0]);
        assert_eq!(
            (u.shape_a[0][0], u.shape_b[0][3]),
            (40.0, 2.0),
            "a capsule's corners reach half the short side, round"
        );
        assert_eq!(u.info[..2], [1.0, 16.0]);
        // regular, short side 40: refraction −20 at scale 2, band 10 points
        assert_eq!(u.shape_a[0][1], -40.0);
        assert_eq!(u.shape_a[0][2], 20.0);
        assert_eq!(u.highlight_matrix[0][3], 0.1471);
        // one shape, one blur: 4 points for a 40 point control
        assert_eq!(c.levels().map(|l| l.1), [true, false, false, false]);
        assert_eq!(c.levels()[0].0, 4.0);
        assert_eq!(u.shape_b[0][2], 0.0);
        assert_eq!(c.fill_sigma(), Some(16.0));
    }

    #[test]
    fn the_blur_fill_is_made_only_when_it_shows_or_the_bleed_reads_it() {
        let shape = vec![GlassShape::new([0.0, 0.0, 150.0, 100.0], 18.0)];
        let mode = |style| GlassMode {
            dark: true,
            style,
            ..GlassMode::default()
        };
        let regular = Container::new(shape.clone(), 0.0, mode(GlassStyle::Regular));
        assert_eq!(regular.fill_sigma(), Some(16.0));
        // clear glass has no fill and no bleed: one blurred copy and nothing else
        let clear = Container::new(shape, 0.0, mode(GlassStyle::Clear));
        assert_eq!(clear.fill_sigma(), None);
        assert_eq!(clear.levels().map(|l| l.1), [true, false, false, false]);
    }

    #[test]
    fn hostile_spacing_is_bounded_and_the_sampled_area_covers_the_bulge() {
        let shapes = vec![
            GlassShape::new([0.0, 0.0, 100.0, 40.0], 12.0),
            GlassShape::new([104.0, 0.0, 100.0, 40.0], 12.0),
        ];
        let mode = GlassMode::default();
        for bad in [f32::NAN, f32::NEG_INFINITY, -3.0] {
            assert_eq!(Container::new(shapes.clone(), bad, mode).spacing, 0.0);
        }
        let huge = Container::new(shapes.clone(), 8.0e6, mode);
        assert_eq!(huge.spacing, MAX_SPACING);
        let r = huge.params.reach();
        let [x, _, w, _] = huge.sample_area().unwrap();
        assert_eq!(
            (x, w),
            (
                -r - MAX_SPACING / 4.0,
                204.0 + 2.0 * (r + MAX_SPACING / 4.0)
            )
        );
        // the field never reaches past the shapes by more than the bulge the area allows for
        let above_gap = [102.0, -MAX_SPACING / 4.0 - 0.01];
        assert!(huge.field(above_gap).0 > 0.0);
        let alone = Container::new(shapes[..1].to_vec(), 20.0, mode);
        assert_eq!(alone.sample_area().unwrap()[0], -alone.params.reach());
    }

    /// The WCAG contrast ratio of two opaque sRGB colours.
    fn contrast(a: Rgb, b: Rgb) -> f32 {
        let y = |c: Rgb| {
            0.2126 * srgb_decode(c[0]) + 0.7152 * srgb_decode(c[1]) + 0.0722 * srgb_decode(c[2])
        };
        let (hi, lo) = (y(a).max(y(b)), y(a).min(y(b)));
        (hi + 0.05) / (lo + 0.05)
    }

    /// A light-appearance panel of regular glass (Control Center's modules and menus) keeps body text, black at 85 %,
    /// at 4.5:1 or more over the darkest backdrops, at every panel size and with each accessibility recipe.
    #[test]
    fn light_glass_keeps_body_text_readable_over_a_dark_backdrop() {
        const BODY: f32 = 0.85;
        for (rt, ic) in [(false, false), (true, false), (false, true), (true, true)] {
            let mode = GlassMode {
                dark: false,
                reduce_transparency: rt,
                increase_contrast: ic,
                ..GlassMode::default()
            };
            for side in [44.0, 64.0, 120.0, 320.0] {
                let c = Container::new(
                    vec![GlassShape::new([0.0, 0.0, 2.0 * side, side], 16.0)],
                    0.0,
                    mode,
                );
                for backdrop in [[0.0; 3], [0.1, 0.1, 0.12], [0.2, 0.05, 0.3]] {
                    let (face, cov) = c.shade([side, side / 2.0], &|_, _| backdrop);
                    assert_eq!(cov, 1.0);
                    let text = face.map(|v| v * (1.0 - BODY));
                    let ratio = contrast(text, face);
                    assert!(
                        ratio >= 4.5,
                        "rt {rt} ic {ic}, {side} pt over {backdrop:?}: body text {ratio:.2}:1 on {face:?}"
                    );
                }
            }
        }
    }

    /// The face of `style` glass `side` points high over a flat backdrop `rgb` (0–255), in 0–255.
    fn flat_face(dark: bool, style: GlassStyle, side: f32, rgb: [f32; 3]) -> [f32; 3] {
        let p = GlassParams::new(
            GlassMode {
                dark,
                style,
                ..GlassMode::default()
            },
            side,
        );
        let c = rgb.map(|v| v / 255.0);
        // over a flat backdrop every blur and the blur fill are the backdrop itself
        face_color(blur_fill(c, c, &p), &p, p.face.max_luma).map(|v| v * 255.0)
    }

    /// Flat backdrops and the face dark menu glass gives each, in 8-bit levels: a dark backdrop lifted, a mid one kept,
    /// a bright one held down near 112 to 125 with its colour.
    const MENU_TONES_DARK: [([f32; 3], [f32; 3]); 9] = [
        ([46.0, 37.0, 32.0], [65.0, 55.0, 50.0]),
        ([67.0, 51.0, 41.0], [84.0, 66.0, 53.0]),
        ([84.0, 61.0, 47.0], [98.0, 73.0, 57.0]),
        ([112.0, 86.0, 73.0], [113.0, 87.0, 74.0]),
        ([140.0, 110.0, 88.0], [126.0, 97.0, 77.0]),
        ([157.0, 144.0, 150.0], [119.0, 109.0, 115.0]),
        ([180.0, 165.0, 163.0], [123.0, 112.0, 111.0]),
        ([196.0, 181.0, 174.0], [124.0, 113.0, 108.0]),
        ([230.0, 215.0, 169.0], [122.0, 113.0, 86.0]),
    ];
    /// The same for light menu glass: a pale wash, about `0.41 · c + 144` per channel.
    const MENU_TONES_LIGHT: [([f32; 3], [f32; 3]); 7] = [
        ([45.0, 35.0, 30.0], [154.0, 139.0, 130.0]),
        ([113.0, 87.0, 74.0], [203.0, 175.0, 161.0]),
        ([134.0, 105.0, 88.0], [215.0, 184.0, 166.0]),
        ([155.0, 143.0, 152.0], [219.0, 207.0, 216.0]),
        ([180.0, 165.0, 164.0], [232.0, 216.0, 215.0]),
        ([196.0, 181.0, 174.0], [240.0, 223.0, 216.0]),
        ([255.0, 255.0, 255.0], [248.0, 248.0, 248.0]),
    ];

    #[test]
    fn menu_glass_tones_a_flat_backdrop_into_a_narrow_band_keeping_its_colour() {
        for (dark, tones, within) in [
            (true, &MENU_TONES_DARK[..], 4.0),
            (false, &MENU_TONES_LIGHT[..], 4.0),
        ] {
            for (backdrop, body) in tones {
                // the size does not change the menu's face
                for side in [40.0, 300.0] {
                    let face = flat_face(dark, GlassStyle::Menu, side, *backdrop);
                    for c in 0..3 {
                        assert!(
                            (face[c] - body[c]).abs() <= within,
                            "dark {dark}: {backdrop:?} gave {face:?}, not {body:?}"
                        );
                    }
                }
            }
        }
        // a bright sky behind a dark menu comes through as a mid-dark grey of it, not a near-opaque dark one
        let sky = flat_face(true, GlassStyle::Menu, 200.0, [212.0, 214.0, 209.0]);
        assert!(sky.iter().all(|v| (105.0..=120.0).contains(v)), "{sky:?}");
        // and black stays well below every bright backdrop
        let black = flat_face(true, GlassStyle::Menu, 200.0, [0.0; 3]);
        assert!(black.iter().all(|v| (15.0..=30.0).contains(v)), "{black:?}");
    }

    /// Flat backdrops and the face module glass gives each, in 8-bit levels, the same in both appearances: about
    /// `0.59 · c + 27` per channel, so a sunlit backdrop comes through as a mid tone of its own colour.
    const MODULE_TONES: [([f32; 3], [f32; 3]); 6] = [
        ([229.0, 216.0, 182.0], [170.0, 155.0, 114.0]),
        ([239.0, 229.0, 201.0], [173.0, 163.0, 134.0]),
        ([236.0, 225.0, 195.0], [172.0, 159.0, 126.0]),
        ([204.0, 172.0, 105.0], [162.0, 127.0, 59.0]),
        ([168.0, 119.0, 64.0], [146.0, 96.0, 39.0]),
        ([118.0, 75.0, 44.0], [111.0, 68.0, 36.0]),
    ];

    #[test]
    fn module_tiles_tone_the_backdrop_into_a_mid_tone_of_its_colour_in_both_appearances() {
        for (backdrop, tile) in MODULE_TONES {
            for side in [80.0, 170.0] {
                let light = flat_face(false, GlassStyle::Module, side, backdrop);
                let dark = flat_face(true, GlassStyle::Module, side, backdrop);
                assert_eq!(light, dark, "{backdrop:?}: the appearance changes nothing");
                for c in 0..3 {
                    assert!(
                        (dark[c] - tile[c]).abs() <= 6.0,
                        "{backdrop:?} gave {dark:?}, not {tile:?}"
                    );
                }
            }
        }
        // white comes out a mid grey and black a dark one
        let white = flat_face(true, GlassStyle::Module, 170.0, [255.0; 3]);
        assert!(white.iter().all(|v| (v - 178.0).abs() <= 1.0), "{white:?}");
        let black = flat_face(true, GlassStyle::Module, 170.0, [0.0; 3]);
        assert!(black.iter().all(|v| (v - 27.0).abs() <= 1.0), "{black:?}");
        // a strong colour keeps its hue
        let red = flat_face(true, GlassStyle::Module, 80.0, [255.0, 51.0, 51.0]);
        assert!(
            red[0] > 2.5 * red[1] && (red[1] - red[2]).abs() < 1.0,
            "{red:?}"
        );
    }

    /// Flat backdrops and the face dark card glass gives each, in 8-bit levels: about `0.42 · c + 26` per channel over
    /// the bright half, a bright neutral held near 115, below a bright warm colour.
    const CARD_TONES_DARK: [([f32; 3], [f32; 3]); 4] = [
        ([231.0, 214.0, 164.0], [123.0, 114.0, 85.0]),
        ([249.0, 249.0, 249.0], [115.0, 115.0, 113.0]),
        ([229.0, 208.0, 134.0], [126.0, 114.0, 70.0]),
        ([83.0, 52.0, 32.0], [61.0, 39.0, 27.0]),
    ];

    #[test]
    fn a_dark_card_is_a_mid_dark_pane_that_keeps_a_little_of_the_backdrops_colour() {
        for (backdrop, card) in CARD_TONES_DARK {
            for side in [56.0, 120.0] {
                let face = flat_face(true, GlassStyle::Card, side, backdrop);
                for c in 0..3 {
                    assert!(
                        (face[c] - card[c]).abs() <= 4.0,
                        "{backdrop:?} gave {face:?}, not {card:?}"
                    );
                }
            }
        }
        // light text reads on it over any backdrop: even white behind stays a mid-dark grey
        for grey in [0.0, 128.0, 192.0, 255.0] {
            let face = flat_face(true, GlassStyle::Card, 56.0, [grey; 3]);
            assert!(face[1] < 120.0, "{grey}: {face:?}");
        }
        // a strong red stays red, darker
        let red = flat_face(true, GlassStyle::Card, 56.0, [255.0, 51.0, 51.0]);
        assert!(red[0] > 2.0 * red[1] && red[0] < 190.0, "{red:?}");
        // the dense glass of menus dims the same red and deepens it, its weaker channels pressed towards black
        let dense = flat_face(true, GlassStyle::Dense, 56.0, [255.0, 51.0, 51.0]);
        assert!(dense[0] < 240.0 && dense[1] < 10.0, "{dense:?}");
    }

    #[test]
    fn the_sidebar_is_a_dense_pane_that_takes_only_a_faint_tint_of_the_backdrop() {
        // (appearance, green of the face over black, over mid grey, over white): about 0.034 · x + 34 dark, 0.02 · x + 229
        // light, which is a little warm
        for (dark, at) in [(true, [34.0, 38.0, 42.5]), (false, [229.1, 231.6, 234.2])] {
            for (grey, want) in [0.0, 128.0, 255.0].into_iter().zip(at) {
                let face = flat_face(dark, GlassStyle::Sidebar, 300.0, [grey; 3]);
                assert!((face[1] - want).abs() <= 1.0, "{dark} {grey}: {face:?}");
                assert!(face[0] >= face[1] && face[1] >= face[2], "{face:?}");
                assert!(face[0] - face[2] < 4.0, "{face:?}");
            }
            // a saturated backdrop moves the pane only a few levels from the grey of the same backdrop level
            let grey = flat_face(dark, GlassStyle::Sidebar, 300.0, [128.0; 3]);
            let blue = flat_face(dark, GlassStyle::Sidebar, 300.0, [40.0, 90.0, 250.0]);
            for c in 0..3 {
                assert!((blue[c] - grey[c]).abs() < 18.0, "{dark}: {blue:?}");
            }
            // Increase Contrast keeps the dense face
            let with = |increase_contrast| {
                GlassParams::new(
                    GlassMode {
                        dark,
                        style: GlassStyle::Sidebar,
                        increase_contrast,
                        ..GlassMode::default()
                    },
                    300.0,
                )
                .face
            };
            assert_eq!(with(true), with(false));
        }
    }

    /// Flat backdrops and the face the light large panel gives each, in 8-bit levels: a cream sheet that keeps the
    /// backdrop's warmth without turning it orange.
    const LARGE_PANEL_TONES_LIGHT: [([f32; 3], [f32; 3]); 9] = [
        ([55.0, 38.0, 29.0], [166.0, 146.0, 135.0]),
        ([65.0, 43.0, 35.0], [176.0, 147.0, 138.0]),
        ([89.0, 62.0, 43.0], [192.0, 160.0, 138.0]),
        ([129.0, 87.0, 55.0], [221.0, 175.0, 139.0]),
        ([141.0, 96.0, 57.0], [229.0, 180.0, 138.0]),
        ([170.0, 126.0, 74.0], [244.0, 196.0, 141.0]),
        ([195.0, 155.0, 90.0], [253.0, 213.0, 147.0]),
        ([209.0, 181.0, 114.0], [254.0, 227.0, 159.0]),
        ([195.0, 182.0, 139.0], [253.0, 239.0, 184.0]),
    ];

    #[test]
    fn the_light_large_panel_keeps_a_warm_backdrops_tint_as_cream() {
        let mut square = 0.0;
        for (backdrop, panel) in LARGE_PANEL_TONES_LIGHT {
            let face = flat_face(false, GlassStyle::LargePanel, 400.0, backdrop);
            for c in 0..3 {
                assert!(
                    (face[c] - panel[c]).abs() <= 8.0,
                    "{backdrop:?} gave {face:?}, not {panel:?}"
                );
                square += (face[c] - panel[c]).powi(2);
            }
        }
        let rms = (square / 27.0).sqrt();
        assert!(rms < 4.0, "{rms}");
    }

    #[test]
    fn the_label_faces_are_the_regular_faces_over_a_mid_grey() {
        use cupertino_theme::glass::{LABEL_FACE_DARK, LABEL_FACE_LIGHT};
        let light = flat_face(false, GlassStyle::Regular, 100.0, [127.5; 3])[1] / 255.0;
        let dark = flat_face(true, GlassStyle::Regular, 120.0, [127.5; 3])[1] / 255.0;
        assert!(close(light, LABEL_FACE_LIGHT, 0.01), "{light}");
        assert!(close(dark, LABEL_FACE_DARK, 0.01), "{dark}");
    }

    #[test]
    fn the_bar_lifts_black_dims_white_and_saturates() {
        let light_black = flat_face(false, GlassStyle::Bar, 70.0, [0.0; 3]);
        let light_white = flat_face(false, GlassStyle::Bar, 70.0, [255.0; 3]);
        assert!(
            close(light_black[0], 0.15 * 255.0, 0.5) && close(light_white[0], 0.9 * 255.0, 0.5)
        );
        // dark: the luma compressed to 0.75 and a faint black veil
        let dark_white = flat_face(true, GlassStyle::Bar, 70.0, [255.0; 3]);
        let expect = (0.75 + 0.05 * 0.25 - 0.08 * 0.75) * 0.98 * 255.0;
        assert!(close(dark_white[0], expect, 1.0), "{dark_white:?} {expect}");
        let muted = [120.0, 140.0, 200.0];
        let out = flat_face(false, GlassStyle::Bar, 70.0, muted);
        assert!(out[2] - out[0] > muted[2] - muted[0], "{out:?}");
    }

    /// A toolbar control's glass over the window's flat background: its face stands further from the background than
    /// regular glass does (31 comes out at 58 rather than 51 in the dark appearance, white at 245 rather than 240 in the
    /// light), a thin line of light runs along its top, fainter along its bottom and not at all across the middle of its
    /// sides, and no rim darkens its edge.
    #[test]
    fn a_toolbar_control_lifts_its_face_and_lights_its_top_edge() {
        for (dark, bg, regular_face, toolbar_face) in
            [(true, 31.0, 51.0, 58.0), (false, 255.0, 240.0, 245.0)]
        {
            let shade = |style| {
                let c = Container::new(
                    vec![GlassShape::new([0.0, 0.0, 110.0, 36.0], 18.0)],
                    0.0,
                    GlassMode {
                        dark,
                        style,
                        ..GlassMode::default()
                    },
                );
                let flat = move |_: usize, _: [f32; 2]| [bg / 255.0; 3];
                move |x: f32, y: f32| {
                    let (col, a) = c.shade([x, y], &flat);
                    col[0] * 255.0 * a + bg * (1.0 - a)
                }
            };
            let regular = shade(GlassStyle::Regular);
            let toolbar = shade(GlassStyle::Toolbar);
            let face = toolbar(55.0, 18.0);
            assert!(
                (regular(55.0, 18.0) - regular_face).abs() <= 1.0,
                "{}",
                regular(55.0, 18.0)
            );
            assert!((face - toolbar_face).abs() <= 1.0, "{face}");
            // the line of light a quarter point inside the top and the bottom, the top the brighter
            let (top, bottom) = (toolbar(55.0, 0.25), toolbar(55.0, 35.75));
            assert!(top >= bottom && bottom > face, "{top} {bottom} {face}");
            if dark {
                assert!(top > face + 40.0 && bottom > face + 15.0, "{top} {bottom}");
            }
            // across the middle of a side it barely shows, and regular glass draws no such line at all
            assert!(toolbar(0.75, 18.0) - face < 4.0, "{}", toolbar(0.75, 18.0));
            assert!(regular(55.0, 0.25) <= regular(55.0, 18.0) + 1.0);
            // no rim darkens the toolbar's edge: nothing inside it (past the antialiased outermost half point) is
            // darker than its face
            for i in 0..12 {
                let d = 0.75 + i as f32 * 0.5;
                for v in [toolbar(55.0, d), toolbar(d, 18.0)] {
                    assert!(v >= face - 1.0, "{d}: {v}");
                }
            }
        }
    }
}
