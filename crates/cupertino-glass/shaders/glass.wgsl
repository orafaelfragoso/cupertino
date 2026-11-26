// Glass composite: one draw per container over the union of its shapes, one quad per damage rectangle. The stages and
// every value are described in src/glass.rs, whose CPU model this follows. Lengths are target pixels, y down; colours
// are sRGB-encoded throughout. The level textures are sampled through the uniform's affine map from target pixels to
// their coordinates. Blend with (SRC_ALPHA, ONE_MINUS_SRC_ALPHA) for colour and (ZERO, ONE) for alpha.

// include place.wgsl

// Uniform block of the container (std140, mirrored by `GlassUniform` in glass/uniform.rs).

struct Glass {
  quad: vec4<f32>,        // where to draw: x, y, w, h
  uv_x: vec4<f32>,        // u = a·x + b·y + c; the deviation of the Gaussian the edge is softened by
  uv_y: vec4<f32>,        // v = d·x + e·y + f; 1 when the shapes differ in look
  info: vec4<f32>,        // shapes, blend width, antialiasing width, opacity
  levels: vec4<f32>,      // the edge line's strength, the veil at the bottom, target w, h
  fill: vec4<f32>,        // blur fill: darken, lighten, normal; the veil at the top
  bleed: vec4<f32>,       // edge bleed: white, black, saturation, darken
  light: vec4<f32>,       // soft band: light angle, spread, curvature, the far end's share
  glow: vec4<f32>,        // soft band: amount, half-width, offset; the outline's opacity
  diffuse: vec4<f32>,     // diffuse band: half-width, arcs' width, strength, per unit of the soft band's; the edge
                          // line's side arcs' width
  hm: array<vec4<f32>, 3>,
  shadow: vec4<f32>,      // offset x, offset y, alpha, blur
  shadow_color: vec4<f32>,// rgb, the outline's width
  shapes: array<vec4<f32>, 16>,
  shape_a: array<vec4<f32>, 16>, // corner extent, refraction strength, refraction band, bleed opacity
  shape_b: array<vec4<f32>, 16>, // bleed band, largest luminance, where the shape's blur lies among the blurred copies, corner exponent
  shape_c: array<vec4<f32>, 16>, // bleed reach, outer refraction strength, band, opacity
  shape_face: array<vec4<f32>, 16>,   // face: saturation, white, black, clamp
  shape_fill: array<vec4<f32>, 16>,   // face fill, straight
  shape_tint: array<vec4<f32>, 16>,   // tint, straight
  shape_rim: array<vec4<f32>, 16>,    // rim: strength, colour bias, depth; unused
  shape_spec: array<vec4<f32>, 16>,   // key and fill light: amount, half-width, band, key angle
  shape_spec2: array<vec4<f32>, 16>,  // the fill's share; diffuse glow: band, arcs' half-width, strength, per unit
  shape_ring: array<vec4<f32>, 16>,   // ring: moved down by, stroke, deviation, darkening
  shape_border: array<vec4<f32>, 16>, // control outline: opacity, width; the key light's matrix share; unused
  cut: vec4<f32>,         // the outline the container is cut to: x, y, w, h
  cut_shape: vec4<f32>,   // its corner extent, corner exponent, 1 when cut; 1 when the edge line is even
  lens: vec4<f32>,        // the blur's weight at the top and at the bottom of the shapes, magnification, the edge's
                          // width
  edge: vec4<f32>,        // the edge's straight colour and opacity
  rim_outside: vec4<f32>, // the rim outside the edge: its strip's start and end outside the outline, its shift per
                          // unit of brightness response (0: none); unused
};

@group(0) @binding(0) var<uniform> G: Glass;
@group(0) @binding(1) var level0: texture_2d<f32>;
@group(0) @binding(2) var level1: texture_2d<f32>;
@group(0) @binding(3) var level2: texture_2d<f32>;
@group(0) @binding(4) var level3: texture_2d<f32>;
@group(0) @binding(5) var blur_fill: texture_2d<f32>;
@group(0) @binding(6) var bleed_blur: texture_2d<f32>;
@group(0) @binding(7) var sharp: texture_2d<f32>;
// the sampler sits beside the placement block: the textures fill the first group
@group(1) @binding(1) var samp: sampler;

