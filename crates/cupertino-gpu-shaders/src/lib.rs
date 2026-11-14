//! The build step of Cupertino's own shader programs. Each program of a set ([`sets`]: `crates/cupertino-gpu/shaders/`
//! and `crates/cupertino-glass/shaders/`) is one WGSL file with a `vs_main` and an `fs_main` entry point, whose
//! `// include NAME.wgsl` lines put the set's include files in their place; the shared module (`shared.wgsl` of
//! `cupertino-gpu`, the drawing maths) is concatenated ahead of it, and `naga` generates, into the set's `generated/`:
//!
//! * `NAME.vert.glsl` and `NAME.frag.glsl`: GLSL ES 3.00, coordinate-space adjustment off (clip y = -1 is a target's
//!   first row on every backend);
//! * `NAME.spv`: SPIR-V 1.0 with both entry points, coordinate-space adjustment off;
//! * `NAME.bindings`: the GLSL name of every uniform block and sampler, by WGSL group and binding, since GLSL ES 3.00
//!   has no binding qualifiers: the GLES backend binds blocks and samplers by these names;
//! * `NAME.wgsl`, for a set that asks for it: the assembled WGSL without the functions the entry points never reach,
//!   for a backend that compiles WGSL itself.
//!
//! A program may hold uniform blocks and textures with their samplers only (no storage, push constants or
//! workgroup memory), and no 2 x 2 matrix anywhere inside a uniform block, whose layout differs between backends.
//! The generated files are committed: no product build runs `naga`. The crate's tests regenerate them and fail on a
//! difference; `cargo run -p cupertino-gpu-shaders` writes them again.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use naga::back::{glsl, spv};
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{AddressSpace, Module, ShaderStage, TypeInner, VectorSize};

/// The shared module's file name in the source directory.
pub const SHARED: &str = "shared.wgsl";

/// The directory the programs' sources are in.
pub fn source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../cupertino-gpu/shaders")
}

/// The directory the generated files go in.
pub fn generated_dir() -> PathBuf {
    source_dir().join("generated")
}

/// One program's generated files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generated {
    pub vertex: String,
    pub fragment: String,
    pub spirv: Vec<u8>,
    pub bindings: String,
}

