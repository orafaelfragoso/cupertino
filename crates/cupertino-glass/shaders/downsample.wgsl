// 2x box downsample of the backdrop pyramid (variance 1/4 source texel² per axis).

// include pass.wgsl

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(frag.xy) * 2;
  let c = (ld(p) + ld(p + vec2<i32>(1, 0)) + ld(p + vec2<i32>(0, 1)) + ld(p + vec2<i32>(1, 1))) * 0.25;
  return vec4<f32>(c.rgb, 1.0);
}
