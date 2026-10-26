//! Reads `cargo metadata` for the workspace and reports every dependency that breaks the layer rules.
//!
//! ```text
//! cupertino-layers        (run from anywhere inside the workspace)
//! ```

use cupertino_layers::{check, Edge, Layer, PENDING};
use serde_json::Value;
use std::collections::HashMap;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("run cargo metadata");
    assert!(out.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
    let meta: Value = serde_json::from_slice(&out.stdout).expect("cargo metadata JSON");
    let packages = meta["packages"].as_array().expect("packages");
    let layer_of: HashMap<&str, Layer> = packages
        .iter()
        .map(|p| {
            let name = p["name"].as_str().expect("package name");
            let layer = p["metadata"]["cupertino"]["layer"]
                .as_str()
                .unwrap_or_else(|| panic!("{name}: no `[package.metadata.cupertino] layer` in its manifest"));
            (name, Layer::from_name(layer).unwrap_or_else(|| panic!("{name}: `{layer}` is not a layer")))
        })
        .collect();
    let mut edges = Vec::new();
    for p in packages {
        let from = p["name"].as_str().expect("package name");
        for d in p["dependencies"].as_array().expect("dependencies") {
            let to = d["name"].as_str().expect("dependency name");
            if let Some(&to_layer) = layer_of.get(to) {
                let e = Edge { from: from.into(), from_layer: layer_of[from], to: to.into(), to_layer };
                if !edges.contains(&e) {
                    edges.push(e);
                }
            }
        }
    }
    let problems = check(&edges, PENDING);
    for p in &problems {
        eprintln!("{p}");
    }
    println!("{} crates, {} internal dependencies, {} pending, {} problems", layer_of.len(), edges.len(), PENDING.len(), problems.len());
    if problems.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
