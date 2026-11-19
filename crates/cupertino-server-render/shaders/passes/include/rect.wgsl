// Where a compositor rectangle (shadow, rounded rectangle, wallpaper) draws: one quad per damage rectangle of its
// element, in output pixels, mapped through the frame's projection so the output's transform is honoured. Drawn as a
// 4-vertex triangle strip into the framebuffer the compositor has bound; `coords` runs from 0 to 1 across the
// element, so a program finds its point by `coords * place.area.xy`.

// Uniform block of the placement (std140, mirrored by `RectPlace` in the server's render::passes).
//   proj = the frame's projection from output pixels to clip space, column-major
//   dest = x, y, w, h of the element in output pixels
//   area = the element's width and height in points, its opacity, and 1 when the result is dithered by its pixel
struct RectPlace {
  proj: mat3x3<f32>,
  dest: vec4<f32>,
  area: vec4<f32>,
};

@group(1) @binding(0) var<uniform> place: RectPlace;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  // 0 to 1 across the element
  @location(0) coords: vec2<f32>,
};

// One damage rectangle (x, y, w, h in pixels from the element's origin) an instance; the strip's corners in the order
// top right, top left, bottom right, bottom left.
@vertex
fn vs_main(@builtin(vertex_index) i: u32, @location(0) damage: vec4<f32>) -> VsOut {
  let corner = vec2<f32>(1.0 - f32(i & 1u), f32(i >> 1u));
  let local = corner * damage.zw + damage.xy;
  let p = place.proj * vec3<f32>(local + place.dest.xy, 1.0);
  var o: VsOut;
  o.pos = vec4<f32>(p.xy, 0.0, 1.0);
  o.coords = local / place.dest.zw;
  return o;
}
