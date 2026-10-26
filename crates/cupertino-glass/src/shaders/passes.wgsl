// Backdrop processing passes: downsample, separable blur, colour filter chain, blit.
//
// All passes are full-screen fragment passes (no compute) so that every stage has a direct GLSL
// ES 3.00 counterpart under shaders/gles/. Values are processed in gamma-encoded RGB
// (the layer framework blends encoded values).

struct Op { a: vec4<f32>, b: vec4<f32> };

// Uniform block shared by every pass (std140-compatible layout, mirrored in ops.rs).
//   sizes      = (src_w, src_h, dst_w, dst_h)
//   dir_radius = (dir_x, dir_y, blur radius in texels, number of colour ops)
//   weights    = one-sided Gaussian weights, weights[i/4][i%4] for tap i
//   ops        = colour filter chain: a.x = kind (1 saturate, 2 luminance curve, 3 monochrome),
//                a.y = amount, b = kind-specific parameters
struct PassParams {
  sizes: vec4<i32>,
  dir_radius: vec4<i32>,
  weights: array<vec4<f32>, 9>,
  ops: array<Op, 8>,
};

@group(0) @binding(0) var<uniform> P: PassParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> VsOut {
  let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
  var o: VsOut;
  o.pos = vec4<f32>(xy * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
  return o;
}

fn ld(p: vec2<i32>) -> vec4<f32> {
  let q = clamp(p, vec2<i32>(0), P.sizes.xy - vec2<i32>(1));
  return textureLoad(src, q, 0);
}

// 2x box downsample (variance 1/4 source texel² per axis)
@fragment
fn fs_downsample(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(frag.xy) * 2;
  let c = (ld(p) + ld(p + vec2<i32>(1, 0)) + ld(p + vec2<i32>(0, 1)) + ld(p + vec2<i32>(1, 1))) * 0.25;
  return vec4<f32>(c.rgb, 1.0);
}

fn weight(i: i32) -> f32 {
  return P.weights[i / 4][i % 4];
}

// separable Gaussian, clamp-to-edge (≈ inputNormalizeEdges)
@fragment
fn fs_blur(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(frag.xy);
  let dir = P.dir_radius.xy;
  let radius = P.dir_radius.z;
  var acc = ld(p) * weight(0);
  for (var i = 1; i <= radius; i++) {
    let o = dir * i;
    acc += (ld(p + o) + ld(p - o)) * weight(i);
  }
  return vec4<f32>(acc.rgb, 1.0);
}

const LUMA709 = vec3<f32>(0.2125, 0.7154, 0.0721);   // luminance curve weights
const SAT709 = vec3<f32>(0.2126, 0.7152, 0.0722);    // saturation matrix weights

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
  return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c < vec3<f32>(0.04045));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
  return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c < vec3<f32>(0.0031308));
}

// `next` is the operation after `op` (the tail of a linear matrix).
fn color_op(c: vec3<f32>, op: Op, next: Op) -> vec3<f32> {
  let k = i32(op.a.x);
  if (k == 1) {            // saturate
    let s = op.a.y;
    let l = dot(c, SAT709);
    return vec3<f32>(l) * (1.0 - s) + c * s;
  } else if (k == 2) {     // luminance curve: b = Horner coefficients (a, b, c, d), a.y = amount
    let x0 = dot(c, LUMA709);
    let p = op.b;
    let x = clamp(((p.x * x0 + p.y) * x0 + p.z) * x0 + p.w, 0.0, 1.0);
    return mix(c, vec3<f32>(x), op.a.y);
  } else if (k == 3) {     // monochrome (approximate)
    let l = dot(c, LUMA709);
    return mix(c, l * op.b.xyz, op.a.y);
  } else if (k == 4) {     // display filter matrix on linear light (linear)
    let m = mat3x3<f32>(op.a.y, op.b.x, op.b.w, op.a.z, op.b.y, next.a.y, op.a.w, op.b.z, next.a.z);
    return srgb_encode(clamp(m * srgb_decode(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))), vec3<f32>(0.0), vec3<f32>(1.0)));
  } else if (k == 5) {     // invert
    return vec3<f32>(1.0) - c;
  } else if (k == 6) {     // contrast stretch
    let t = op.a.y;
    if (t >= 1.0) { return step(vec3<f32>(0.5), c); }
    return clamp((c - vec3<f32>(0.5 * t)) / (1.0 - t), vec3<f32>(0.0), vec3<f32>(1.0));
  }
  return c;
}

@fragment
fn fs_color(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  var rgb = ld(vec2<i32>(frag.xy)).rgb;
  let n = P.dir_radius.w;
  for (var i = 0; i < n; i++) { rgb = color_op(rgb, P.ops[i], P.ops[min(i + 1, 7)]); }
  return vec4<f32>(rgb, 1.0);
}

// Copies the source into the target (bilinear; exact when both have the same size).
@fragment
fn fs_blit(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  let uv = frag.xy / vec2<f32>(P.sizes.zw);
  return vec4<f32>(textureSampleLevel(src, samp, uv, 0.0).rgb, 1.0);
}