/// The names of the programs in `dir`: every `.wgsl` file but the shared module, sorted.
pub fn programs(dir: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(stem) = name.strip_suffix(".wgsl") {
            if name != SHARED {
                names.push(stem.to_owned());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Parse and validate `shared` followed by `program`.
pub fn module(
    name: &str,
    shared: &str,
    program: &str,
) -> Result<(Module, naga::valid::ModuleInfo), String> {
    let source = format!("{shared}\n// ---- {name}.wgsl ----\n{program}");
    let module = naga::front::wgsl::parse_str(&source)
        .map_err(|e| format!("{name}: {}", e.emit_to_string(&source)))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .map_err(|e| format!("{name}: {}", e.emit_to_string(&source)))?;
    check(name, &module)?;
    Ok((module, info))
}

/// The rules every program keeps: uniform blocks and textures with samplers only, no 2 x 2 matrix in a block, and
/// a vertex and a fragment entry point.
pub fn check(name: &str, module: &Module) -> Result<(), String> {
    for (_, var) in module.global_variables.iter() {
        let label = var.name.as_deref().unwrap_or("?");
        match var.space {
            AddressSpace::Uniform => {
                if let Some(path) = matrix_2x2(module, var.ty, label) {
                    return Err(format!(
                        "{name}: {path} is a 2 x 2 matrix in a uniform block"
                    ));
                }
            }
            AddressSpace::Handle | AddressSpace::Private | AddressSpace::Function => {}
            other => {
                return Err(format!(
                    "{name}: {label} is in {other:?}; programs use uniform blocks only"
                ))
            }
        }
    }
    for (stage, entry) in [
        (ShaderStage::Vertex, "vs_main"),
        (ShaderStage::Fragment, "fs_main"),
    ] {
        if !module
            .entry_points
            .iter()
            .any(|e| e.stage == stage && e.name == entry)
        {
            return Err(format!("{name}: no {stage:?} entry point {entry}"));
        }
    }
    Ok(())
}

/// The path of a 2 x 2 matrix inside the type `ty`, if one is there.
fn matrix_2x2(module: &Module, ty: naga::Handle<naga::Type>, path: &str) -> Option<String> {
    match &module.types[ty].inner {
        TypeInner::Matrix {
            columns: VectorSize::Bi,
            rows: VectorSize::Bi,
            ..
        } => Some(path.to_owned()),
        TypeInner::Struct { members, .. } => members.iter().find_map(|m| {
            matrix_2x2(
                module,
                m.ty,
                &format!("{path}.{}", m.name.as_deref().unwrap_or("?")),
            )
        }),
        TypeInner::Array { base, .. } => matrix_2x2(module, *base, &format!("{path}[]")),
        _ => None,
    }
}

fn glsl_stage(
    module: &Module,
    info: &naga::valid::ModuleInfo,
    stage: ShaderStage,
    entry: &str,
) -> Result<(String, glsl::ReflectionInfo), String> {
    let options = glsl::Options {
        version: glsl::Version::Embedded {
            version: 300,
            is_webgl: false,
        },
        // no coordinate-space adjustment: clip y = -1 is the first row on every backend
        writer_flags: glsl::WriterFlags::empty(),
        binding_map: Default::default(),
        zero_initialize_workgroup_memory: false,
    };
    let pipeline = glsl::PipelineOptions {
        shader_stage: stage,
        entry_point: entry.to_owned(),
        multiview: None,
    };
    let mut out = String::new();
    let mut writer = glsl::Writer::new(
        &mut out,
        module,
        info,
        &options,
        &pipeline,
        naga::proc::BoundsCheckPolicies::default(),
    )
    .map_err(|e| format!("{entry}: {e}"))?;
    let reflection = writer.write().map_err(|e| format!("{entry}: {e}"))?;
    Ok((out, reflection))
}

/// Generate one program from the shared module's text and its own.
pub fn generate(name: &str, shared: &str, program: &str) -> Result<Generated, String> {
    let (module, info) = module(name, shared, program)?;
    let mut lines = BTreeSet::new();
    let mut stages = Vec::new();
    for (stage, entry, label) in [
        (ShaderStage::Vertex, "vs_main", "vertex"),
        (ShaderStage::Fragment, "fs_main", "fragment"),
    ] {
        let (mut text, reflection) =
            glsl_stage(&module, &info, stage, entry).map_err(|e| format!("{name}: {e}"))?;
        if let Some(reserved) = reserved_name(&text) {
            return Err(format!(
                "{name}: {label} GLSL names {reserved}; GLSL reserves names starting with gl_ (rename the WGSL `gl`)"
            ));
        }
        for (handle, block) in &reflection.uniforms {
            // GLSL ES 3.00 blocks default to the shared layout, which the driver chooses; every backend lays
            // blocks out as std140
            let declaration = format!("\nuniform {block} {{");
            if !text.contains(&declaration) {
                return Err(format!("{name}: no declaration of the block {block}"));
            }
            text = text.replace(
                &declaration,
                &format!("\nlayout(std140) uniform {block} {{"),
            );
            let binding = module.global_variables[*handle]
                .binding
                .as_ref()
                .ok_or_else(|| format!("{name}: a uniform without a binding"))?;
            lines.insert(format!(
                "uniform {} {} {label} {block}",
                binding.group, binding.binding
            ));
        }
        for (sampler_name, mapping) in &reflection.texture_mapping {
            let texture = module.global_variables[mapping.texture]
                .binding
                .as_ref()
                .ok_or_else(|| format!("{name}: a texture without a binding"))?;
            let sampler = match mapping.sampler {
                Some(s) => {
                    let b = module.global_variables[s]
                        .binding
                        .as_ref()
                        .ok_or_else(|| format!("{name}: a sampler without a binding"))?;
                    format!("{} {}", b.group, b.binding)
                }
                None => "- -".to_owned(),
            };
            lines.insert(format!(
                "texture {} {} {sampler} {label} {sampler_name}",
                texture.group, texture.binding
            ));
        }
        stages.push(text);
    }
    let options = spv::Options {
        lang_version: (1, 0),
        // no coordinate-space adjustment, as for GLSL
        flags: spv::WriterFlags::empty(),
        ..Default::default()
    };
    let words = spv::write_vec(&module, &info, &options, None)
        .map_err(|e| format!("{name}: SPIR-V: {e}"))?;
    let spirv = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut bindings = String::new();
    let _ = writeln!(
        bindings,
        "# Generated by cupertino-gpu-shaders from {name}.wgsl; do not edit.\n\
         # uniform GROUP BINDING STAGE BLOCK | texture GROUP BINDING SAMPLER_GROUP SAMPLER_BINDING STAGE SAMPLER"
    );
    for line in lines {
        let _ = writeln!(bindings, "{line}");
    }
    let fragment = stages.pop().unwrap_or_default();
    let vertex = stages.pop().unwrap_or_default();
    Ok(Generated {
        vertex,
        fragment,
        spirv,
        bindings,
    })
}

/// A name of our own in `glsl` that GLSL reserves: one starting with `gl_` that is not a built-in (the built-ins
/// continue in capitals, `gl_Position`), as the generator makes of a WGSL name `gl` that clashes with nothing else.
fn reserved_name(glsl: &str) -> Option<String> {
    let mut rest = glsl;
    while let Some(i) = rest.find("gl_") {
        let before = rest[..i].chars().next_back();
        let tail = &rest[i + 3..];
        let ident: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let starts_word = before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if starts_word && !ident.starts_with(|c: char| c.is_ascii_uppercase()) {
            return Some(format!("gl_{ident}"));
        }
        rest = tail;
    }
    None
}

/// One directory of programs: its sources, where their generated files go, and what goes ahead of each program.
#[derive(Clone, Debug)]
pub struct SourceSet {
    /// The programs: every `.wgsl` file directly in it but the shared module.
    pub dir: PathBuf,
    /// Where the generated files go.
    pub generated: PathBuf,
    /// The shared module concatenated ahead of each program.
    pub shared: PathBuf,
    /// Where a program's include lines ([`INCLUDE`]) find the files they name.
    pub include: Option<PathBuf>,
    /// Also write each program's assembled WGSL (`NAME.wgsl`: the shared module, the included files and the
    /// program), for a backend that takes WGSL itself.
    pub wgsl: bool,
}

/// Every set of programs the build step generates: `cupertino-gpu`'s, and the backdrop and material programs of
/// `cupertino-glass`, which the compositor draws too, with the same shared module ahead of each.
pub fn sets() -> Vec<SourceSet> {
    let glass = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cupertino-glass/shaders");
    vec![
        SourceSet {
            dir: source_dir(),
            generated: generated_dir(),
            shared: source_dir().join(SHARED),
            include: None,
            wgsl: false,
        },
        SourceSet {
            generated: glass.join("generated"),
            shared: source_dir().join(SHARED),
            include: Some(glass.join("include")),
            wgsl: true,
            dir: glass,
        },
    ]
}

/// The start of a line that puts a file of the set's include directory in its place.
pub const INCLUDE: &str = "// include ";

/// `program` with each of its include lines replaced by the file it names, read by `read`.
pub fn resolve_includes(
    name: &str,
    program: &str,
    read: &dyn Fn(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    for line in program.lines() {
        match line.trim_end().strip_prefix(INCLUDE) {
            Some(file) => {
                let file = file.trim();
                let text = read(file).map_err(|e| format!("{name}: include {e}"))?;
                let _ = writeln!(out, "// ---- {file} ----");
                out.push_str(&text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// The WGSL a program is generated from: the shared module, then the program with its includes in place, without
/// the functions its entry points never reach. (A Vulkan driver's compiler has been seen to fail on an unused
/// function that takes arrays by value, so the WGSL a backend compiles holds only what the program runs.)
pub fn assembled(name: &str, shared: &str, program: &str) -> Result<String, String> {
    let text = format!(
        "// Generated by cupertino-gpu-shaders from {name}.wgsl and the shared module; do not edit.\n\
         {shared}\n// ---- {name}.wgsl ----\n{program}"
    );
    let module = naga::front::wgsl::parse_str(&text)
        .map_err(|e| format!("{name}: {}", e.emit_to_string(&text)))?;
    let mut used = reached(&module);
    used.extend(module.entry_points.iter().map(|e| e.name.clone()));
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        if skipping {
            skipping = line != "}";
            continue;
        }
        let unused = line
            .strip_prefix("fn ")
            .and_then(|rest| rest.split('(').next())
            .is_some_and(|f| !used.contains(f));
        if unused {
            // a function written on one line ends where it starts
            skipping = !line.trim_end().ends_with('}');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // what is left must still be a valid program with the same entry points
    let kept = naga::front::wgsl::parse_str(&out)
        .map_err(|e| format!("{name}: assembled: {}", e.emit_to_string(&out)))?;
    Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&kept)
        .map_err(|e| format!("{name}: assembled: {}", e.emit_to_string(&out)))?;
    Ok(out)
}

/// The names of the functions the entry points of `module` call, directly or through each other.
fn reached(module: &Module) -> BTreeSet<String> {
    fn calls(block: &naga::Block, out: &mut Vec<naga::Handle<naga::Function>>) {
        for statement in block.iter() {
            match statement {
                naga::Statement::Call { function, .. } => out.push(*function),
                naga::Statement::Block(b) => calls(b, out),
                naga::Statement::If { accept, reject, .. } => {
                    calls(accept, out);
                    calls(reject, out);
                }
                naga::Statement::Switch { cases, .. } => {
                    for case in cases {
                        calls(&case.body, out);
                    }
                }
                naga::Statement::Loop {
                    body, continuing, ..
                } => {
                    calls(body, out);
                    calls(continuing, out);
                }
                _ => {}
            }
        }
    }
    let mut stack = Vec::new();
    for entry in &module.entry_points {
        calls(&entry.function.body, &mut stack);
    }
    let mut seen = BTreeSet::new();
    while let Some(f) = stack.pop() {
        if seen.insert(f) {
            calls(&module.functions[f].body, &mut stack);
        }
    }
    seen.into_iter()
        .filter_map(|f| module.functions[f].name.clone())
        .collect()
}

/// Every generated file of the programs of `set`, by file name, from a fresh generation.
pub fn generate_set(set: &SourceSet) -> Result<Vec<(String, Vec<u8>)>, String> {
    let shared = std::fs::read_to_string(&set.shared)
        .map_err(|e| format!("{}: {e}", set.shared.display()))?;
    let read_in = |dir: &Path, file: &str| {
        std::fs::read_to_string(dir.join(file)).map_err(|e| format!("{file}: {e}"))
    };
    let mut files = Vec::new();
    for name in programs(&set.dir)? {
        let own = read_in(&set.dir, &format!("{name}.wgsl"))?;
        let program = match &set.include {
            Some(dir) => resolve_includes(&name, &own, &|f| read_in(dir, f))?,
            None => own,
        };
        let generated = generate(&name, &shared, &program)?;
        files.push((format!("{name}.vert.glsl"), generated.vertex.into_bytes()));
        files.push((format!("{name}.frag.glsl"), generated.fragment.into_bytes()));
        files.push((format!("{name}.spv"), generated.spirv));
        files.push((format!("{name}.bindings"), generated.bindings.into_bytes()));
        if set.wgsl {
            let text = assembled(&name, &shared, &program)?;
            files.push((format!("{name}.wgsl"), text.into_bytes()));
        }
    }
    Ok(files)
}

/// Every generated file of the programs in `dir`, by file name, from a fresh generation.
pub fn generate_all(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let read =
        |file: &str| std::fs::read_to_string(dir.join(file)).map_err(|e| format!("{file}: {e}"));
    let shared = read(SHARED)?;
    let mut files = Vec::new();
    for name in programs(dir)? {
        let generated = generate(&name, &shared, &read(&format!("{name}.wgsl"))?)?;
        files.push((format!("{name}.vert.glsl"), generated.vertex.into_bytes()));
        files.push((format!("{name}.frag.glsl"), generated.fragment.into_bytes()));
        files.push((format!("{name}.spv"), generated.spirv));
        files.push((format!("{name}.bindings"), generated.bindings.into_bytes()));
    }
    Ok(files)
}

/// The differences between a fresh generation and the committed files in `generated`: files that differ, are
/// missing, or are there without a program.
pub fn differences(fresh: &[(String, Vec<u8>)], generated: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for (file, bytes) in fresh {
        match std::fs::read(generated.join(file)) {
            Ok(committed) if &committed == bytes => {}
            Ok(_) => problems.push(format!("{file} differs from a fresh generation")),
            Err(_) => problems.push(format!("{file} is missing")),
        }
    }
    if let Ok(entries) = std::fs::read_dir(generated) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !fresh.iter().any(|(f, _)| *f == name) {
                problems.push(format!("{name} is not generated from any program"));
            }
        }
    }
    problems.sort();
    problems
}

/// Write `fresh` into `generated`, removing files no program generates.
pub fn write(fresh: &[(String, Vec<u8>)], generated: &Path) -> Result<(), String> {
    std::fs::create_dir_all(generated).map_err(|e| e.to_string())?;
    if let Ok(entries) = std::fs::read_dir(generated) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !fresh.iter().any(|(f, _)| *f == name) {
                std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
    }
    for (file, bytes) in fresh {
        std::fs::write(generated.join(file), bytes).map_err(|e| format!("{file}: {e}"))?;
    }
    Ok(())
}
