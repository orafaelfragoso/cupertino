//! The committed shaders are a fresh generation of their sources, and the build step refuses what the backends
//! cannot share.

use cupertino_gpu_shaders::{
    assembled, differences, generate, generate_set, resolve_includes, sets,
};

#[test]
fn the_committed_shaders_are_what_the_sources_generate() {
    let mut problems = Vec::new();
    for set in sets() {
        let fresh = generate_set(&set).expect("every program generates");
        assert!(!fresh.is_empty(), "{}", set.dir.display());
        for p in differences(&fresh, &set.generated) {
            problems.push(format!("{}: {p}", set.generated.display()));
        }
    }
    assert!(
        problems.is_empty(),
        "run `cargo run -p cupertino-gpu-shaders` and commit the result:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_compositor_s_rectangle_passes_are_a_set_each_placed_by_the_included_block() {
    let set = sets()
        .into_iter()
        .find(|s| s.dir.ends_with("cupertino-server/shaders/passes"))
        .expect("the compositor's rectangle passes are a set");
    assert!(!set.wgsl, "the compositor links GLSL only");
    let fresh = generate_set(&set).expect("every pass generates");
    for pass in ["rrect", "shadow", "wallpaper"] {
        let bindings = fresh
            .iter()
            .find(|(f, _)| *f == format!("{pass}.bindings"))
            .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_else(|| panic!("{pass} is generated"));
        // the placement is group 1 in both stages, the pass's own parameters group 0
        assert!(
            bindings.contains("uniform 1 0 vertex RectPlace_block"),
            "{pass}: {bindings}"
        );
        assert!(
            bindings.contains("uniform 1 0 fragment RectPlace_block"),
            "{pass}: {bindings}"
        );
        assert!(
            bindings.contains("uniform 0 0 fragment"),
            "{pass}: {bindings}"
        );
    }
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

#[test]
fn an_include_line_puts_its_file_in_place_and_the_assembled_wgsl_keeps_only_what_runs() {
    let files = |f: &str| match f {
        "common.wgsl" => Ok("fn used() -> f32 { return 0.5; }\n\
fn unused(a: array<vec4<f32>, 2>) -> f32 {\n    return a[0].x;\n}\n"
            .to_owned()),
        other => Err(format!("{other}: no such file")),
    };
    let program = "// include common.wgsl\n\
@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(used()); }\n\
@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }\n";
    let resolved = resolve_includes("p", program, &files).unwrap();
    assert!(resolved.contains("fn used()") && resolved.contains("fn unused("));
    assert!(!resolved.contains("// include "));
    let missing = resolve_includes("p", "// include missing.wgsl\n", &files).unwrap_err();
    assert!(missing.contains("missing.wgsl"), "{missing}");
    let shared = "fn shared_unused() -> f32 {\n    return 1.0;\n}\n";
    let wgsl = assembled("p", shared, &resolved).unwrap();
    assert!(wgsl.contains("fn used()"), "{wgsl}");
    assert!(
        wgsl.contains("fn vs_main(") && wgsl.contains("fn fs_main("),
        "{wgsl}"
    );
    assert!(
        !wgsl.contains("fn unused(") && !wgsl.contains("fn shared_unused("),
        "{wgsl}"
    );
}

#[test]
fn a_name_glsl_reserves_is_refused() {
    // the GLSL writer names the second local `gl` of a program `gl_1`, which GLSL reserves
    let program = "
@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }
fn half_length(v: vec2<f32>) -> f32 {
    let gl = length(v);
    return gl * 0.5;
}
@fragment fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let gl = length(p.xy);
    return vec4<f32>(gl + half_length(p.zw));
}
";
    let e = generate("reserved", "", program).unwrap_err();
    assert!(e.contains("reserves names starting with gl_"), "{e}");
    // the built-ins are not ours
    assert!(generate("fine", "", &program.replace("gl", "slope")).is_ok());
}
