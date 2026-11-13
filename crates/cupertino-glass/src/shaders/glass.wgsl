// Glass composite: one draw per container over the union of its shapes. The stages and every value are described in
// src/glass.rs, whose CPU model this follows. Lengths are target pixels, y down; colours are sRGB-encoded throughout.

struct Glass {
  quad: vec4<f32>,        // where to draw: x, y, w, h
  uv_x: vec4<f32>,        // u = a·x + b·y + c
  uv_y: vec4<f32>,        // v = d·x + e·y + f
  info: vec4<f32>,        // shapes, blend width, antialiasing width, opacity
  levels: vec4<f32>,      // the edge line's strength, unused (0), target w, h
  fill: vec4<f32>,        // blur fill: darken, lighten, normal
  face: vec4<f32>,        // saturation, white, black, clamp
  face_fill: vec4<f32>,
  bleed: vec4<f32>,       // white, black, saturation, darken
  highlight: vec4<f32>,   // rim strength, light angle, spread, rim depth
  highlight2: vec4<f32>,  // offset, colour bias, curvature, the soft band's far end
  glow: vec4<f32>,        // soft band: amount, half-width, offset
  diffuse: vec4<f32>,     // diffuse band: half-width, arcs' width, strength, per unit of the soft band's; the edge
                          // line's side arcs' width
  hm: array<vec4<f32>, 3>,
  shadow: vec4<f32>,      // offset x, offset y, alpha, blur
  shadow_color: vec4<f32>,// rgb, ring darkening
  ring: vec4<f32>,        // ring inside the shape: moved up by, stroke, deviation; outline width
  shapes: array<vec4<f32>, 16>,
  shape_a: array<vec4<f32>, 16>, // corner extent, refraction strength, refraction band, bleed fraction
  shape_b: array<vec4<f32>, 16>, // bleed band, largest luminance, where the shape's blur lies among the blurred copies, corner exponent
  cut: vec4<f32>,         // the outline the container is cut to: x, y, w, h
  cut_shape: vec4<f32>,   // its corner extent, corner exponent, 1 when cut; 1 when the edge line is even
};

@group(0) @binding(0) var<uniform> G: Glass;
@group(0) @binding(1) var level0: texture_2d<f32>;
@group(0) @binding(2) var level1: texture_2d<f32>;
@group(0) @binding(3) var level2: texture_2d<f32>;
@group(0) @binding(4) var level3: texture_2d<f32>;
@group(0) @binding(5) var blur_fill: texture_2d<f32>;
@group(0) @binding(6) var samp: sampler;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) px: vec2<f32>,
};

@vertex
fn vs_glass(@builtin(vertex_index) vi: u32) -> VsOut {
  let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
  let p = G.quad.xy + corner * G.quad.zw;
  var o: VsOut;
  o.pos = vec4<f32>(p.x / G.levels.z * 2.0 - 1.0, 1.0 - p.y / G.levels.w * 2.0, 0.0, 1.0);
  o.px = p;
  return o;
}

const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);
const PI = 3.14159265;

// Distance to a rectangle whose corners reach e along each side with a superellipse of exponent n (2: a circle);
// to first order in a corner of n over 2: the norm's excess over its slope.
fn rect_sdf(p: vec2<f32>, r: vec4<f32>, e: f32, n: f32) -> f32 {
  let hs = r.zw * 0.5;
  let ext = clamp(e, 0.0, min(hs.x, hs.y));
  let q = abs(p - (r.xy + hs)) - (hs - vec2<f32>(ext));
  if (q.x <= 0.0 || q.y <= 0.0 || ext <= 0.0) {
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - ext;
  }
  if (n <= 2.0) { return length(q) - ext; }
  let u = max(q / ext, vec2<f32>(1e-6));
  let un = pow(u, vec2<f32>(n));
  let g = pow(un.x + un.y, 1.0 / n);
  let slope = un / u / pow(g, n - 1.0);
  // never nearer than the corner's square, which holds the curve
  return max((g - 1.0) * ext / max(length(slope), 1e-6), max(q.x, q.y) - ext);
}

