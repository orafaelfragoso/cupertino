// Vibrant blending for template glyphs and text drawn on top of a material.
//
// the vibrant blend functions operate
// on premultiplied colour, with
//   vibrantDark(s, d)  = lightenSover(s·c1, colorDodge(s·c0, d))
//   vibrantLight(s, d) = darkenSover(s·c1, colorBurn(s·c0, d))
// and the reversed variants swap the nesting order. lightenSover scales S by (1−y)⁴ and
// darkenSover by y⁴, y = Rec.709 luma of D, followed by source-over.
// The constants c0 / c1 come from the vibrancy effect parameters and are unknown: they are
// uniforms here (identity by default on the Rust side).

struct Vibrant {
  rect: vec4<f32>,     // x, y, w, h in target pixels (y down)
  uv: vec4<f32>,       // glyph texture region (u0, v0, u1, v1)
  sizes: vec4<f32>,    // target w, h, 0, 0
  mode: vec4<f32>,     // x: 0 = dark, 1 = light; y: reversed (0/1)
  c0: vec4<f32>,       // colour dodge / colour burn multiplier
  c1: vec4<f32>,       // lighten-sover / darken-sover multiplier
};

@group(0) @binding(0) var<uniform> V: Vibrant;
@group(0) @binding(1) var glyphs: texture_2d<f32>;
@group(0) @binding(2) var backdrop: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) tex: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
  let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
  let p = V.rect.xy + corner * V.rect.zw;
  var o: VsOut;
  o.pos = vec4<f32>(p.x / V.sizes.x * 2.0 - 1.0, 1.0 - p.y / V.sizes.y * 2.0, 0.0, 1.0);
  o.tex = mix(V.uv.xy, V.uv.zw, corner);
  return o;
}

const LUMA709 = vec3<f32>(0.2125, 0.7154, 0.0721);

fn lighten_sover(s_in: vec4<f32>, d: vec4<f32>) -> vec4<f32> {
  let y = 1.0 - dot(d.rgb, LUMA709);
  let s = s_in * (y * y) * (y * y);
  return d * (1.0 - s.a) + s;
}

fn darken_sover(s_in: vec4<f32>, d: vec4<f32>) -> vec4<f32> {
  let y = dot(d.rgb, LUMA709);
  let s = s_in * (y * y) * (y * y);
  return d * (1.0 - s.a) + s;
}

fn color_dodge(s: vec4<f32>, d: vec4<f32>) -> vec4<f32> {
  var r = d * (1.0 - s.a) + s * (1.0 - d.a);
  let t = vec3<f32>(s.a) - s.rgb;
  let m = step(vec3<f32>(0.005), t);
  r = vec4<f32>(r.rgb + mix(vec3<f32>(s.a), d.rgb * s.a * s.a / max(t, vec3<f32>(0.005)), m), r.a);
  r.a += d.a * s.a;
  return vec4<f32>(clamp(r.rgb, vec3<f32>(0.0), vec3<f32>(r.a)), r.a);
}

fn color_burn(s: vec4<f32>, d: vec4<f32>) -> vec4<f32> {
  var r = d * (1.0 - s.a) + s * (1.0 - d.a);
  let dasa = d.a * s.a;
  r = vec4<f32>(r.rgb + step(vec3<f32>(0.005), s.rgb) * (vec3<f32>(dasa) - s.a * s.a * (vec3<f32>(d.a) - d.rgb) / max(s.rgb, vec3<f32>(0.005))), r.a);
  r.a += dasa;
  return vec4<f32>(clamp(r.rgb, vec3<f32>(0.0), vec3<f32>(r.a)), r.a);
}

fn vibrant_dark(s0: vec4<f32>, d0: vec4<f32>, reversed: bool) -> vec4<f32> {
  if (reversed) {
    return color_dodge(s0 * V.c0, lighten_sover(s0 * V.c1, d0));
  }
  return lighten_sover(s0 * V.c1, color_dodge(s0 * V.c0, d0));
}

fn vibrant_light(s0: vec4<f32>, d0: vec4<f32>, reversed: bool) -> vec4<f32> {
  if (reversed) {
    return color_burn(s0 * V.c0, darken_sover(s0 * V.c1, d0));
  }
  return darken_sover(s0 * V.c1, color_burn(s0 * V.c0, d0));
}

// Writes the blended colour (no hardware blending): `backdrop` must hold the current content of
// the target, `glyphs` is premultiplied.
@fragment
fn fs_main(fin: VsOut) -> @location(0) vec4<f32> {
  let s0 = textureSampleLevel(glyphs, samp, fin.tex, 0.0);
  let p = vec2<i32>(fin.pos.xy);
  let d0 = vec4<f32>(textureLoad(backdrop, p, 0).rgb, 1.0);
  let reversed = V.mode.y > 0.5;
  var r: vec4<f32>;
  if (V.mode.x > 0.5) { r = vibrant_light(s0, d0, reversed); } else { r = vibrant_dark(s0, d0, reversed); }
  return vec4<f32>(clamp(r.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
