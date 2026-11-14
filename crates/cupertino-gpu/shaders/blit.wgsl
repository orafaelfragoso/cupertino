// A texture drawn over the whole target through its sampler: v = 0 is the texture's first row, drawn on the
// target's first row.

@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Varyings {
    var out: Varyings;
    out.position = fullscreen_position(index);
    out.uv = (out.position.xy + vec2<f32>(1.0)) * 0.5;
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    return textureSample(source, source_sampler, in.uv);
}