// the refraction's offset t of its band deep as a share of its strength: 0.9·e^(−4.2t)·(1 − t⁴), none past the band
fn refraction_profile(t: f32) -> f32 {
  if (t >= 1.0) { return 0.0; }
  let t2 = t * t;
  return 0.9 * exp(-4.2 * t) * (1.0 - t2 * t2);
}

struct Field { d: f32, a: vec4<f32>, b: vec4<f32> };

// Smooth union of every shape; the per-shape values are mixed with the same weights.
fn field(p: vec2<f32>) -> Field {
  let n = i32(G.info.x);
  let k = G.info.y;
  var f: Field;
  f.d = rect_sdf(p, G.shapes[0], G.shape_a[0].x, G.shape_b[0].w);
  f.a = G.shape_a[0];
  f.b = G.shape_b[0];
  for (var i = 1; i < n; i++) {
    let di = rect_sdf(p, G.shapes[i], G.shape_a[i].x, G.shape_b[i].w);
    var h: f32;
    var d: f32;
    if (k <= 0.0) {
      h = select(0.0, 1.0, di <= f.d);
      d = min(di, f.d);
    } else {
      h = clamp(0.5 + 0.5 * (f.d - di) / k, 0.0, 1.0);
      d = mix(f.d, di, h) - k * h * (1.0 - h);
    }
    f.a = mix(f.a, G.shape_a[i], h);
    f.b = mix(f.b, G.shape_b[i], h);
    f.d = d;
  }
  return f;
}

fn field_d(p: vec2<f32>) -> f32 {
  return field(p).d;
}

fn uv_of(p: vec2<f32>) -> vec2<f32> {
  let h = vec3<f32>(p, 1.0);
  return vec2<f32>(dot(G.uv_x.xyz, h), dot(G.uv_y.xyz, h));
}

// saturation about the Rec. 709 luma of encoded values
fn saturate_encoded(c: vec3<f32>, s: f32) -> vec3<f32> {
  let y = dot(c, LUMA);
  return max(vec3<f32>(y) + (c - vec3<f32>(y)) * s, vec3<f32>(0.0));
}

fn tap(t: texture_2d<f32>, uv: vec2<f32>) -> vec3<f32> {
  return textureSampleLevel(t, samp, uv, 0.0).rgb;
}

// Stage 1: the blurred copy of the shape's blur at `pos` among them, or the two around it mixed.
fn blurred(uv: vec2<f32>, pos: f32) -> vec3<f32> {
  let w = max(vec4<f32>(1.0) - abs(vec4<f32>(clamp(pos, 0.0, 3.0)) - vec4<f32>(0.0, 1.0, 2.0, 3.0)), vec4<f32>(0.0));
  return w.x * tap(level0, uv) + w.y * tap(level1, uv) + w.z * tap(level2, uv) + w.w * tap(level3, uv);
}

// Stages 1 and 2: the shape's blur, the blur fill and the face colour at `q`.
fn face_at(q: vec2<f32>, pos: f32, max_luma: f32) -> vec3<f32> {
  let uv = uv_of(q);
  var c = blurred(uv, pos);
  let f = tap(blur_fill, uv);
  c = mix(c, min(c, f), G.fill.x);
  c = mix(c, max(c, f), G.fill.y);
  c = mix(c, f, G.fill.z);
  // stage 2, on encoded values: saturation, luma compression, levels, limit, fill
  var s = saturate_encoded(c, G.face.x);
  var l = dot(s, LUMA);
  s = s - (1.0 - max_luma) * l * (0.75 * s + vec3<f32>(0.25 * l));
  l = dot(s, LUMA);
  s = s + vec3<f32>(G.face.z * (1.0 - l) - (1.0 - G.face.y) * l);
  let e = clamp(s, vec3<f32>(0.0), vec3<f32>(min(G.face.w, 1.0)));
  return mix(e, G.face_fill.rgb, G.face_fill.a);
}

