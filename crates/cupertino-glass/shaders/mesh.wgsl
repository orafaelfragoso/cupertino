// A window snapshot drawn through an effect mesh (minimize, closing): a triangle list whose positions, in output
// pixels, are mapped through the frame's projection, each texel of the premultiplied snapshot scaled by the element's
// opacity. Blend with (ONE, ONE_MINUS_SRC_ALPHA).

// Uniform block of a mesh draw (std140, mirrored by `ops::MeshUniform`).
//   proj  = the frame's projection from output pixels to clip space, column-major (the compositor's mesh element)
//   alpha = (opacity, 0, 0, 0)
struct Mesh {
  proj: mat3x3<f32>,
  alpha: vec4<f32>,
};

@group(0) @binding(0) var<uniform> M: Mesh;
@group(0) @binding(1) var snapshot: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
  let p = M.proj * vec3<f32>(pos, 1.0);
  var o: VsOut;
  o.pos = vec4<f32>(p.xy, 0.0, 1.0);
  o.uv = uv;
  return o;
}

@fragment
fn fs_main(fin: VsOut) -> @location(0) vec4<f32> {
  return textureSample(snapshot, samp, fin.uv) * M.alpha.x;
}
