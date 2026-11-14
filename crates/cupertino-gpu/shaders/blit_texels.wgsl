// A texture copied over the target texel for texel: each target pixel takes the texel at its own position, so a
// texture smaller than the target lands on its first rows and columns unscaled. Pixels past the texture take its last
// row or column (whatever crops the target to the texture's size never shows them).

@group(0) @binding(1) var source: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return fullscreen_position(index);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(source, 0));
    let t = clamp(vec2<i32>(floor(frag.xy)), vec2<i32>(0), size - vec2<i32>(1));
    return textureLoad(source, t, 0);
}
