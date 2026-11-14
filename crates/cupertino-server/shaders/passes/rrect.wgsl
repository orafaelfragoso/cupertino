// A solid rounded rectangle with an optional border for the shell's own drawing (dock tiles, highlights, the
// switcher), its coverage from the shared distance. Output is premultiplied. Blend with (ONE, ONE_MINUS_SRC_ALPHA).

// include rect.wgsl

// Uniform block of a rounded rectangle (std140, mirrored by `render::passes::rounded_params`).
//   rect   = x, y, w, h in points from the element's origin
//   shape  = radius (pt), border width (pt), corner exponent, pixels per point
//   fill   = straight rgba
//   border = straight rgba
//   edge   = powers on |n.x| and |n.y| of the unit outward normal (0: left out), floor; all 0: the same all around
struct RoundedParams {
  rect: vec4<f32>,
  shape: vec4<f32>,
  fill: vec4<f32>,
  border: vec4<f32>,
  edge: vec4<f32>,
};

@group(0) @binding(0) var<uniform> R: RoundedParams;

@fragment
fn fs_main(fin: VsOut) -> @location(0) vec4<f32> {
  let pos = fin.coords * place.area.xy;
  let hs = R.rect.zw * 0.5;
  let radius = min(R.shape.x, min(hs.x, hs.y));
  let q = pos - (R.rect.xy + hs);
  let d = rounded_distance(q, hs, radius, R.shape.z);
  let px = R.shape.w;
  let outer = coverage(d, px);
  var inner = outer;
  if (R.shape.y > 0.0) {
    inner = coverage(d + R.shape.y, px);
  }
  let f = vec4<f32>(R.fill.rgb * R.fill.a, R.fill.a) * inner;
  var ring = outer - inner;
  if ((R.edge.x > 0.0 || R.edge.y > 0.0) && ring > 0.0) {
    // the border follows the direction the edge faces: floor + (1 - floor) * |n.x|^sides * |n.y|^ends
    let e = 0.25 / px;
    let n = vec2<f32>(
      rounded_distance(q + vec2<f32>(e, 0.0), hs, radius, R.shape.z) - rounded_distance(q - vec2<f32>(e, 0.0), hs, radius, R.shape.z),
      rounded_distance(q + vec2<f32>(0.0, e), hs, radius, R.shape.z) - rounded_distance(q - vec2<f32>(0.0, e), hs, radius, R.shape.z));
    let len = length(n);
    var w = 0.0;
    if (len > 0.0) {
      let u = abs(n) / len;
      var sides = 1.0;
      if (R.edge.x > 0.0) {
        sides = pow(u.x, R.edge.x);
      }
      var ends = 1.0;
      if (R.edge.y > 0.0) {
        ends = pow(u.y, R.edge.y);
      }
      w = sides * ends;
    }
    ring = ring * (R.edge.z + (1.0 - R.edge.z) * w);
  }
  let b = vec4<f32>(R.border.rgb * R.border.a, R.border.a) * ring;
  return (f + b) * place.area.z;
}
