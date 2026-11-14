// Where a region program (material, glass) draws: one quad per damage rectangle, mapped through the frame's
// projection so the output's transform is honoured, and whether the result is dithered by pixel before an 8-bit
// store. Drawn as a 4-vertex triangle strip; lengths are target pixels, y down.

// Uniform block of the placement (std140, mirrored by `Placement` in ops.rs).
//   proj  = the frame's projection from target pixels to clip space, column-major
//   quad  = x, y, w, h of the quad drawn, in target pixels
//   flags = (dither the result by its pixel: 1, 0, 0, 0)
struct Placement {
  proj: mat3x3<f32>,
  quad: vec4<f32>,
  flags: vec4<i32>,
};

@group(1) @binding(0) var<uniform> place: Placement;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  // the target pixel, y down
  @location(0) px: vec2<f32>,
  // the framebuffer-normalised coordinate of the same point
  @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
  let corner = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u));
  let p = place.quad.xy + corner * place.quad.zw;
  let c = place.proj * vec3<f32>(p, 1.0);
  var o: VsOut;
  o.pos = vec4<f32>(c.xy, 0.0, 1.0);
  o.px = p;
  o.uv = c.xy * 0.5 + vec2<f32>(0.5);
  return o;
}

// `c` dithered by its pixel when the placement asks for it.
fn placed(c: vec4<f32>, frag: vec2<f32>) -> vec4<f32> {
  if (place.flags.x != 0) {
    return vec4<f32>(dither_rgb(c.rgb, frag), c.a);
  }
  return c;
}
