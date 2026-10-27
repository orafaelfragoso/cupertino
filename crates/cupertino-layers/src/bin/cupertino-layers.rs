//! Reads `cargo metadata` for the workspace and reports every dependency that breaks the layer rules, today's and the
//! target's.
//!
//! ```text
//! cupertino-layers                          (run from anywhere inside the workspace)
//! cupertino-layers --graph current|target   DOT graph of one view on stdout
//! ```

use cupertino_layers::{
    check, classify, dot, edges, Layer, DENIED_PATHS, PENDING, RULES, TARGET, TARGET_EXCEPTIONS,
    TARGET_RULES,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
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

    let (layers, deps) = workspace();
    let current_edges = edges(&layers, &deps);
    let (target, mut target_problems) = classify(layers.keys(), TARGET);
    let target_edges = edges(&target, &deps);
    let problems = check(RULES, &current_edges, PENDING);
    target_problems.extend(check(TARGET_RULES, &target_edges, TARGET_EXCEPTIONS));
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

/// Each workspace crate's declared layer, and every dependency (normal, build, dev) of a workspace crate.
fn workspace() -> (BTreeMap<String, Layer>, BTreeSet<(String, String)>) {
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
        for d in p["dependencies"].as_array().expect("dependencies") {
            let to = d["name"].as_str().expect("dependency name");
            deps.insert((name.to_owned(), to.to_owned()));
        }
    }
    (layers, deps)
}
