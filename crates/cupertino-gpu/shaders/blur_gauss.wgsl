// One separable pass of the blur's Gaussian on the last level of the pyramid: the one-sided weights of the shared
// kernel (at most 36, four to a vector) along one axis, premultiplied, with the blur's edge rule.

struct Kernel {
    // w[0] the centre, w[i] the weight of the texels i away on either side.
    weights: array<vec4<f32>, 9>,
    // (1, 0) across, (0, 1) down.
    direction: vec2<i32>,
    radius: i32,
    // 0: clamp; 1: transparent.
    edge: u32,
}

@group(0) @binding(0) var<uniform> kernel: Kernel;
@group(0) @binding(1) var source: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(index);
}

fn read(t: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    let inside = t.x >= 0 && t.y >= 0 && t.x < size.x && t.y < size.y;
    if (!inside && kernel.edge == 1u) {
        return vec4<f32>(0.0);
    }
    return textureLoad(source, clamp(t, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

fn weight(i: i32) -> f32 {
    return kernel.weights[i / 4][i % 4];
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(source, 0));
    let t = vec2<i32>(floor(frag.xy));
    var acc = read(t, size) * weight(0);
    for (var i = 1; i <= kernel.radius; i = i + 1) {
        let o = kernel.direction * i;
        acc = acc + (read(t + o, size) + read(t - o, size)) * weight(i);
    }
    return acc;
}
