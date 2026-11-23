//! The stages of the glass composite a recipe turns on with a value of its own, each off at its default: the edge bleed
//! of the backdrop outside the shape, the tint over the face, the control outline, the key and fill light, and the bend
//! of the backdrop outside the edge. Each shape of a container carries its own look ([`ShapeLook`]), mixed across the
//! union with the field's weights.

use super::{angle_between, luma, mix, refraction_profile, Rgb};
use cupertino_theme::glass::{GlassParams, BORDER_LIT_SHARE, RIM_DEPTH_PER_HEIGHT};

/// The smoothstep of `x` from `e0` to `e1`: 0 below, 1 above, `t²(3 − 2t)` between.
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The edge bleed's colour of `c`, what its blur reads: saturation about the luma by `saturation`, then the levels
/// `black · (1 − L) − (1 − white) · L` added to every channel (`L` the luma after the saturation), within `[0, 1]`.
pub fn bleed_tone(c: Rgb, p: &GlassParams) -> Rgb {
    let b = &p.bleed;
    let y = luma(c);
    let s = c.map(|v| y + (v - y) * b.saturation);
    let l = luma(s);
    let shift = b.black * (1.0 - l) - (1.0 - b.white) * l;
    s.map(|v| (v + shift).clamp(0.0, 1.0))
}

/// Stage 3: where the edge bleed reads the backdrop for a pixel at `p`, `d` from the edge (negative inside) with
/// outward `normal`, and how much of it the face takes there: `reach · (1 − t)` points along the normal and
/// `opacity · (1 − t)²`, `t` the depth into the band of `height` (0 at the edge). None past the band or at no opacity.
pub fn bleed_read(
    p: [f32; 2],
    d: f32,
    normal: [f32; 2],
    reach: f32,
    height: f32,
    opacity: f32,
) -> Option<([f32; 2], f32)> {
    if opacity <= 0.0 || height <= 0.0 || -d >= height {
        return None;
    }
    let t = (-d).max(0.0) / height;
    let r = reach * (1.0 - t);
    let w = opacity.min(1.0) * (1.0 - t) * (1.0 - t);
    Some(([p[0] + normal[0] * r, p[1] + normal[1] * r], w))
}

/// Stage 3: the bleed `tone` (from [`bleed_tone`]) blended into the face `c` at `weight`: the smaller of each channel
/// when the recipe darkens, the tone itself otherwise.
pub fn bleed_into(c: Rgb, tone: Rgb, weight: f32, p: &GlassParams) -> Rgb {
    let blended: Rgb = if p.bleed.darken {
        std::array::from_fn(|i| c[i].min(tone[i]))
    } else {
        tone
    };
    mix(c, blended, weight)
}

/// Stage 3, last: the face's fill, then the tint, over the toned face `c` in normal blending.
pub fn fill_and_tint(c: Rgb, p: &GlassParams) -> Rgb {
    let f = p.face.fill;
    let c = mix(c, [f[0], f[1], f[2]], f[3]);
    let t = p.tint;
    if t[3] > 0.0 {
        mix(c, [t[0], t[1], t[2]], t[3])
    } else {
        c
    }
}

/// An arc of the key or fill light on an edge whose outward normal points at `normal_angle` (radians, y up), from a
/// light at `light`: all of it facing the light, none `spread` away, `1 − smoothstep(0, spread, a)` between.
pub fn light_arc(normal_angle: f32, light: f32, spread: f32) -> f32 {
    if spread <= 0.0 {
        return 0.0;
    }
    1.0 - smoothstep(0.0, spread, angle_between(normal_angle, light))
}

/// How much of the key and the fill light (at the recipe's share of the key) falls on an edge whose outward normal
/// points at `normal_angle`, over arcs of half-width `spread`.
pub fn key_and_fill(normal_angle: f32, spread: f32, p: &GlassParams) -> f32 {
    let s = &p.specular;
    light_arc(normal_angle, s.angle, spread)
        + s.fill * light_arc(normal_angle, s.angle + std::f32::consts::PI, spread)
}