fn angle_between(a: f32, b: f32) -> f32 {
  let t = 2.0 * PI;
  let d = a - b - floor((a - b) / t) * t;
  return min(d, t - d);
}

// a raised cosine of the angle from the light, nothing past LOBE_REACH of the spread
const LOBE_REACH: f32 = 0.75;
// the shortest field slope the rim's distance to the edge divides by
const MIN_FIELD_SLOPE: f32 = 0.05;

fn lobe(normal_angle: f32, light: f32, spread_scale: f32) -> f32 {
  let reach = G.highlight.z * spread_scale * LOBE_REACH;
  if (reach <= 0.0) { return 0.0; }
  let a = angle_between(normal_angle, light);
  if (a >= reach) { return 0.0; }
  return pow(0.5 + 0.5 * cos(a / reach * PI), max(G.highlight2.z, 0.001));
}

fn key_color(c: vec3<f32>) -> vec3<f32> {
  let h = vec4<f32>(c, 1.0);
  return clamp(vec3<f32>(dot(G.hm[0], h), dot(G.hm[1], h), dot(G.hm[2], h)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn coverage(d: f32, r: f32) -> f32 {
  if (r <= 0.0) { return select(0.0, 1.0, d <= 0.0); }
  let t = clamp(0.5 - d / (3.0 * r), 0.0, 1.0);
  return t * t * (3.0 - 2.0 * t);
}

// the rim's response to the luma of the face under it, linear between (0, 0), (0.25, 0.63), (0.5, 1), (0.75, 1.13)
// and (1, 1)
fn rim_brightness(l0: f32) -> f32 {
  let l = clamp(l0, 0.0, 1.0);
  if (l < 0.25) { return l * 4.0 * 0.63; }
  if (l < 0.5) { return mix(0.63, 1.0, (l - 0.25) * 4.0); }
  if (l < 0.75) { return mix(1.0, 1.13, (l - 0.5) * 4.0); }
  return mix(1.13, 1.0, (l - 0.75) * 4.0);
}

// the rim's response to the edge's direction: 0.15 on straight edges, all of it on diagonal ones
fn rim_direction(n: vec2<f32>) -> f32 {
  let s = 2.0 * n.x * n.y;
  return 0.15 + 0.85 * s * s;
}

// the share of a Gaussian of deviation 1 below x
fn normal_cdf(x: f32) -> f32 {
  return 1.0 / (1.0 + exp(-1.702 * x));
}

// stage 7: the drop shadow outside the shape at its opacity (none at rest), and a hairline of the shadow's colour
// along the outside of the shape
fn outside_alpha(p: vec2<f32>, d: f32) -> f32 {
  var drop = 0.0;
  if (G.shadow.z > 0.0) { drop = G.shadow.z * coverage(field_d(p - G.shadow.xy), G.shadow.w); }
  var line = 0.0;
  if (G.glow.w > 0.0 && G.ring.w > 0.0) { line = G.glow.w * clamp(G.ring.w + 0.5 - d, 0.0, 1.0); }
  return min(drop + line, 1.0);
}

// the outward normal of the field at p, (0, 0) where it has none
fn field_normal(p: vec2<f32>) -> vec2<f32> {
  let g = vec2<f32>(field_d(p + vec2<f32>(0.5, 0.0)) - field_d(p - vec2<f32>(0.5, 0.0)),
                    field_d(p + vec2<f32>(0.0, 0.5)) - field_d(p - vec2<f32>(0.0, 0.5)));
  let gl = length(g);
  if (gl > 1e-6) { return g / gl; }
  return vec2<f32>(0.0);
}

// how much of the edge line shows where the outward normal is n: with arcs facing left and right (their width in
// diffuse.w), a raised cosine of the normal's angle from the horizontal, nothing LOBE_REACH of the width away; all of it
// without arcs or without a normal
fn edge_line_side_share(n: vec2<f32>) -> f32 {
  let reach = G.diffuse.w * LOBE_REACH;
  if (reach <= 0.0 || dot(n, n) < 1e-12) { return 1.0; }
  let a = acos(clamp(abs(n.x) / length(n), 0.0, 1.0));
  if (a >= reach) { return 0.0; }
  return 0.5 + 0.5 * cos(a / reach * PI);
}

// the edge line: black one pixel wide just outside the outline, at its strength times the rim's response to the
// brightness of what lies there as the shape's blur at `pos` reads it, so it shows over light content and not over
// black (an even line skips that response), and times the share the outline's direction n keeps
fn edge_line(p: vec2<f32>, d: f32, pos: f32, n: vec2<f32>) -> f32 {
  let band = clamp(1.5 - d, 0.0, 1.0);
  if (G.levels.x <= 0.0 || band <= 0.0) { return 0.0; }
  let share = edge_line_side_share(n);
  if (share <= 0.0) { return 0.0; }
  // an even line keeps its strength over any backdrop
  var response = 1.0;
  if (G.cut_shape.w <= 0.5) { response = rim_brightness(dot(blurred(uv_of(p), pos), LUMA)); }
  return min(G.levels.x, 1.0) * response * band * share;
}

// stage 7: what the shape draws outside itself where its outward normal is n, the drop shadow and the outline in the
// shadow's colour with the edge line over them in black: its colour and alpha
fn outside(p: vec2<f32>, d: f32, pos: f32, n: vec2<f32>) -> vec4<f32> {
  let shadow = outside_alpha(p, d);
  let line = edge_line(p, d, pos, n);
  let a = line + shadow * (1.0 - line);
  if (a <= 0.0) { return vec4<f32>(G.shadow_color.rgb, 0.0); }
  return vec4<f32>(G.shadow_color.rgb * (shadow * (1.0 - line) / a), a);
}

// the share of the pixel inside the outline the container is cut to: all of it when it is not cut
fn cut_coverage(p: vec2<f32>) -> f32 {
  if (G.cut_shape.z <= 0.0) { return 1.0; }
  return clamp(0.5 - rect_sdf(p, G.cut, G.cut_shape.x, G.cut_shape.y) / G.info.z, 0.0, 1.0);
}

@fragment
fn fs_glass(fin: VsOut) -> @location(0) vec4<f32> {
  let p = fin.px;
  let kept = cut_coverage(p);
  if (kept <= 0.0) { discard; }
  let f = field(p);
  let d = f.d;
  if (d > 0.5) {
    // the normal only matters to an edge line kept to the sides, in its band
    var on = vec2<f32>(0.0);
    if (G.diffuse.w > 0.0 && G.levels.x > 0.0 && d < 1.5) { on = field_normal(p); }
    let o = outside(p, d, f.b.z, on);
    let a = o.a * G.info.w * kept;
    if (a <= 0.0) { discard; }
    return vec4<f32>(o.rgb, a);
  }
  let g = vec2<f32>(field_d(p + vec2<f32>(0.5, 0.0)) - field_d(p - vec2<f32>(0.5, 0.0)),
                    field_d(p + vec2<f32>(0.0, 0.5)) - field_d(p - vec2<f32>(0.0, 0.5)));
  let gl = length(g);
  var n = vec2<f32>(0.0);
  if (gl > 1e-6) { n = g / gl; }
  // stage 3: refraction inside the band along the edge
  var off = vec2<f32>(0.0);
  if (f.a.z > 0.0 && d <= 0.0) {
    off = n * f.a.y * refraction_profile(max(-d / f.a.z, 0.0));
  }
  var c = face_at(p + off, f.b.z, f.b.y);
  // stage 4: bleed of the blur fill
  if (f.a.w > 0.0 && f.b.x > 0.0) {
    let t = clamp(-d / f.b.x, 0.0, 1.0);
    let w = f.a.w * (1.0 - t) * (1.0 - t);
    let tone = vec3<f32>(G.bleed.y) + (G.bleed.x - G.bleed.y)
      * min(saturate_encoded(tap(blur_fill, uv_of(p)), G.bleed.z), vec3<f32>(1.0));
    let b = select(max(c, tone), min(c, tone), G.bleed.w > 0.5);
    c = mix(c, b, w);
  }
  // stage 5: the soft band of light along the light's axis, then the rim
  let rim_on = G.highlight.x * G.highlight2.y != 0.0 && G.highlight.w > 0.0;
  let glow_on = G.glow.x > 0.0 && G.glow.y > 0.0;
  if ((rim_on || glow_on) && gl > 1e-6) {
    // the distance to the edge the field's slope gives: far inside along a seam between joined shapes
    let de = d / max(gl, MIN_FIELD_SLOPE);
    if (glow_on) {
      let na = atan2(-n.y, n.x);
      let fill = max(G.highlight2.w, 0.0);
      let ends = lobe(na, G.highlight.y, 1.0) + lobe(na, G.highlight.y + PI, 1.0) * fill;
      let xg = (de - G.glow.z) / G.glow.y;
      var w = G.glow.x * exp(-xg * xg) * ends;
      if (G.diffuse.z > 0.0 && G.diffuse.x > 0.0) {
        let xd = (de - G.glow.z) / (G.glow.y * G.diffuse.x);
        let wide = lobe(na, G.highlight.y, G.diffuse.y) + lobe(na, G.highlight.y + PI, G.diffuse.y) * fill;
        w = w + G.glow.x * G.diffuse.z * exp(-xd * xd) * wide;
      }
      c = min(c + key_color(c) * w, vec3<f32>(1.0));
    }
    // the rim: one shift of every channel, by the face's brightness and the edge's direction, in a band along it
    if (rim_on) {
      let band = clamp(G.highlight.w + 0.5 - max(-de, 0.0), 0.0, 1.0);
      let shift = G.highlight.x * G.highlight2.y * rim_brightness(dot(c, LUMA)) * rim_direction(n) * band;
      c = clamp(c + vec3<f32>(shift), vec3<f32>(0.0), vec3<f32>(1.0));
    }
  }
  // the black veil, from its opacity at the top of the shapes to its opacity at their bottom
  if (G.fill.w > 0.0 || G.levels.y > 0.0) {
    let vn = i32(G.info.x);
    var top = G.shapes[0].y;
    var bottom = G.shapes[0].y + G.shapes[0].w;
    for (var i = 1; i < vn; i++) {
      top = min(top, G.shapes[i].y);
      bottom = max(bottom, G.shapes[i].y + G.shapes[i].w);
    }
    let vt = clamp((p.y - top) / max(bottom - top, 1e-3), 0.0, 1.0);
    c = c * (1.0 - clamp(mix(G.fill.w, G.levels.y, vt), 0.0, 1.0));
  }
  // stage 6: the ring shadow, a blurred stroke along the inside of the outline moved up, darkening the face
  if (G.shadow_color.w > 0.0 && G.ring.y > 0.0) {
    let dr = field_d(p + vec2<f32>(0.0, G.ring.x));
    var m = 0.0;
    if (G.ring.z > 0.0) {
      m = normal_cdf(-dr / G.ring.z) - normal_cdf((-G.ring.y - dr) / G.ring.z);
    } else if (dr <= 0.0 && dr >= -G.ring.y) {
      m = 1.0;
    }
    c = c * (1.0 - min(G.shadow_color.w, 1.0) * max(m, 0.0));
  }
  // an edge pixel: the glass over what lies outside it in the part it leaves uncovered
  let cov = clamp(0.5 - d / G.info.z, 0.0, 1.0);
  let o = outside(p, d, f.b.z, n);
  let under = (1.0 - cov) * o.a;
  let a = cov + under;
  var rgb = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
  if (a > 0.0) { rgb = (rgb * cov + o.rgb * under) / a; }
  return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), a * G.info.w * kept);
}
