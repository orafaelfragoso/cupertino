// Material region: a rounded mask from a signed distance field, the backdrop as the base colour, then the recipe's
// fill layers with their compositing filters. Blend with (SRC_ALPHA, ONE_MINUS_SRC_ALPHA) for colour and (ZERO, ONE)
// for alpha.
//
// The backdrop is always bound (a recipe without a backdrop filter reads an unfiltered copy of the target) and shares
// the target's orientation; it may cover only part of the target, the part around the region it was made for:
// Region.info.zw is its origin and Region.sizes.zw its size, in target pixels.

// include place.wgsl

// Uniform block of the region (std140, mirrored by `RegionParams` in ops.rs).
struct Region {
  rect: vec4<f32>,      // x, y, w, h in target pixels (y down)
  shape: vec4<f32>,     // corner extent (px), superellipse exponent, opacity, antialiasing width (px)
  info: vec4<f32>,      // number of fill layers, 1, the backdrop's origin x, y
  sizes: vec4<f32>,     // target w, h, the backdrop's w, h
  tint: vec4<f32>,      // tint layer colour (rgb)
  fills: array<vec4<f32>, 16>,  // per fill: (blend id, tint flag, 0, 0), straight rgba
};

@group(0) @binding(0) var<uniform> R: Region;
@group(0) @binding(1) var backdrop: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

// Signed distance (negative inside) to a rounded rectangle centred at the origin with half extents `hs`. `e` is the
// corner extent; `n` the superellipse exponent (2 = circular). Same formula as
// cupertino_geometry::RoundedRect::sdf_approx.
fn region_sdf(p: vec2<f32>, hs: vec2<f32>, e: f32, n: f32) -> f32 {
  let q = abs(p) - (hs - vec2<f32>(e));
  if (q.x <= 0.0 && q.y <= 0.0) { return max(q.x, q.y) - e; }
  if (q.x <= 0.0) { return q.y - e; }
  if (q.y <= 0.0) { return q.x - e; }
  if (e <= 0.0) { return length(q); }
  if (n <= 2.001) { return length(q) - e; }
  let u = max(q.x / e, 1e-6);
  let v = max(q.y / e, 1e-6);
  let g = pow(pow(u, n) + pow(v, n), 1.0 / n);
  let grad = sqrt(pow(u, 2.0 * n - 2.0) + pow(v, 2.0 * n - 2.0)) / pow(g, n - 1.0);
  let d = (g - 1.0) / grad * e;
  return max(d, max(q.x, q.y) - e);
}

fn soft_light(s: f32, d: f32) -> f32 {   // W3C compositing definition
  if (s <= 0.5) { return d - (1.0 - 2.0 * s) * d * (1.0 - d); }
  var g: f32;
  if (d <= 0.25) { g = ((16.0 * d - 12.0) * d + 4.0) * d; } else { g = sqrt(d); }
  return d + (2.0 * s - 1.0) * (g - d);
}

// d is opaque; s = straight source colour, a = source alpha.
// Blend ids: 0 normal, 1 plus-lighter, 2 plus-darker, 3 darken, 4 lighten, 5 soft-light, 6 copy.
fn blend(mode: i32, s: vec3<f32>, a: f32, d: vec3<f32>) -> vec3<f32> {
  var b: vec3<f32>;
  switch mode {
    case 1: { b = min(vec3<f32>(1.0), s + d); }
    case 2: { b = max(vec3<f32>(0.0), s + d - vec3<f32>(1.0)); }
    case 3: { b = min(s, d); }
    case 4: { b = max(s, d); }
    case 5: { b = vec3<f32>(soft_light(s.r, d.r), soft_light(s.g, d.g), soft_light(s.b, d.b)); }
    case 6: { return s * a; }
    default: { b = s; }
  }
  return d * (1.0 - a) + b * a;
}

@fragment
fn fs_main(fin: VsOut) -> @location(0) vec4<f32> {
  let hs = R.rect.zw * 0.5;
  let d = region_sdf(fin.px - (R.rect.xy + hs), hs, R.shape.x, R.shape.y);
  let cov = clamp(0.5 - d / R.shape.w, 0.0, 1.0) * R.shape.z;
  if (cov <= 0.0) { discard; }
  let uv = (fin.uv * R.sizes.xy - R.info.zw) / R.sizes.zw;
  var base = textureSampleLevel(backdrop, samp, uv, 0.0).rgb;
  let n = i32(R.info.x);
  for (var i = 0; i < n; i++) {
    let a = R.fills[2 * i];
    let b = R.fills[2 * i + 1];
    var s = b.xyz;
    if (a.y > 0.5) { s = R.tint.rgb; }
    base = blend(i32(a.x), s, b.w, base);
  }
  return placed(vec4<f32>(clamp(base, vec3<f32>(0.0), vec3<f32>(1.0)), cov), fin.pos.xy);
}
