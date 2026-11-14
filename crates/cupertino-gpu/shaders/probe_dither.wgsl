// Test program: the shared dither at given pixels, one pixel of the vectors a pixel of a one-row target. Row i is
// rows[i] = (x, y, v, unused); the target pixel holds (offset, dithered v, 0, 1).

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
    let row = rows.data[i32(floor(frag.x))];
    let centre = row.xy + vec2<f32>(0.5);
    return vec4<f32>(dither_offset(centre), dither(row.z, centre), 0.0, 1.0);
}