const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);
const PI = 3.14159265;
// how much of the control outline shows where the key or fill light falls on it fully
const BORDER_LIT_SHARE: f32 = 0.2;

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

// The field and the per-shape values mixed with its weights.
struct Field {
  d: f32,
  a: vec4<f32>,
  b: vec4<f32>,
  c: vec4<f32>,
  face: vec4<f32>,
  fill: vec4<f32>,
  tint: vec4<f32>,
  rim: vec4<f32>,
  spec: vec4<f32>,
  spec2: vec4<f32>,
  ring: vec4<f32>,
  border: vec4<f32>,
};

// x moved towards y by h: x itself when they agree
fn towards(x: vec4<f32>, y: vec4<f32>, h: f32) -> vec4<f32> {
  return x + (y - x) * h;
}

// Smooth union of every shape; the per-shape values are mixed with the same weights.
fn field(p: vec2<f32>) -> Field {
  let n = i32(G.info.x);
  let k = G.info.y;
  var f: Field;
  f.d = rect_sdf(p, G.shapes[0], G.shape_a[0].x, G.shape_b[0].w);
  f.a = G.shape_a[0];
  f.b = G.shape_b[0];
  f.c = G.shape_c[0];
  f.face = G.shape_face[0];
  f.fill = G.shape_fill[0];
  f.tint = G.shape_tint[0];
  f.rim = G.shape_rim[0];
  f.spec = G.shape_spec[0];
  f.spec2 = G.shape_spec2[0];
  f.ring = G.shape_ring[0];
  f.border = G.shape_border[0];
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
    f.c = towards(f.c, G.shape_c[i], h);
    f.face = towards(f.face, G.shape_face[i], h);
    f.fill = towards(f.fill, G.shape_fill[i], h);
    f.tint = towards(f.tint, G.shape_tint[i], h);
    f.rim = towards(f.rim, G.shape_rim[i], h);
    f.spec = towards(f.spec, G.shape_spec[i], h);
    f.spec2 = towards(f.spec2, G.shape_spec2[i], h);
    f.ring = towards(f.ring, G.shape_ring[i], h);
    f.border = towards(f.border, G.shape_border[i], h);
    f.d = d;
  }
  // where the shapes differ in look, each shape's look over the mix by the share of the pixel it covers, back to
  // front: a segment inside its group shows its own glass
  for (var i = 0; i < select(0, n, G.uv_y.w > 0.5); i++) {
    let cov = clamp(0.5 - rect_sdf(p, G.shapes[i], G.shape_a[i].x, G.shape_b[i].w) / G.info.z, 0.0, 1.0);
    if (cov > 0.0) {
      f.a.w = f.a.w + (G.shape_a[i].w - f.a.w) * cov;
      f.c.w = f.c.w + (G.shape_c[i].w - f.c.w) * cov;
      f.face = towards(f.face, G.shape_face[i], cov);
      f.fill = towards(f.fill, G.shape_fill[i], cov);
      f.tint = towards(f.tint, G.shape_tint[i], cov);
      f.rim = towards(f.rim, G.shape_rim[i], cov);
      f.spec = towards(f.spec, G.shape_spec[i], cov);
      f.spec2 = towards(f.spec2, G.shape_spec2[i], cov);
      f.ring = towards(f.ring, G.shape_ring[i], cov);
      f.border = towards(f.border, G.shape_border[i], cov);
    }
  }
  return f;
}

