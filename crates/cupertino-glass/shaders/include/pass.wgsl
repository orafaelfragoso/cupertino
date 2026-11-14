// What every backdrop pass shares: its uniform block, its source texture and the full-screen triangle it is drawn
// with. Each pass (downsample, blur, color, blit) is one program including this file; values are processed in
// gamma-encoded RGB (the layer framework blends encoded values).

struct Op { a: vec4<f32>, b: vec4<f32> };

// Uniform block shared by every pass (std140, mirrored by `PassParams` in ops.rs).
//   sizes      = (src_w, src_h, dst_w, dst_h)
//   dir_radius = (dir_x, dir_y, blur radius in texels, number of colour ops)
//   weights    = one-sided Gaussian weights, weights[i/4][i%4] for tap i
//   ops        = colour filter chain: a.x = kind (1 saturate, 2 luminance curve, 3 monochrome, 4 linear matrix,
//                5 invert, 6 contrast, 7 matrix tail), a.y = amount, b = kind-specific parameters
//   flags      = (dither the result by its pixel before an 8-bit store: 1, keep the source's alpha: 1, 0, 0)
struct PassParams {
  sizes: vec4<i32>,
  dir_radius: vec4<i32>,
  weights: array<vec4<f32>, 9>,
  ops: array<Op, 8>,
  flags: vec4<i32>,
};

@group(0) @binding(0) var<uniform> P: PassParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
  var o: VsOut;
  o.pos = fullscreen_position(i);
  return o;
}

// The source texel at p, clamped to the source.
fn ld(p: vec2<i32>) -> vec4<f32> {
  let q = clamp(p, vec2<i32>(0), P.sizes.xy - vec2<i32>(1));
  return textureLoad(src, q, 0);
}
