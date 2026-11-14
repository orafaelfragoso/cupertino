// Test program: the shared distance and coverage at given points, one point a pixel of a one-row target. Row i is
// rows[2i] = (px, py, hx, hy) and rows[2i + 1] = (extent, exponent, pixels per unit, unused); the pixel holds
// (distance, coverage, 0, 1).

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
    let a = rows.data[2 * i];
    let b = rows.data[2 * i + 1];
    let d = rounded_distance(a.xy, a.zw, b.x, b.y);
    return vec4<f32>(d, coverage(d, b.z), 0.0, 1.0);
}