/// The angle of `normal` (y down) counter-clockwise from the right with y up.
fn up_angle(normal: [f32; 2]) -> f32 {
    (-normal[1]).atan2(normal[0])
}

/// Stage 5: the white light the key and fill add to every channel `depth` points inside the edge (0 on it) where its
/// outward normal is `normal`: `amount · (key · band + glow)`, the band `1 − smoothstep(0, height, depth)` and the
/// diffuse glow `amount_scale · key′ · (1 − depth / (height · height_scale))²` over arcs `spread_scale` as wide.
pub fn specular_light(depth: f32, normal: [f32; 2], p: &GlassParams) -> f32 {
    let s = &p.specular;
    if s.amount <= 0.0 || (normal[0] == 0.0 && normal[1] == 0.0) {
        return 0.0;
    }
    let angle = up_angle(normal);
    let band = if s.height > 0.0 {
        1.0 - smoothstep(0.0, s.height, depth)
    } else {
        0.0
    };
    let key = key_and_fill(angle, s.spread, p);
    let reach = s.height * s.diffuse.height_scale;
    let glow = if reach > 0.0 && depth < reach {
        let left = 1.0 - depth / reach;
        s.diffuse.amount_scale
            * key_and_fill(angle, s.spread * s.diffuse.spread_scale, p)
            * left
            * left
    } else {
        0.0
    };
    s.amount * (key * band + glow)
}

/// Stage 5: the opacity of the control outline at a pixel `d` from the edge (negative inside) whose outward normal is
/// `normal`: a band of its width centred on the edge, `opacity · clamp(width / 2 + ½ − |d|)`, hidden down to
/// [`BORDER_LIT_SHARE`] where the key or fill light falls.
pub fn border_line(d: f32, normal: [f32; 2], p: &GlassParams) -> f32 {
    let b = &p.border;
    if b.opacity <= 0.0 || b.width <= 0.0 {
        return 0.0;
    }
    let line = b.opacity * (0.5 * b.width + 0.5 - d.abs()).clamp(0.0, 1.0);
    if line <= 0.0 || p.specular.amount <= 0.0 || (normal[0] == 0.0 && normal[1] == 0.0) {
        return line;
    }
    let lit = key_and_fill(up_angle(normal), p.specular.spread, p);
    line * (1.0 - (1.0 - BORDER_LIT_SHARE) * lit.min(1.0))
}

/// Stage 5 on the face `c`: the control outline darkens it, then the key and fill light add to it as white.
pub fn border_and_light(c: Rgb, d: f32, depth: f32, normal: [f32; 2], p: &GlassParams) -> Rgb {
    let line = border_line(d, normal, p);
    let c = if line > 0.0 {
        c.map(|v| v * (1.0 - line))
    } else {
        c
    };
    let light = specular_light(depth, normal, p);
    if light > 0.0 {
        c.map(|v| (v + light).clamp(0.0, 1.0))
    } else {
        c
    }
}

/// Stage 8: where the bend outside the edge reads the backdrop for a pixel at `p`, `d` from the edge with outward
/// `normal`, and how much of it is laid there: `amount · g(t)` points along the normal ([`refraction_profile`]) at
/// `opacity · (1 − smoothstep(0, 1, t))`, `t = max(d, 0) / height`. None past the band, without a strength or at no
/// opacity.
pub fn outer_read(
    p: [f32; 2],
    d: f32,
    normal: [f32; 2],
    amount: f32,
    height: f32,
    opacity: f32,
) -> Option<([f32; 2], f32)> {
    if opacity <= 0.0 || height <= 0.0 || amount == 0.0 {
        return None;
    }
    let t = d.max(0.0) / height;
    if t >= 1.0 {
        return None;
    }
    let r = amount * refraction_profile(t);
    let alpha = opacity.min(1.0) * (1.0 - smoothstep(0.0, 1.0, t));
    Some(([p[0] + normal[0] * r, p[1] + normal[1] * r], alpha))
}

