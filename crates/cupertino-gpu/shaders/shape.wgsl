// Filled rectangles and rounded rectangles, one instance each: coverage from the shared distance, a premultiplied
// colour. Instance attributes: the rectangle (x, y, width, height in target pixels from the first row and column),
// the colour, and the corner (extent in pixels, exponent).

struct Viewport {
    // The target's size in pixels.
    size: vec2<f32>,
    pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) corner: vec2<f32>,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) corner: vec2<f32>,
}

// Pixels the quad reaches past the rectangle, for the anti-aliased edge.
const EDGE_MARGIN: f32 = 1.0;

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> Varyings {
    let uv = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let origin = instance.rect.xy - vec2<f32>(EDGE_MARGIN);
    let extent = instance.rect.zw + vec2<f32>(2.0 * EDGE_MARGIN);
    let pixel = origin + uv * extent;
    var out: Varyings;
    out.position = vec4<f32>(pixel / viewport.size * 2.0 - 1.0, 0.0, 1.0);
    out.half_size = instance.rect.zw * 0.5;
    out.local = pixel - (instance.rect.xy + out.half_size);
    out.color = instance.color;
    out.corner = instance.corner;
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let extent = min(in.corner.x, min(in.half_size.x, in.half_size.y));
    let d = rounded_distance(in.local, in.half_size, extent, in.corner.y);
    return in.color * coverage(d, 1.0);
}
