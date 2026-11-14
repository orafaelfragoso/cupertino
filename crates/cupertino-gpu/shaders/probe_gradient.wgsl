// Test program: the shared gradient rule at given parameters, one parameter a pixel of a one-row target; the pixel
// holds the premultiplied colour.

struct Gradient {
    offsets: array<vec4<f32>, 2>,
    colors: array<vec4<f32>, 8>,
    count: i32,
    pad0: i32,
    pad1: i32,
    pad2: i32,
    // The parameter of pixel i at ts[i / 4][i % 4].
    ts: array<vec4<f32>, 64>,
}

@group(0) @binding(0) var<uniform> gradient: Gradient;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(index);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let i = i32(floor(frag.x));
    let t = gradient.ts[i / 4][i % 4];
    return gradient_at(gradient.offsets, gradient.colors, gradient.count, t);
}
