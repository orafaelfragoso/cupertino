//! The build step of the shader programs: the committed files are what it generates, a program outside the rules is
//! refused, a program's blocks and textures are named in its bindings, includes are put in place and unreached
//! functions left out, and `write` and `differences` keep a generated directory in step with its programs.

use cupertino_gpu_shaders::{assembled, differences, generate, resolve_includes, write};
use std::path::PathBuf;
use std::process::Command;

/// A program with a uniform block, a texture and its sampler, and both entry points.
const PROGRAM: &str = "struct Paint { color: vec4<f32> }
@group(0) @binding(0) var<uniform> paint: Paint;
@group(1) @binding(0) var picture: texture_2d<f32>;
@group(1) @binding(1) var picture_sampler: sampler;
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32(i), 0.0, 0.0, 1.0);
}
@fragment fn fs_main() -> @location(0) vec4<f32> {
    return paint.color * textureSample(picture, picture_sampler, vec2<f32>(0.5, 0.5));
}
";

const ENTRY_POINTS: &str =
    "@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }
@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
";

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_committed_files_of_every_set_are_what_the_build_step_generates() {
    let output = Command::new(env!("CARGO_BIN_EXE_cupertino-gpu-shaders"))
        .arg("--check")
        .output()
        .expect("the build step runs");
    assert!(
        output.status.success(),
        "cupertino-gpu-shaders --check: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_program_outside_the_rules_is_refused() {
    // (case, program, what the error names)
    let cases = [
        (
            "a storage buffer",
            format!("@group(0) @binding(0) var<storage, read> values: array<f32>;\n{ENTRY_POINTS}"),
            "values is in Storage",
        ),
        (
            "a 2 x 2 matrix in a block",
            format!(
                "struct Turn {{ m: mat2x2<f32> }}\n@group(0) @binding(0) var<uniform> turn: Turn;\n{ENTRY_POINTS}"
            ),
            "turn.m is a 2 x 2 matrix in a uniform block",
        ),
        (
            "no fragment entry point",
            "@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }".to_owned(),
            "no Fragment entry point fs_main",
        ),
        ("WGSL that does not parse", "fn".to_owned(), "p: "),
    ];
    for (case, program, named) in cases {
        let error = generate("p", "", &program).expect_err(case);
        assert!(error.contains(named), "{case}: {error}");
    }
}

#[test]
fn a_program_s_blocks_and_textures_are_std140_and_named_in_its_bindings() {
    let generated = generate("paint", "", PROGRAM).expect("the program generates");
    assert!(
        generated.fragment.contains("layout(std140) uniform"),
        "{}",
        generated.fragment
    );
    let lines: Vec<&str> = generated
        .bindings
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect();
    assert_eq!(lines.len(), 2, "{}", generated.bindings);
    assert!(
        lines.iter().any(|l| l.starts_with("uniform 0 0 fragment ")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("texture 1 0 1 1 fragment ")),
        "{lines:?}"
    );
    assert_eq!(
        &generated.spirv[..4],
        &0x0723_0203_u32.to_le_bytes(),
        "SPIR-V's magic number"
    );
}

#[test]
fn includes_are_put_in_place_and_the_assembled_program_holds_only_the_functions_it_runs() {
    let read = |file: &str| {
        match file {
        "colour.wgsl" => Ok("fn colour() -> vec4<f32> { return vec4<f32>(1.0); }\nfn unused() -> f32 {\n    return 0.0;\n}".to_owned()),
        other => Err(format!("{other}: no such file")),
    }
    };
    let program = "// include colour.wgsl\n@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }\n@fragment fn fs_main() -> @location(0) vec4<f32> { return colour(); }\n";
    let resolved = resolve_includes("p", program, &read).expect("the include is read");
    assert!(
        resolved.contains("fn colour()") && !resolved.contains("// include"),
        "{resolved}"
    );
    let text = assembled("p", "fn shared_unused() -> f32 { return 1.0; }", &resolved)
        .expect("it assembles");
    assert!(text.contains("fn colour()"), "{text}");
    assert!(
        !text.contains("fn unused") && !text.contains("fn shared_unused"),
        "{text}"
    );
    let error =
        resolve_includes("p", "// include missing.wgsl\n", &read).expect_err("a missing include");
    assert!(error.contains("missing.wgsl"), "{error}");
}

#[test]
fn write_makes_a_directory_match_its_programs_and_differences_names_what_does_not() {
    let dir = scratch("generated");
    let fresh = vec![
        ("a.spv".to_owned(), b"a".to_vec()),
        ("b.bindings".to_owned(), b"b".to_vec()),
    ];
    std::fs::write(dir.join("stale.spv"), b"old").unwrap();
    assert_eq!(
        differences(&fresh, &dir),
        [
            "a.spv is missing",
            "b.bindings is missing",
            "stale.spv is not generated from any program"
        ]
    );
    write(&fresh, &dir).expect("it writes");
    assert!(differences(&fresh, &dir).is_empty());
    std::fs::write(dir.join("a.spv"), b"changed").unwrap();
    assert_eq!(
        differences(&fresh, &dir),
        ["a.spv differs from a fresh generation"]
    );
}
