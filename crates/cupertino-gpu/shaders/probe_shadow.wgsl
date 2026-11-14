// Test program: the shared drop and inner shadows at given points, one point a pixel of a one-row target. Row i is
// rows[3i] = (px, py, hx, hy), rows[3i + 1] = (radius, extent, exponent, sigma) and rows[3i + 2] = (ox, oy, density,
// pixels per unit); the pixel holds (drop, drop with the caster cut out, inner, 1).

struct Rows {
    data: array<vec4<f32>, 256>,
}

@group(0) @binding(0) var<uniform> rows: Rows;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(index);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let i = i32(floor(frag.x));
    let a = rows.data[3 * i];
    let b = rows.data[3 * i + 1];
    let c = rows.data[3 * i + 2];
    let caster = Caster(a.zw, b.x, b.y, b.z);
    let shadow = Shadow(b.w, c.xy, c.z);
    return vec4<f32>(
        drop_shadow(a.xy, caster, shadow, false, c.w),
        drop_shadow(a.xy, caster, shadow, true, c.w),
        inner_shadow(a.xy, caster, shadow, c.w),
        1.0,
    );
}
