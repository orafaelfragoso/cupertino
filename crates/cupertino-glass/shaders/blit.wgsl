// Copies the source into the target (bilinear; exact when both have the same size).

// include pass.wgsl

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  let uv = frag.xy / vec2<f32>(P.sizes.zw);
  return vec4<f32>(textureSampleLevel(src, samp, uv, 0.0).rgb, 1.0);
}