/// The look of one shape that its own recipe sets inside a container: the face, its fill and tint, the rim, the key and
/// fill light, the ring shadow, the control outline and the opacities of the edge bleed and the bend outside the edge.
/// Mixed across the union with the field's weights, so where shapes join the looks blend, and a hard union keeps each
/// shape's own.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShapeLook {
    /// Saturation, white, black and clamp of the face.
    pub face: [f32; 4],
    pub fill: [f32; 4],
    pub tint: [f32; 4],
    /// The rim's strength, colour bias and height.
    pub rim: [f32; 3],
    /// The key and fill light: amount, half-width, band height, key angle, fill share, and the diffuse glow's height,
    /// spread and amount scales.
    pub specular: [f32; 8],
    /// The ring shadow's offset, stroke, deviation and opacity.
    pub ring: [f32; 4],
    /// The control outline's opacity and width.
    pub border: [f32; 2],
    pub bleed_opacity: f32,
    pub outer_opacity: f32,
}

impl ShapeLook {
    /// The look recipe `p` gives a shape.
    pub fn of(p: &GlassParams) -> Self {
        let (f, h, s, sh) = (&p.face, &p.highlight, &p.specular, &p.shadow);
        Self {
            face: [f.saturation, f.white, f.black, f.clamp],
            fill: f.fill,
            tint: p.tint,
            rim: [h.amount, h.color_bias, h.height],
            specular: [
                s.amount,
                s.spread,
                s.height,
                s.angle,
                s.fill,
                s.diffuse.height_scale,
                s.diffuse.spread_scale,
                s.diffuse.amount_scale,
            ],
            ring: [
                sh.ring_offset,
                sh.ring_stroke,
                sh.ring_blur,
                sh.ring_opacity,
            ],
            border: [p.border.opacity, p.border.width],
            bleed_opacity: p.bleed.opacity,
            outer_opacity: p.refraction.outer_opacity,
        }
    }

    /// `a` moved towards `b` by `t`: `a` itself where they agree.
    pub fn mix(a: Self, b: Self, t: f32) -> Self {
        fn m<const N: usize>(x: [f32; N], y: [f32; N], t: f32) -> [f32; N] {
            std::array::from_fn(|i| x[i] + (y[i] - x[i]) * t)
        }
        let s = |x: f32, y: f32| x + (y - x) * t;
        Self {
            face: m(a.face, b.face, t),
            fill: m(a.fill, b.fill, t),
            tint: m(a.tint, b.tint, t),
            rim: m(a.rim, b.rim, t),
            specular: m(a.specular, b.specular, t),
            ring: m(a.ring, b.ring, t),
            border: m(a.border, b.border, t),
            bleed_opacity: s(a.bleed_opacity, b.bleed_opacity),
            outer_opacity: s(a.outer_opacity, b.outer_opacity),
        }
    }

    /// `p` with this look in place of its own.
    pub fn apply(&self, p: &mut GlassParams) {
        let [saturation, white, black, clamp] = self.face;
        (p.face.saturation, p.face.white, p.face.black, p.face.clamp) =
            (saturation, white, black, clamp);
        p.face.fill = self.fill;
        p.tint = self.tint;
        [
            p.highlight.amount,
            p.highlight.color_bias,
            p.highlight.height,
        ] = self.rim;
        let s = &mut p.specular;
        [
            s.amount,
            s.spread,
            s.height,
            s.angle,
            s.fill,
            s.diffuse.height_scale,
            s.diffuse.spread_scale,
            s.diffuse.amount_scale,
        ] = self.specular;
        let sh = &mut p.shadow;
        [
            sh.ring_offset,
            sh.ring_stroke,
            sh.ring_blur,
            sh.ring_opacity,
        ] = self.ring;
        [p.border.opacity, p.border.width] = self.border;
        p.bleed.opacity = self.bleed_opacity;
        p.refraction.outer_opacity = self.outer_opacity;
    }

    /// The rim's depth in points.
    pub fn rim_depth(&self) -> f32 {
        RIM_DEPTH_PER_HEIGHT * self.rim[2]
    }
}
