// Glass composite: one draw per container over the union of its shapes. The stages and every value are described in
// src/glass.rs, whose CPU model this follows. Lengths are target pixels, y down; colours are sRGB-encoded except in
// the face stage, which works on linear light.

struct Glass {
  quad: vec4<f32>,        // where to draw: x, y, w, h
  uv_x: vec4<f32>,        // u = a·x + b·y + c
  uv_y: vec4<f32>,        // v = d·x + e·y + f
  info: vec4<f32>,        // shapes, blend width, antialiasing width, opacity
  levels: vec4<f32>,      // unused (0, 0), target w, h
  fill: vec4<f32>,        // blur fill: darken, lighten, normal
  face: vec4<f32>,        // saturation, white, black, clamp
  face_fill: vec4<f32>,
  bleed: vec4<f32>,       // white, black, saturation, darken
  highlight: vec4<f32>,   // amount, angle, spread, half-width
  highlight2: vec4<f32>,  // offset, colour bias, curvature
  glow: vec4<f32>,        // soft band: amount, half-width, offset
  hm: array<vec4<f32>, 3>,
  shadow: vec4<f32>,      // offset x, offset y, alpha, blur
  shadow_color: vec4<f32>,// rgb, ring alpha
  ring: vec4<f32>,        // offset, stroke, blur
  shapes: array<vec4<f32>, 16>,
  shape_a: array<vec4<f32>, 16>, // radius, refraction offset at the edge, refraction band, bleed fraction
  shape_b: array<vec4<f32>, 16>, // bleed band, largest luminance, where the shape's blur lies among the blurred copies
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

fn rect_sdf(p: vec2<f32>, r: vec4<f32>, radius: f32) -> f32 {
  let hs = r.zw * 0.5;
  let rad = clamp(radius, 0.0, min(hs.x, hs.y));
  let q = abs(p - (r.xy + hs)) - (hs - vec2<f32>(rad));
  return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - rad;
}

struct Field { d: f32, a: vec4<f32>, b: vec4<f32> };

// Smooth union of every shape; the per-shape values are mixed with the same weights.
fn field(p: vec2<f32>) -> Field {
  let n = i32(G.info.x);
  let k = G.info.y;
  var f: Field;
  f.d = rect_sdf(p, G.shapes[0], G.shape_a[0].x);
  f.a = G.shape_a[0];
  f.b = G.shape_b[0];
  for (var i = 1; i < n; i++) {
    let di = rect_sdf(p, G.shapes[i], G.shape_a[i].x);
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

fn decode(c: vec3<f32>) -> vec3<f32> {
  return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn encode(c0: vec3<f32>) -> vec3<f32> {
  let c = max(c0, vec3<f32>(0.0));
  return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
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
  var lin = decode(saturate_encoded(c, G.face.x));
  lin = min(lin * G.face.y, vec3<f32>(G.face.w));
  let limit = max_luma * G.face.w;
  let y = dot(lin, LUMA);
  if (y > limit && y > 0.0) { lin = lin * (limit / y); }
  var e = min(encode(lin), vec3<f32>(1.0));
  e = e + G.face.z * (vec3<f32>(1.0) - e);
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

fn lobe(normal_angle: f32, light: f32) -> f32 {
  let reach = G.highlight.z * LOBE_REACH;
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

@fragment
fn fs_glass(fin: VsOut) -> @location(0) vec4<f32> {
  let p = fin.px;
  let f = field(p);
  let d = f.d;
  if (d > 0.5) {
    // stage 6: shadows outside the shape, drawn at their opacity (none at rest)
    let outlined = G.glow.w > 0.0 && G.ring.w > 0.0;
    if (G.shadow.z <= 0.0 && G.shadow_color.w <= 0.0 && !outlined) { discard; }
    let drop = G.shadow.z * coverage(field_d(p - G.shadow.xy), G.shadow.w);
    let dr = field_d(p - vec2<f32>(0.0, G.ring.x));
    let ring = G.shadow_color.w * max(coverage(dr, G.ring.z) - coverage(dr + G.ring.y, G.ring.z), 0.0);
    // a hairline of the shadow's colour along the outside of the shape
    var line = 0.0;
    if (outlined) { line = G.glow.w * clamp(G.ring.w + 0.5 - d, 0.0, 1.0); }
    let a = min(drop + ring + line, 1.0) * G.info.w;
    if (a <= 0.0) { discard; }
    return vec4<f32>(G.shadow_color.rgb, a);
  }
  let g = vec2<f32>(field_d(p + vec2<f32>(0.5, 0.0)) - field_d(p - vec2<f32>(0.5, 0.0)),
                    field_d(p + vec2<f32>(0.0, 0.5)) - field_d(p - vec2<f32>(0.0, 0.5)));
  let gl = length(g);
  var n = vec2<f32>(0.0);
  if (gl > 1e-6) { n = g / gl; }
  // stage 3: refraction inside the band along the edge
  var off = vec2<f32>(0.0);
  if (f.a.z > 0.0 && d <= 0.0) {
    let t = clamp(-d / f.a.z, 0.0, 1.0);
    off = n * f.a.y * (1.0 - t) * (1.0 - t);
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
  // stage 5: key and fill rim lights, added along the light's axis
  let rim_on = G.highlight.x > 0.0 && G.highlight.w > 0.0;
  let glow_on = G.glow.x > 0.0 && G.glow.y > 0.0;
  if ((rim_on || glow_on) && gl > 1e-6) {
    let na = atan2(-n.y, n.x);
    let ends = lobe(na, G.highlight.y) + lobe(na, G.highlight.y + PI) * max(1.0 + G.highlight2.y, 0.0);
    // the distance to the edge the field's slope gives: far inside along a seam between joined shapes
    let de = d / max(gl, MIN_FIELD_SLOPE);
    var strength = 0.0;
    if (rim_on) {
      let x = (de - G.highlight2.x) / G.highlight.w;
      strength = G.highlight.x * exp(-x * x);
    }
    // a soft band wider than the rim, lighting the same ends of the axis
    if (glow_on) {
      let xg = (de - G.glow.z) / G.glow.y;
      strength = strength + G.glow.x * exp(-xg * xg);
    }
    c = min(c + key_color(c) * (strength * ends), vec3<f32>(1.0));
  }
  let cov = clamp(0.5 - d / G.info.z, 0.0, 1.0) * G.info.w;
  return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), cov);
}
