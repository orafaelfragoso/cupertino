//! Reads `cargo metadata` for the workspace and reports every dependency that breaks the layer rules, today's and the
//! target's.
//!
//! ```text
//! cupertino-layers                          (run from anywhere inside the workspace)
//! cupertino-layers --graph current|target   DOT graph of one view on stdout
//! ```

use cupertino_layers::{
    check, classify, denied_crate_paths, dot, edges, licence_problems, Dep, Kind, Layer, Problem,
    DENIED_CRATES, DENIED_PATHS, PENDING, RULES, TARGET, TARGET_EXCEPTIONS, TARGET_RULES,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let graph = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => None,
        ["--graph", view @ ("current" | "target")] => Some(view.to_owned()),
        _ => {
            eprintln!("usage: cupertino-layers [--graph current|target]");
            return ExitCode::from(2);
        }
    };

    let (layers, deps, licences) = workspace();
    let current_edges = edges(&layers, &deps);
    let (target, mut target_problems) = classify(layers.keys(), TARGET);
    let target_edges = edges(&target, &deps);
    let mut problems = check(RULES, &current_edges, PENDING, DENIED_PATHS);
    problems.extend(
        denied_crate_paths(&deps, DENIED_CRATES)
            .into_iter()
            .map(Problem::DeniedPath),
    );
    problems.extend(licence_problems(&licences.licences, &deps, |name| {
        licences.has_text.get(name).copied().unwrap_or(false)
    }));
    target_problems.extend(check(
        TARGET_RULES,
        &target_edges,
        TARGET_EXCEPTIONS,
        DENIED_PATHS,
    ));
    for p in &problems {
        eprintln!("{p}");
    }
    for p in &target_problems {
        eprintln!("target: {p}");
    }

    match graph.as_deref() {
        Some("current") => print!(
            "{}",
            dot(
                "current",
                RULES,
                &layers,
                &current_edges,
                PENDING,
                DENIED_PATHS
            )
        ),
        Some(_) => print!(
            "{}",
            dot(
                "target",
                TARGET_RULES,
                &target,
                &target_edges,
                TARGET_EXCEPTIONS,
                DENIED_PATHS
            )
        ),
        None => println!(
            "{} crates, {} internal dependencies, {} pending, {} target exceptions, {} problems",
            layers.len(),
            current_edges.len(),
            PENDING.len(),
            TARGET_EXCEPTIONS.len(),
            problems.len() + target_problems.len()
        ),
    }
    if problems.is_empty() && target_problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The licence of each workspace crate, and whether a `LICENSE` file with the MPL-2.0 text sits next to its manifest.
struct Licences {
    licences: BTreeMap<String, String>,
    has_text: BTreeMap<String, bool>,
}

/// Each workspace crate's declared layer, every dependency (normal, build, dev) of a workspace crate, and its licence.
fn workspace() -> (BTreeMap<String, Layer>, BTreeSet<Dep>, Licences) {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: Value = serde_json::from_slice(&out.stdout).expect("cargo metadata JSON");
    let mut layers = BTreeMap::new();
    let mut deps = BTreeSet::new();
    let mut licences = Licences {
        licences: BTreeMap::new(),
        has_text: BTreeMap::new(),
    };
    for p in meta["packages"].as_array().expect("packages") {
        let name = p["name"].as_str().expect("package name");
        let layer = p["metadata"]["cupertino"]["layer"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("{name}: no `[package.metadata.cupertino] layer` in its manifest")
            });
        let layer = Layer::from_name(layer)
            .filter(|l| RULES.iter().any(|(r, _)| r == l))
            .unwrap_or_else(|| panic!("{name}: `{layer}` is not a layer in RULES"));
        layers.insert(name.to_owned(), layer);
        licences.licences.insert(
            name.to_owned(),
            p["license"].as_str().unwrap_or_default().to_owned(),
        );
        let manifest = Path::new(p["manifest_path"].as_str().expect("manifest path"));
        let text = std::fs::read_to_string(manifest.with_file_name("LICENSE")).unwrap_or_default();
        licences.has_text.insert(
            name.to_owned(),
            text.trim_start()
                .starts_with("Mozilla Public License Version 2.0"),
        );
        for d in p["dependencies"].as_array().expect("dependencies") {
            let to = d["name"].as_str().expect("dependency name");
            let kind = Kind::from_metadata(d["kind"].as_str())
                .unwrap_or_else(|| panic!("{name} -> {to}: unknown dependency kind {}", d["kind"]));
            deps.insert((name.to_owned(), to.to_owned(), kind));
        }
    }
    (layers, deps, licences)
}
