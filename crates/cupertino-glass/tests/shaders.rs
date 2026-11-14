//! Shader validation: every generated program's WGSL validates with naga, and its uniform blocks hold the bytes the
//! Rust structs write, in the WGSL and in the GLSL generated from it; the compositor and the headless renderer bind
//! them by the names generated beside them.

use naga::valid::{Capabilities, ValidationFlags, Validator};

fn validate_wgsl(name: &str, src: &str, entries: &[&str]) {
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
    Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    for want in entries {
        assert!(
            module.entry_points.iter().any(|e| e.name == *want),
            "{name}: missing entry point {want}"
        );
    }
}

#[test]
fn wgsl_shaders_validate() {
    for program in cupertino_glass::shaders::ALL {
        validate_wgsl(program.source.name, program.wgsl, &["vs_main", "fs_main"]);
    }
}

/// Members of a uniform block in order, each with its number of vec4 slots.
fn block_members(src: &str, open: &str, wgsl: bool) -> Vec<(String, usize)> {
    let start = src.find(open).unwrap_or_else(|| panic!("no block {open}"));
    let body = &src[start + open.len()..];
    let body = &body[..body.find('}').unwrap()];
    body.lines()
        .map(|l| l.split("//").next().unwrap().trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let count = |s: &str| {
                s.split(['[', ','])
                    .nth(1)
                    .and_then(|n| n.trim_matches(|c: char| !c.is_ascii_digit()).parse().ok())
                    .unwrap_or(1)
            };
            if wgsl {
                let (name, ty) = l.trim_end_matches(',').split_once(':').unwrap();
                let n = if ty.contains("array") { count(ty) } else { 1 };
                (name.trim().to_string(), n)
            } else {
                let decl = l.trim_end_matches(';').trim_start_matches("vec4 ");
                let name = decl.split('[').next().unwrap().trim().to_string();
                (name, if decl.contains('[') { count(decl) } else { 1 })
            }
        })
        .collect()
}

/// The vec4 slots of `members`, an `Op` two of them.
fn slots(members: &[(String, usize)]) -> usize {
    members
        .iter()
        .map(|(m, n)| if m == "ops" { 2 * n } else { *n })
        .sum()
}

#[test]
fn the_region_blocks_hold_the_rust_uniforms_in_the_wgsl_and_the_generated_glsl() {
    use cupertino_glass::glass::GlassUniform;
    use cupertino_glass::ops::{Placement, RegionParams, VibrantUniform};
    use cupertino_glass::shaders::{GLASS, MATERIAL, VIBRANT};
    for (program, block, size) in [
        (MATERIAL, "Region", std::mem::size_of::<RegionParams>()),
        (GLASS, "Glass", std::mem::size_of::<GlassUniform>()),
        (VIBRANT, "Vibrant", std::mem::size_of::<VibrantUniform>()),
    ] {
        let name = program.source.name;
        let open = format!("struct {block} {{");
        let w = block_members(program.wgsl, &open, true);
        assert_eq!(slots(&w) * 16, size, "{name}");
        // the GLSL generated from it declares the same members, each as large (a name ending in a digit takes an
        // underscore)
        let g: Vec<(String, usize)> = block_members(program.source.fragment_glsl, &open, false)
            .into_iter()
            .map(|(m, n)| (m.trim_end_matches('_').to_owned(), n))
            .collect();
        assert_eq!(w, g, "{name}");
    }
    // the placement: a column-major 3 x 3 projection (three vec4 columns in std140), the quad and the flags
    for program in [MATERIAL, GLASS] {
        let names: Vec<String> = block_members(program.wgsl, "struct Placement {", true)
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        assert_eq!(names, ["proj", "quad", "flags"], "{}", program.source.name);
        assert!(program.wgsl.contains("proj: mat3x3<f32>,"));
    }
    assert_eq!(std::mem::size_of::<Placement>(), 3 * 16 + 16 + 16);
    // the mesh block: the same projection and the opacity
    let mesh = cupertino_glass::shaders::MESH;
    let names: Vec<String> = block_members(mesh.wgsl, "struct Mesh {", true)
        .into_iter()
        .map(|(m, _)| m)
        .collect();
    assert_eq!(names, ["proj", "alpha"]);
    assert!(mesh.wgsl.contains("proj: mat3x3<f32>,") && mesh.wgsl.contains("alpha: vec4<f32>,"));
    assert!(mesh
        .source
        .vertex_glsl
        .contains("mat3x3 proj;\n    vec4 alpha;"));
    assert_eq!(
        std::mem::size_of::<cupertino_glass::ops::MeshUniform>(),
        3 * 16 + 16
    );
}

