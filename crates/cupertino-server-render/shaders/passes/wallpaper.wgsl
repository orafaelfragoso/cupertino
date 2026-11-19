// The procedural desktop background: a diagonal gradient with soft colour blobs, so the material passes and the
// shadows have something to work on. Opaque at full opacity. Blend with (ONE, ONE_MINUS_SRC_ALPHA).

// include rect.wgsl

// Uniform block of the wallpaper (std140, mirrored by `render::passes::wallpaper_params`): its four colours, rgb
// straight (alpha unused). The gradient runs from c0 to c1; c2 and c3 colour the blobs.
struct WallpaperParams {
  c0: vec4<f32>,
  c1: vec4<f32>,
  c2: vec4<f32>,
  c3: vec4<f32>,
};

@group(0) @binding(0) var<uniform> W: WallpaperParams;

// A soft blob of falloff `k` at `centre` (0 to 1 across the element), round on the screen.
fn blob(uv: vec2<f32>, centre: vec2<f32>, k: f32) -> f32 {
  let d = (uv - centre) * vec2<f32>(place.area.x / place.area.y, 1.0);
  return exp(-dot(d, d) * k);
}

@fragment
fn fs_main(fin: VsOut) -> @location(0) vec4<f32> {
  let uv = fin.coords;
  var base = mix(W.c0.rgb, W.c1.rgb, clamp(uv.x * 0.45 + uv.y * 0.55, 0.0, 1.0));
  base = mix(base, W.c2.rgb, blob(uv, vec2<f32>(0.22, 0.30), 9.0) * 0.85);
  base = mix(base, W.c3.rgb, blob(uv, vec2<f32>(0.80, 0.68), 7.0) * 0.85);
  base = mix(base, W.c3.rgb, blob(uv, vec2<f32>(0.58, 0.04), 40.0) * 0.9);
  base = mix(base, W.c2.rgb, blob(uv, vec2<f32>(0.12, 0.02), 60.0) * 0.9);
  return vec4<f32>(base, 1.0) * place.area.z;
}
