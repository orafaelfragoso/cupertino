// Window shadow: the Gaussian-blurred rounded box of the window's shape in closed form (the shared drop shadow),
// optionally with the window's own area cut out. Output is premultiplied black, dithered by its pixel when the
// placement asks. Blend with (ONE, ONE_MINUS_SRC_ALPHA).

// include rect.wgsl

// Uniform block of a shadow (std140, mirrored by `render::passes::shadow_params`).
//   caster = the window's rectangle (x, y, w, h) in points from the element's origin
//   params = corner radius (pt), sigma (pt), density, y offset (pt)
//   flags  = cut the window's shape out (1) or not (0), corner exponent, pixels per point, corner extent (pt)
struct ShadowParams {
  caster: vec4<f32>,
  params: vec4<f32>,
  flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> S: ShadowParams;

@fragment
fn fs_main(fin: VsOut) -> @location(0) vec4<f32> {
  let pos = fin.coords * place.area.xy;
  let hs = S.caster.zw * 0.5;
  let centre = S.caster.xy + hs;
  let caster = Caster(hs, S.params.x, S.flags.w, S.flags.y);
  let shadow = Shadow(S.params.y, vec2<f32>(0.0, S.params.w), S.params.z);
  var a = drop_shadow(pos - centre, caster, shadow, S.flags.x > 0.5, S.flags.z) * place.area.z;
  if (place.area.w > 0.5) {
    a = dither(a, fin.pos.xy);
  }
  return vec4<f32>(0.0, 0.0, 0.0, a);
}
