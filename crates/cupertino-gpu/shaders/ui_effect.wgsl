// The last step of a layer's blur or shadow: the last level of the blur pyramid read back at every pixel of the layer
// (moved by the shadow's offset, linearly between the level's texels, transparent past its edge), then stored as the
// CPU renderer stores it. A blur keeps the blurred colour, dithered by the window pixel; a shadow keeps one coverage
// byte, dithered, and lays its colour under the layer's content (drop) or over it, inside it (inner), in 8-bit
// steps.

struct Resolve {
    // 0 blur, 1 drop shadow, 2 inner shadow; the level's texels per layer pixel; the offset in layer pixels.
    info: vec4<f32>,
    // Whether the stores are dithered.
    flags: vec4<f32>,
    // The window pixel of the layer's first pixel.
    origin: vec4<f32>,
    // The shadow's colour, premultiplied, in bytes.
    tint: vec4<f32>,
}

@group(0) @binding(0) var<uniform> resolve: Resolve;
@group(0) @binding(1) var level: texture_2d<f32>;
@group(0) @binding(2) var content: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(index);
}

fn div255(v: u32) -> u32 {
    let w = v + 128u;
    return (w + (w >> 8u)) >> 8u;
}

fn quantize(v: f32) -> u32 {
    let x = clamp(v, 0.0, 1.0) * 255.0;
    let whole = floor(x);
    return u32(whole) + select(0u, 1u, x - whole >= 0.5);
}

fn tap(t: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    if (t.x < 0 || t.y < 0 || t.x >= size.x || t.y >= size.y) {
        return vec4<f32>(0.0);
    }
    return textureLoad(level, t, 0);
}

// The level at layer pixel `x`: linearly between its texels, across then down.
fn blurred(x: vec2<f32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(level, 0));
    let factor = resolve.info.y;
    let u = (x + vec2<f32>(0.5) - resolve.info.zw) / factor - vec2<f32>(0.5);
    let i = floor(u);
    let f = u - i;
    let t = vec2<i32>(i);
    let top = tap(t, size) * (1.0 - f.x) + tap(t + vec2<i32>(1, 0), size) * f.x;
    let bottom = tap(t + vec2<i32>(0, 1), size) * (1.0 - f.x) + tap(t + vec2<i32>(1, 1), size) * f.x;
    return top * (1.0 - f.y) + bottom * f.y;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let x = floor(frag.xy);
    let window = x + resolve.origin.xy + vec2<f32>(0.5);
    var step = 0.0;
    if (resolve.flags.x > 0.5) {
        step = dither_offset(window) / 255.0;
    }
    let v = blurred(x);
    let mode = i32(resolve.info.x);
    if (mode == 0) {
        let alpha = quantize(v.a);
        let c = vec3<u32>(
            min(quantize(v.r + step), alpha),
            min(quantize(v.g + step), alpha),
            min(quantize(v.b + step), alpha),
        );
        return vec4<f32>(vec4<u32>(c, alpha)) / 255.0;
    }
    let c = vec4<u32>(textureLoad(content, vec2<i32>(x), 0) * 255.0 + vec4<f32>(0.5));
    let tint = vec4<u32>(resolve.tint);
    if (mode == 1) {
        let a = quantize(v.a + step);
        if (a == 0u || c.w == 255u) {
            return vec4<f32>(c) / 255.0;
        }
        let keep = 255u - c.w;
        var out = c;
        for (var i = 0; i < 4; i = i + 1) {
            out[i] = min(c[i] + div255(div255(tint[i] * a) * keep), 255u);
        }
        return vec4<f32>(out) / 255.0;
    }
    let a = div255(quantize(clamp(1.0 - v.a, 0.0, 1.0) + step) * c.w);
    var s = vec4<u32>(0u);
    for (var i = 0; i < 4; i = i + 1) {
        s[i] = div255(tint[i] * a);
    }
    let keep = 255u - s.w;
    var out = c;
    for (var i = 0; i < 4; i = i + 1) {
        out[i] = min(s[i] + div255(c[i] * keep), 255u);
    }
    return vec4<f32>(out) / 255.0;
}