// the field's distance alone
fn field_d(p: vec2<f32>) -> f32 {
  let n = i32(G.info.x);
  let k = G.info.y;
  var fd = rect_sdf(p, G.shapes[0], G.shape_a[0].x, G.shape_b[0].w);
  for (var i = 1; i < n; i++) {
    let di = rect_sdf(p, G.shapes[i], G.shape_a[i].x, G.shape_b[i].w);
    if (k <= 0.0) {
      fd = min(di, fd);
    } else {
      let h = clamp(0.5 + 0.5 * (fd - di) / k, 0.0, 1.0);
      fd = mix(fd, di, h) - k * h * (1.0 - h);
    }
  }
  return fd;
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

// Stage 1: the blurred copy of the shape's blur at `pos` among them, or the two around it mixed; a copy of no weight
// is not read.
fn blurred(uv: vec2<f32>, pos: f32) -> vec3<f32> {
  let w = max(vec4<f32>(1.0) - abs(vec4<f32>(clamp(pos, 0.0, 3.0)) - vec4<f32>(0.0, 1.0, 2.0, 3.0)), vec4<f32>(0.0));
  var c = vec3<f32>(0.0);
  if (w.x > 0.0) { c += w.x * tap(level0, uv); }
  if (w.y > 0.0) { c += w.y * tap(level1, uv); }
  if (w.z > 0.0) { c += w.z * tap(level2, uv); }
  if (w.w > 0.0) { c += w.w * tap(level3, uv); }
  return c;
}

// Stages 1 and 2: the shape's blur, the blur fill and the face's tone at `q`, before its fill.
fn face_at(q: vec2<f32>, pos: f32, max_luma: f32, w: f32, face: vec4<f32>) -> vec3<f32> {
  let uv = uv_of(q);
  var c = blurred(uv, pos * w);
  let f = tap(blur_fill, uv);
  c = mix(c, min(c, f), G.fill.x * w);
  c = mix(c, max(c, f), G.fill.y * w);
  c = mix(c, f, G.fill.z * w);
  // stage 2, on encoded values: saturation, luma compression, levels, limit
  var s = saturate_encoded(c, face.x);
  var l = dot(s, LUMA);
  s = s - (1.0 - max_luma) * l * (0.75 * s + vec3<f32>(0.25 * l));
  l = dot(s, LUMA);
  s = s + vec3<f32>(face.z * (1.0 - l) - (1.0 - face.y) * l);
  return clamp(s, vec3<f32>(0.0), vec3<f32>(min(face.w, 1.0)));
}

// stage 3: the edge bleed's colour of what its blur reads, through its saturation and levels
fn bleed_tone(c: vec3<f32>) -> vec3<f32> {
  let y = dot(c, LUMA);
  let s = vec3<f32>(y) + (c - vec3<f32>(y)) * G.bleed.z;
  let l = dot(s, LUMA);
  return clamp(s + vec3<f32>(G.bleed.y * (1.0 - l) - (1.0 - G.bleed.x) * l), vec3<f32>(0.0), vec3<f32>(1.0));
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
  let reach = G.light.y * spread_scale * LOBE_REACH;
  if (reach <= 0.0) { return 0.0; }
  let a = angle_between(normal_angle, light);
  if (a >= reach) { return 0.0; }
  return pow(0.5 + 0.5 * cos(a / reach * PI), max(G.light.z, 0.001));
}

// an arc of the key or fill light: all of it towards the light, none `spread` away, along a smoothstep
fn light_arc(normal_angle: f32, light: f32, spread: f32) -> f32 {
  if (spread <= 0.0) { return 0.0; }
  return 1.0 - smoothstep(0.0, spread, angle_between(normal_angle, light));
}

// how much of the key and fill light falls on an edge whose outward normal points at `na` (y up), over arcs of
// half-width `spread`
fn key_and_fill(na: f32, spec: vec4<f32>, fill: f32, spread: f32) -> f32 {
  return light_arc(na, spec.w, spread) + fill * light_arc(na, spec.w + PI, spread);
}

// stage 6: the white light the key and fill add `depth` inside the edge where its outward normal points at `na`
fn specular(depth: f32, na: f32, spec: vec4<f32>, spec2: vec4<f32>) -> f32 {
  var band = 0.0;
  if (spec.z > 0.0) { band = 1.0 - smoothstep(0.0, spec.z, depth); }
  let key = key_and_fill(na, spec, spec2.x, spec.y);
  var glow = 0.0;
  let dh = spec.z * spec2.y;
  if (dh > 0.0 && depth < dh) {
    let td = 1.0 - depth / dh;
    glow = spec2.w * key_and_fill(na, spec, spec2.x, spec.y * spec2.z) * td * td;
  }
  return spec.x * (key * band + glow);
}

// the control outline's opacity `d` from the edge where its outward normal is n: a band of its width centred on the
// edge, hidden down to BORDER_LIT_SHARE where the key or fill light falls
fn border_line(d: f32, n: vec2<f32>, f: Field) -> f32 {
  if (f.border.x <= 0.0 || f.border.y <= 0.0) { return 0.0; }
  var line = f.border.x * clamp(0.5 * f.border.y + 0.5 - abs(d), 0.0, 1.0);
  if (line > 0.0 && f.spec.x > 0.0 && dot(n, n) > 0.0) {
    let lit = key_and_fill(atan2(-n.y, n.x), f.spec, f.spec2.x, f.spec.y);
    line = line * (1.0 - (1.0 - BORDER_LIT_SHARE) * min(1.0, lit));
  }
  return line;
}

fn key_color(c: vec3<f32>) -> vec3<f32> {
  let h = vec4<f32>(c, 1.0);
  return clamp(vec3<f32>(dot(G.hm[0], h), dot(G.hm[1], h), dot(G.hm[2], h)), vec3<f32>(0.0), vec3<f32>(1.0));
}

// the share of a shadow of blur radius r at the distance d from its caster's outline
fn shadow_coverage(d: f32, r: f32) -> f32 {
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

// stage 8: the drop shadow outside the shape at its opacity (none at rest), and a hairline of the shadow's colour
// along the outside of the shape
fn outside_alpha(p: vec2<f32>, d: f32) -> f32 {
  var drop = 0.0;
  if (G.shadow.z > 0.0) { drop = G.shadow.z * shadow_coverage(field_d(p - G.shadow.xy), G.shadow.w); }
  var line = 0.0;
  if (G.glow.w > 0.0 && G.shadow_color.w > 0.0) { line = G.glow.w * clamp(G.shadow_color.w + 0.5 - d, 0.0, 1.0); }
  return min(drop + line, 1.0);
}

// the outward normal of the field at p, (0, 0) where it has none
fn field_normal(p: vec2<f32>) -> vec2<f32> {
  let g = vec2<f32>(field_d(p + vec2<f32>(0.5, 0.0)) - field_d(p - vec2<f32>(0.5, 0.0)),
                    field_d(p + vec2<f32>(0.0, 0.5)) - field_d(p - vec2<f32>(0.0, 0.5)));
  let grad_len = length(g);
  if (grad_len > 1e-6) { return g / grad_len; }
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

// stage 8: the backdrop bent outside the edge, `d` from it where its outward normal is n, at its opacity fading out
// across its band: its colour and alpha
fn outer_refraction(p: vec2<f32>, d: f32, n: vec2<f32>, f: Field) -> vec4<f32> {
  if (f.c.w <= 0.0 || f.c.z <= 0.0 || f.c.y == 0.0) { return vec4<f32>(0.0); }
  let t = max(d, 0.0) / f.c.z;
  if (t >= 1.0) { return vec4<f32>(0.0); }
  let r = tap(sharp, uv_of(p + n * f.c.y * refraction_profile(t)));
  return vec4<f32>(r, min(f.c.w, 1.0) * (1.0 - smoothstep(0.0, 1.0, t)));
}

// the colour and alpha that bring a backdrop of colour b to t in normal blending, at the least alpha that keeps the
// colour within [0, 1]; none where t is b
fn over_backdrop(b0: vec3<f32>, t0: vec3<f32>) -> vec4<f32> {
  let b = clamp(b0, vec3<f32>(0.0), vec3<f32>(1.0));
  let t = clamp(t0, vec3<f32>(0.0), vec3<f32>(1.0));
  let down = select(vec3<f32>(0.0), (b - t) / max(b, vec3<f32>(1e-6)), t < b);
  let up = select(vec3<f32>(0.0), (t - b) / max(vec3<f32>(1.0) - b, vec3<f32>(1e-6)), t > b);
  let need = max(down, up);
  let a = min(max(need.x, max(need.y, need.z)), 1.0);
  if (a <= 0.0) { return vec4<f32>(0.0); }
  return vec4<f32>(clamp(b + (t - b) / a, vec3<f32>(0.0), vec3<f32>(1.0)), a);
}

// stage 8: the rim outside the edge, `d` outside it where its outward normal is n: each channel of what the shape's
// blur at `pos` reads there shifted by its brightness response, in the share of the pixel inside its strip, on the arcs
// facing the ends of the soft band's axis; as the colour and alpha laid over the backdrop
fn rim_outside(p: vec2<f32>, d: f32, pos: f32, n: vec2<f32>) -> vec4<f32> {
  let r = G.rim_outside;
  if (r.z == 0.0 || dot(n, n) <= 0.0) { return vec4<f32>(0.0); }
  let band = max(min(d + 0.5, r.y) - max(d - 0.5, r.x), 0.0);
  if (band <= 0.0) { return vec4<f32>(0.0); }
  let na = atan2(-n.y, n.x);
  let reach = LOBE_REACH * G.light.y;
  let arcs = light_arc(na, G.light.x, reach) + light_arc(na, G.light.x + PI, reach);
  let k = r.z * min(arcs, 1.0) * band;
  if (k == 0.0) { return vec4<f32>(0.0); }
  let b = blurred(uv_of(p), pos);
  let shift = vec3<f32>(rim_brightness(b.r), rim_brightness(b.g), rim_brightness(b.b)) * k;
  return over_backdrop(b, b + shift);
}

// stage 8: what the shape draws outside itself where its outward normal is n, the bent backdrop under the drop shadow
// and the outline in the shadow's colour, with the edge line and the control outline over them in black: its colour
// and alpha
fn outside(p: vec2<f32>, d: f32, pos: f32, n: vec2<f32>, f: Field) -> vec4<f32> {
  let shadow = outside_alpha(p, d);
  var line = edge_line(p, d, pos, n);
  let border = border_line(d, n, f);
  if (border > 0.0) { line = line + border - line * border; }
  let a = line + shadow * (1.0 - line);
  var o = vec4<f32>(G.shadow_color.rgb, 0.0);
  if (a > 0.0) { o = vec4<f32>(G.shadow_color.rgb * (shadow * (1.0 - line) / a), a); }
  let r = outer_refraction(p, d, n, f);
  if (r.a > 0.0) {
    let ao = o.a + r.a * (1.0 - o.a);
    o = vec4<f32>((o.rgb * o.a + r.rgb * r.a * (1.0 - o.a)) / ao, ao);
  }
  let s = rim_outside(p, d, pos, n);
  if (s.a > 0.0) {
    let ao = o.a + s.a * (1.0 - o.a);
    o = vec4<f32>((o.rgb * o.a + s.rgb * s.a * (1.0 - o.a)) / ao, ao);
  }
  return o;
}

// the share of a unit Gaussian below x deviations, from the error function's rational approximation
fn gaussian_cdf(x: f32) -> f32 {
  let z = abs(x) * 0.70710678;
  let t = 1.0 / (1.0 + 0.3275911 * z);
  let poly = t * (0.2548296 + t * (-0.28449674 + t * (1.4214137 + t * (-1.453152 + t * 1.0614054))));
  let erf = 1.0 - poly * exp(-z * z);
  return 0.5 * (1.0 + select(-erf, erf, x >= 0.0));
}

// the integral of the Gaussian's share up to u deviations: u·Φ(u) + φ(u)
fn gaussian_ramp(u: f32) -> f32 {
  return u * gaussian_cdf(u) + 0.3989423 * exp(-0.5 * u * u);
}

// the share of a pixel whose centre lies d outside the outline that the edge's band, from the outline to w outside it,
// covers; softened by a Gaussian of deviation s, that band blurred by it, in closed form
fn edge_share(d: f32, w: f32, s: f32) -> f32 {
  if (s <= 0.0) { return clamp(min(d + 0.5, w) - max(d - 0.5, 0.0), 0.0, 1.0); }
  let r = s * (gaussian_ramp((d + 0.5) / s) - gaussian_ramp((d - 0.5) / s) - gaussian_ramp((d + 0.5 - w) / s)
    + gaussian_ramp((d - 0.5 - w) / s));
  return clamp(r, 0.0, 1.0);
}

// stage 9's opacity at p: each shape's band from its outline to w outside it at the edge's opacity, left out by the
// share of the pixel each other shape covers, the bands laid over each other; a stroke round each shape cut by the
// others, so where the shapes overlap only the outline of the whole shows
fn edge_alpha(p: vec2<f32>) -> f32 {
  let w = G.lens.w;
  if (w <= 0.0 || G.edge.a <= 0.0) { return 0.0; }
  // a soft edge reaches three deviations further both ways
  let s = G.uv_x.w;
  let reach = 0.5 + 3.0 * max(s, 0.0);
  let n = i32(G.info.x);
  var d: array<f32, 16>;
  var near = false;
  for (var i = 0; i < n; i++) {
    d[i] = rect_sdf(p, G.shapes[i], G.shape_a[i].x, G.shape_b[i].w);
    near = near || (d[i] > -reach && d[i] < w + reach);
  }
  if (!near) { return 0.0; }
  var keep = 1.0;
  for (var i = 0; i < n; i++) {
    let band = edge_share(d[i], w, s);
    if (band <= 0.0) { continue; }
    var outside = 1.0;
    for (var j = 0; j < n; j++) {
      if (j != i) { outside *= 1.0 - clamp(0.5 - d[j], 0.0, 1.0); }
    }
    keep *= 1.0 - G.edge.a * band * outside;
  }
  return clamp(1.0 - keep, 0.0, 1.0);
}

// stage 9: the edge over what the pixel at p shows (straight colour c at coverage c.a), as a stroke drawn over it; its
// band in the field's units, as the glass's own coverage is
fn with_edge(c: vec4<f32>, p: vec2<f32>) -> vec4<f32> {
  let e = edge_alpha(p);
  if (e <= 0.0) { return c; }
  let a = c.a + e - c.a * e;
  return vec4<f32>((c.rgb * c.a * (1.0 - e) + G.edge.rgb * e) / a, a);
}

// the share of the pixel inside the outline the container is cut to: all of it when it is not cut
fn cut_coverage(p: vec2<f32>) -> f32 {
  if (G.cut_shape.z <= 0.0) { return 1.0; }
  return clamp(0.5 - rect_sdf(p, G.cut, G.cut_shape.x, G.cut_shape.y) / G.info.z, 0.0, 1.0);
}

// whether what the shape draws outside itself at `d` needs the outline's direction
fn outside_needs_normal(d: f32, f: Field) -> bool {
  let side_line = G.diffuse.w > 0.0 && G.levels.x > 0.0 && d < 1.5;
  let border = f.border.x > 0.0 && f.border.y > 0.0 && d < 0.5 * f.border.y + 0.5;
  let bent = f.c.w > 0.0 && f.c.z > 0.0 && f.c.y != 0.0 && d < f.c.z;
  let rim = G.rim_outside.z != 0.0 && d < G.rim_outside.y + 0.5;
  return side_line || border || bent || rim;
}

@fragment
fn fs_main(fin: VsOut) -> @location(0) vec4<f32> {
  let p = fin.px;
  let kept = cut_coverage(p);
  if (kept <= 0.0) { discard; }
  let f = field(p);
  let d = f.d;
  if (d > 0.5) {
    var on = vec2<f32>(0.0);
    if (outside_needs_normal(d, f)) { on = field_normal(p); }
    let o = with_edge(outside(p, d, f.b.z, on, f), p);
    let a = o.a * G.info.w * kept;
    if (a <= 0.0) { discard; }
    return placed(vec4<f32>(o.rgb, a), fin.pos.xy);
  }
  let g = vec2<f32>(field_d(p + vec2<f32>(0.5, 0.0)) - field_d(p - vec2<f32>(0.5, 0.0)),
                    field_d(p + vec2<f32>(0.0, 0.5)) - field_d(p - vec2<f32>(0.0, 0.5)));
  let grad_len = length(g);
  var n = vec2<f32>(0.0);
  if (grad_len > 1e-6) { n = g / grad_len; }
  // stage 2: refraction inside the band along the edge
  var off = vec2<f32>(0.0);
  if (f.a.z > 0.0 && d <= 0.0) {
    off = n * f.a.y * refraction_profile(max(-d / f.a.z, 0.0));
  }
  // a lens shows what lies behind it magnified about its centre; a ramp weighs the blur from top to bottom
  var q = p + off;
  if (G.lens.z > 1.0) {
    let lc = G.shapes[0].xy + 0.5 * G.shapes[0].zw;
    q = lc + (q - lc) / G.lens.z;
  }
  var blur_weight = 1.0;
  if (G.lens.x < 1.0 || G.lens.y < 1.0) {
    let rt = clamp((p.y - G.shapes[0].y) / max(G.shapes[0].w, 1e-3), 0.0, 1.0);
    blur_weight = mix(G.lens.x, G.lens.y, smoothstep(0.0, 1.0, rt));
  }
  var c = face_at(q, f.b.z, f.b.y, blur_weight, f.face);
  // stage 3: the edge bleed of what lies just outside the shape, then the fill and the tint over the face
  if (f.a.w > 0.0 && f.b.x > 0.0 && -d < f.b.x) {
    let tb = max(-d, 0.0) / f.b.x;
    let b = bleed_tone(tap(bleed_blur, uv_of(p + n * f.c.x * (1.0 - tb))));
    let wb = min(f.a.w, 1.0) * (1.0 - tb) * (1.0 - tb);
    c = mix(c, select(b, min(c, b), G.bleed.w > 0.5), wb);
  }
  c = mix(c, f.fill.rgb, f.fill.a);
  if (f.tint.a > 0.0) { c = mix(c, f.tint.rgb, f.tint.a); }
  // the distance to the edge the field's slope gives: far inside along a seam between joined shapes
  let de = d / max(grad_len, MIN_FIELD_SLOPE);
  // stage 4: the soft band of light along the light's axis, then the rim
  let rim_on = f.rim.x * f.rim.y != 0.0 && f.rim.z > 0.0;
  let glow_on = G.glow.x > 0.0 && G.glow.y > 0.0;
  if ((rim_on || glow_on) && grad_len > 1e-6) {
    if (glow_on) {
      let na = atan2(-n.y, n.x);
      let fill = max(G.light.w, 0.0);
      let ends = lobe(na, G.light.x, 1.0) + lobe(na, G.light.x + PI, 1.0) * fill;
      let xg = (de - G.glow.z) / G.glow.y;
      var w = G.glow.x * exp(-xg * xg) * ends;
      if (G.diffuse.z > 0.0 && G.diffuse.x > 0.0) {
        let xd = (de - G.glow.z) / (G.glow.y * G.diffuse.x);
        let wide = lobe(na, G.light.x, G.diffuse.y) + lobe(na, G.light.x + PI, G.diffuse.y) * fill;
        w = w + G.glow.x * G.diffuse.z * exp(-xd * xd) * wide;
      }
      c = min(c + key_color(c) * w, vec3<f32>(1.0));
    }
    // the rim: a shift of each channel by that channel's brightness and the edge's direction, in a band along it
    if (rim_on) {
      let band = clamp(f.rim.z + 0.5 - max(-de, 0.0), 0.0, 1.0);
      let k = f.rim.x * f.rim.y * rim_direction(n) * band;
      let shift = vec3<f32>(rim_brightness(c.r), rim_brightness(c.g), rim_brightness(c.b)) * k;
      c = clamp(c + shift, vec3<f32>(0.0), vec3<f32>(1.0));
    }
  }
  // stage 5: the control outline, then the key and fill light added as white over it, or the highlight matrix's
  // colour of the face laid over it at their strength, by their matrix share
  let line = border_line(d, n, f);
  if (line > 0.0) { c = c * (1.0 - line); }
  if (f.spec.x > 0.0 && grad_len > 1e-6) {
    let na = atan2(-n.y, n.x);
    let light = specular(max(-de, 0.0), na, f.spec, f.spec2);
    let white = clamp(c + vec3<f32>(light), vec3<f32>(0.0), vec3<f32>(1.0));
    let share = clamp(f.border.z, 0.0, 1.0);
    if (share > 0.0 && light > 0.0) {
      c = mix(white, mix(c, key_color(c), min(light, 1.0)), share);
    } else {
      c = white;
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
  // stage 7: the ring shadow, a blurred stroke along the inside of the outline moved down, darkening the face
  if (f.ring.w > 0.0 && f.ring.y > 0.0) {
    let dr = field_d(p - vec2<f32>(0.0, f.ring.x));
    var m = 0.0;
    if (f.ring.z > 0.0) {
      m = normal_cdf(-dr / f.ring.z) - normal_cdf((-f.ring.y - dr) / f.ring.z);
    } else if (dr <= 0.0 && dr >= -f.ring.y) {
      m = 1.0;
    }
    c = c * (1.0 - min(f.ring.w, 1.0) * max(m, 0.0));
  }
  // an edge pixel: the glass over what lies outside it in the part it leaves uncovered
  let cov = clamp(0.5 - d / G.info.z, 0.0, 1.0);
  let o = outside(p, d, f.b.z, n, f);
  let under = (1.0 - cov) * o.a;
  let a = cov + under;
  var rgb = c;
  if (a > 0.0) { rgb = (clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * cov + o.rgb * under) / a; }
  let edged = with_edge(vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), a), p);
  return placed(vec4<f32>(clamp(edged.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), edged.a * G.info.w * kept), fin.pos.xy);
}