#[test]
fn the_material_and_glass_programs_dither_only_by_the_placement_flag() {
    use cupertino_glass::shaders::{GLASS, MATERIAL, PLACEMENT, VIBRANT};
    for program in [MATERIAL, GLASS] {
        let name = program.source.name;
        assert!(program.wgsl.contains("if (place.flags.x != 0) {"), "{name}");
        // every return of the fragment stage goes through the placed result
        let fragment = program.wgsl.split("fn fs_main(").nth(1).unwrap();
        let returns = fragment.matches("return ").count();
        assert!(returns > 0, "{name}");
        assert_eq!(
            returns,
            fragment.matches("return placed(").count(),
            "{name}"
        );
        // the vertex and fragment stages each read the placement, under a name of their own
        assert_eq!(program.blocks(PLACEMENT.0, PLACEMENT.1).len(), 2, "{name}");
    }
    assert!(!VIBRANT.source.fragment_glsl.contains("dither_rgb(_"));
}

#[test]
fn the_generated_passes_bind_their_block_and_source_by_the_names_beside_them() {
    use cupertino_glass::shaders::{PASSES, PASS_BLOCK, PASS_SOURCE};
    for program in PASSES {
        let name = program.source.name;
        assert!(
            program.source.vertex_glsl.starts_with("#version 300 es"),
            "{name}"
        );
        assert!(
            program.source.fragment_glsl.starts_with("#version 300 es"),
            "{name}"
        );
        let block = program
            .block(PASS_BLOCK.0, PASS_BLOCK.1)
            .unwrap_or_else(|| panic!("{name}: no block"));
        assert!(
            program
                .source
                .fragment_glsl
                .contains(&format!("layout(std140) uniform {block} {{ PassParams ")),
            "{name}: {block}"
        );
        let sampler = program
            .sampler(PASS_SOURCE.0, PASS_SOURCE.1)
            .unwrap_or_else(|| panic!("{name}: no sampler"));
        assert!(
            program
                .source
                .fragment_glsl
                .contains(&format!("sampler2D {sampler};")),
            "{name}"
        );
        // the CPU writes the same bytes for every backend: the block's vec4 slots are the Rust struct's
        let w = block_members(program.wgsl, "struct PassParams {", true);
        let names: Vec<&str> = w.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(
            names,
            ["sizes", "dir_radius", "weights", "ops", "flags"],
            "{name}"
        );
        // each entry of ops is an Op of two vec4s
        let slots: usize = w
            .iter()
            .map(|(m, n)| if m == "ops" { 2 * n } else { *n })
            .sum();
        assert_eq!(
            slots * 16,
            std::mem::size_of::<cupertino_glass::ops::PassParams>(),
            "{name}"
        );
    }
    // only the blur dithers, and only when its flag says so
    let blur = cupertino_glass::shaders::BLUR.source.fragment_glsl;
    assert!(blur.contains("dither_rgb("));
    for other in [
        cupertino_glass::shaders::DOWNSAMPLE,
        cupertino_glass::shaders::COLOR,
        cupertino_glass::shaders::BLIT,
    ] {
        assert!(
            !other.source.fragment_glsl.contains("dither_rgb(_"),
            "{}",
            other.source.name
        );
    }
}
