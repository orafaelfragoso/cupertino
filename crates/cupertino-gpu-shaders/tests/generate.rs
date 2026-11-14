//! The committed shaders are a fresh generation of their sources, and the build step refuses what the backends
//! cannot share.

use cupertino_gpu_shaders::{differences, generate, generate_all, generated_dir, source_dir};

#[test]
fn the_committed_shaders_are_what_the_sources_generate() {
    let fresh = generate_all(&source_dir()).expect("every program generates");
    assert!(!fresh.is_empty());
    let problems = differences(&fresh, &generated_dir());
    assert!(
        problems.is_empty(),
        "run `cargo run -p cupertino-gpu-shaders` and commit the result:\n{}",
        problems.join("\n")
    );
}

const ENTRY_POINTS: &str = "
@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }
@fragment fn fs_main() -> @location(0) vec4<f32> { return probe(); }
";

#[test]
fn a_storage_buffer_or_a_2x2_matrix_in_a_block_is_refused() {
    let storage = format!(
        "@group(0) @binding(0) var<storage, read> data: array<vec4<f32>>;\nfn probe() -> vec4<f32> {{ return data[0]; }}\n{ENTRY_POINTS}"
    );
    let e = generate("storage", "", &storage).unwrap_err();
    assert!(e.contains("uniform blocks only"), "{e}");
    let matrix = format!(
        "struct M {{ m: mat2x2<f32>, }}\n@group(0) @binding(0) var<uniform> u: M;\nfn probe() -> vec4<f32> {{ return vec4<f32>(u.m[0], 0.0, 1.0); }}\n{ENTRY_POINTS}"
    );
    let e = generate("matrix", "", &matrix).unwrap_err();
    assert!(e.contains("u.m is a 2 x 2 matrix"), "{e}");
    let missing = "@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }";
    assert!(generate("missing", "", missing)
        .unwrap_err()
        .contains("fs_main"));
}

#[test]
fn glsl_is_es_3_00_without_coordinate_adjustment_and_blocks_and_samplers_are_named() {
    let program = "
struct U { color: vec4<f32>, }
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var t: texture_2d<f32>;
@group(0) @binding(2) var s: sampler;
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32(i), 0.5, 0.0, 1.0);
}
@fragment fn fs_main() -> @location(0) vec4<f32> { return u.color * textureSample(t, s, vec2<f32>(0.5)); }
";
    let g = generate("named", "", program).unwrap();
    assert!(g.vertex.starts_with("#version 300 es"), "{}", g.vertex);
    assert!(g.fragment.starts_with("#version 300 es"));
    // the adjustment would flip y and remap z of the position
    assert!(!g.vertex.contains("gl_Position.yz"), "{}", g.vertex);
    assert!(!g.vertex.contains("layout(binding"), "{}", g.vertex);
    let uniform = g
        .bindings
        .lines()
        .find(|l| l.starts_with("uniform 0 0 fragment "))
        .unwrap();
    let block = uniform.rsplit(' ').next().unwrap();
    assert!(
        g.fragment
            .contains(&format!("layout(std140) uniform {block} {{")),
        "{}",
        g.fragment
    );
    let texture = g
        .bindings
        .lines()
        .find(|l| l.starts_with("texture 0 1 0 2 fragment "))
        .unwrap();
    let sampler = texture.rsplit(' ').next().unwrap();
    assert!(
        g.fragment.contains(&format!("sampler2D {sampler};")),
        "{}",
        g.fragment
    );
    // SPIR-V: the magic number first
    assert_eq!(&g.spirv[..4], &0x0723_0203u32.to_le_bytes());
}
