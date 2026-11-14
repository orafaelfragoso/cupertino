//! Shader validation with naga: the WGSL sources, the generated programs and the GLSL ES 3.00 ports not yet
//! generated.
//!
//! naga's GLSL front end only accepts the Vulkan dialect (`#version 450`, explicit locations,
//! bindings and separate texture/sampler objects), so the ES 3.00 sources are mechanically rewritten before parsing (`to_naga_dialect`).
//! The rewrite changes nothing but the version line, `gl_VertexID`, the `precision` statements
//! (naga does not implement them), and adds location/binding qualifiers and splits each into a texture and a sampler; it proves that the ported sources parse, type-check and validate.

use naga::front::glsl::{Frontend, Options};
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::ShaderStage;

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
    validate_wgsl(
        "material.wgsl",
        cupertino_glass::renderer::MATERIAL_WGSL,
        &["vs_region", "fs_material"],
    );
    validate_wgsl(
        "vibrant.wgsl",
        cupertino_glass::renderer::VIBRANT_WGSL,
        &["vs_vibrant", "fs_vibrant"],
    );
    validate_wgsl(
        "glass.wgsl",
        cupertino_glass::renderer::GLASS_WGSL,
        &["vs_glass", "fs_glass"],
    );
}

fn to_naga_dialect(src: &str, fragment: bool) -> String {
    let (mut next_in, mut next_out, mut next_binding) = (0, 0, 0);
    let mut samplers: Vec<String> = Vec::new();
    let mut out = String::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("#version") {
            out.push_str("#version 450 core\n");
            continue;
        }
        if t.starts_with("precision ") {
            continue;
        }
        let mut l = line.replace("gl_VertexID", "gl_VertexIndex");
        if t.starts_with("layout(std140) uniform") {
            l = l.replacen(
                "layout(std140)",
                &format!("layout(std140, set = 0, binding = {next_binding})"),
                1,
            );
            next_binding += 1;
        } else if let Some(rest) = t.strip_prefix("uniform sampler2D ") {
            let name = rest.trim_end_matches(';').trim();
            samplers.push(name.to_string());
            l = format!("layout(set = 0, binding = {next_binding}) uniform texture2D {name}_tex;\nlayout(set = 0, binding = {}) uniform sampler {name}_smp;", next_binding + 1);
            next_binding += 2;
        } else if let Some(rest) = t
            .strip_prefix("uniform ")
            .filter(|r| !r.starts_with("sampler2D") && r.ends_with(";"))
        {
            // a plain uniform: Vulkan GLSL only takes them in blocks
            let name = rest
                .trim_end_matches(";")
                .split_whitespace()
                .last()
                .unwrap_or("u");
            l = format!("layout(std140, set = 0, binding = {next_binding}) uniform {name}_block {{ {rest} }};");
            next_binding += 1;
        } else if t.starts_with("in ") && fragment {
            l = format!("layout(location = {next_in}) {t}");
            next_in += 1;
        } else if t.starts_with("out ") && !fragment {
            l = format!("layout(location = {next_out}) {t}");
            next_out += 1;
        }
        if !t.starts_with("uniform sampler2D ") {
            for name in &samplers {
                l = replace_word(&l, name, &format!("sampler2D({name}_tex, {name}_smp)"));
            }
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

fn replace_word(line: &str, word: &str, with: &str) -> String {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find(word) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + word.len()..].chars().next();
        out.push_str(&rest[..i]);
        if before.is_none_or(|c| !is_ident(c)) && after.is_none_or(|c| !is_ident(c)) {
            out.push_str(with);
        } else {
            out.push_str(word);
        }
        rest = &rest[i + word.len()..];
    }
    out.push_str(rest);
    out
}

fn validate_glsl(name: &str, src: &str, fragment: bool) {
    assert!(
        src.starts_with("#version 300 es"),
        "{name}: not GLSL ES 3.00"
    );
    assert!(
        !src.contains("layout(binding"),
        "{name}: ES 3.00 has no binding qualifiers"
    );
    let shim = to_naga_dialect(src, fragment);
    let stage = if fragment {
        ShaderStage::Fragment
    } else {
        ShaderStage::Vertex
    };
    let module = Frontend::default()
        .parse(&Options::from(stage), &shim)
        .unwrap_or_else(|e| panic!("{name}: {e:?}\n{shim}"));
    Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert!(
        module
            .entry_points
            .iter()
            .any(|e| e.name == "main" && e.stage == stage),
        "{name}: no main entry point"
    );
}

#[test]
fn gles_shaders_parse_and_validate() {
    for (name, fragment, src) in cupertino_glass::gles::ALL {
        validate_glsl(name, src, fragment);
    }
}

#[test]
fn gles_files_on_disk_match_embedded_sources() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders/gles");
    for (name, _, src) in cupertino_glass::gles::ALL {
        let disk = std::fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(disk, src, "{name}");
    }
    let count = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(
        count,
        cupertino_glass::gles::ALL.len(),
        "every file in shaders/gles is registered"
    );
}

#[test]
fn gles_sources_use_only_es300_features() {
    for (name, _, src) in cupertino_glass::gles::ALL {
        for banned in [
            "imageLoad",
            "std430",
            "#version 4",
            "gl_VertexIndex",
            "texture2D(",
            "textureSample",
        ] {
            assert!(!src.contains(banned), "{name} contains {banned}");
        }
        assert!(
            !src.lines()
                .any(|l| l.trim_start().starts_with("buffer ") || l.contains("layout(local_size")),
            "{name} uses compute-only features"
        );
    }
}

#[test]
fn gles_and_wgsl_agree_on_uniform_layout() {
    // The same Rust structs are uploaded for both targets: the member counts must match.
    for (wgsl, glsl, names) in [
        (
            cupertino_glass::renderer::MATERIAL_WGSL,
            cupertino_glass::gles::MATERIAL_FRAG,
            ["rect", "shape", "info", "sizes", "tint", "fills"],
        ),
        (
            cupertino_glass::renderer::VIBRANT_WGSL,
            cupertino_glass::gles::VIBRANT_FRAG,
            ["rect", "uv", "sizes", "mode", "c0", "c1"],
        ),
    ] {
        let wb: Vec<&str> = wgsl
            .lines()
            .filter(|l| {
                names
                    .iter()
                    .any(|n| l.trim_start().starts_with(&format!("{n}:")))
            })
            .collect();
        let gb: Vec<&str> = glsl
            .lines()
            .filter(|l| {
                names
                    .iter()
                    .any(|n| l.trim_start().starts_with(&format!("vec4 {n}")))
            })
            .collect();
        assert_eq!(wb.len(), names.len());
        assert_eq!(gb.len(), names.len());
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

#[test]
fn glass_block_matches_the_rust_uniform_in_both_languages() {
    let w = block_members(
        cupertino_glass::renderer::GLASS_WGSL,
        "struct Glass {",
        true,
    );
    let g = block_members(cupertino_glass::gles::GLASS_FRAG, "uniform Glass {", false);
    assert_eq!(w, g);
    let slots: usize = w.iter().map(|(_, n)| n).sum();
    assert_eq!(
        slots * 16,
        std::mem::size_of::<cupertino_glass::glass::GlassUniform>()
    );
}

#[test]
fn the_generated_passes_bind_their_block_and_source_by_the_names_beside_them() {
    use cupertino_glass::shaders::{ALL, PASS_BLOCK, PASS_SOURCE};
    for program in ALL {
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
