// One 2x box halving of the blur pyramid: each texel of the target the mean of the 2 x 2 source texels it covers,
// premultiplied, with the blur's edge rule past the source.

struct Level {
    // 0: clamp to the nearest texel (a backdrop); 1: transparent (a layer's own effects).
    edge: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(0) var<uniform> level: Level;
@group(0) @binding(1) var source: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(index);
}

fn read(t: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    let inside = t.x >= 0 && t.y >= 0 && t.x < size.x && t.y < size.y;
    if (!inside && level.edge == 1u) {
        return vec4<f32>(0.0);
    }
    return textureLoad(source, clamp(t, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(source, 0));
    let t = vec2<i32>(floor(frag.xy)) * 2;
    let sum = read(t, size) + read(t + vec2<i32>(1, 0), size) + read(t + vec2<i32>(0, 1), size)
        + read(t + vec2<i32>(1, 1), size);
    return sum * 0.25;
}
