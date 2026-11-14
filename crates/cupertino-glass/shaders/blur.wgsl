// One separable pass of the backdrop's Gaussian, clamped to the edge; dithered by pixel when its target has 8 bits.

// include pass.wgsl

fn weight(i: i32) -> f32 {
  return P.weights[i / 4][i % 4];
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(frag.xy);
  let dir = P.dir_radius.xy;
  let radius = P.dir_radius.z;
  var acc = ld(p) * weight(0);
  for (var i = 1; i <= radius; i++) {
    let o = dir * i;
    acc += (ld(p + o) + ld(p - o)) * weight(i);
  }
  var rgb = acc.rgb;
  if (P.flags.x != 0) {
    rgb = dither_rgb(rgb, frag.xy);
  }
  return vec4<f32>(rgb, 1.0);
}
