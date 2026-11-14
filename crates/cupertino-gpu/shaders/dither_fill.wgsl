// One grey level over the whole target, dithered by the position of the pixel it is stored in, opaque.

struct Fill {
    value: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
}

@group(0) @binding(0) var<uniform> fill: Fill;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(index);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(dither_rgb(vec3<f32>(fill.value), frag.xy), 1.0);
}
