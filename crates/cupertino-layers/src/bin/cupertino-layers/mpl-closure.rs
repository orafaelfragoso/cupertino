use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};

const ACCEPTED: &[&str] = &[
    "0BSD",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "CC0-1.0",
    "ISC",
    "MIT",
    "MPL-2.0",
    "Unicode-3.0",
    "Unlicense",
    "Zlib",
];

fn accepted(expression: &str) -> bool {
    spdx::Expression::parse_mode(expression, spdx::ParseMode::LAX)
        .map(|expression| expression.evaluate(|req| ACCEPTED.contains(&req.to_string().as_str())))
        .unwrap_or(false)
}

// Match cargo-deny's clarification only for the vendored copy and its reviewed licence text.
fn clarified(package: &Value, root: &str) -> bool {
    let expected = std::path::Path::new(root).join("third-party/xkbcommon-rs/Cargo.toml");
    if package["manifest_path"].as_str().map(std::path::Path::new) != Some(expected.as_path()) {
        return false;
    }
    let Ok(contents) = std::fs::read(expected.with_file_name("LICENSE")) else {
        return false;
    };
    format!("{:x}", Sha256::digest(contents))
        == "091f822cc42299060805fc0a4c7a6552147f22aa6833147a15a6be0111dced30"
}

pub fn check() -> Vec<String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
        ])
        .output()
        .expect("run cargo metadata for MPL closure");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let metadata: Value = serde_json::from_slice(&out.stdout).expect("cargo metadata JSON");
    check_metadata(&metadata)
}

fn check_metadata(meta: &Value) -> Vec<String> {
    let packages: BTreeMap<_, _> = meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|p| (p["id"].as_str().expect("package id"), p))
        .collect();
    let workspace: BTreeSet<_> = meta["workspace_members"]
        .as_array()
        .expect("workspace members")
        .iter()
        .map(|id| id.as_str().expect("workspace id"))
        .collect();
    let nodes: BTreeMap<_, _> = meta["resolve"]["nodes"]
        .as_array()
        .expect("resolved nodes")
        .iter()
        .map(|n| (n["id"].as_str().expect("node id"), n))
        .collect();
    let mut problems = BTreeSet::new();
    for root in &workspace {
        let package = packages.get(root).expect("workspace package");
        if !package["license"]
            .as_str()
            .is_some_and(cupertino_layers::is_mpl)
        {
            continue;
        }
        let mut seen = BTreeSet::new();
        let mut todo = vec![*root];
        while let Some(id) = todo.pop() {
            if !seen.insert(id) {
                continue;
            }
            let package = packages.get(id).expect("resolved package");
            if !workspace.contains(id) {
                let licence = package["license"].as_str();
                if !licence.is_some_and(accepted)
                    && !(licence.is_none()
                        && clarified(
                            package,
                            meta["workspace_root"].as_str().expect("workspace root"),
                        ))
                {
                    problems.insert(format!(
                        "{} MPL closure: {} {} has unapproved licence {}",
                        packages[root]["name"].as_str().expect("root name"),
                        package["name"].as_str().expect("package name"),
                        package["version"].as_str().expect("package version"),
                        licence.unwrap_or("<missing>")
                    ));
                }
            }
            let node = nodes.get(id).expect("resolved node");
            for dep in node["deps"].as_array().expect("resolved deps") {
                if dep["dep_kinds"]
                    .as_array()
                    .expect("dependency kinds")
                    .iter()
                    .any(|k| k["kind"].is_null())
                {
                    todo.push(dep["pkg"].as_str().expect("dependency id"));
                }
            }
        }
    }
    problems.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn evaluates_expressions_and_rejects_unknown_terms() {
        assert!(accepted("MIT OR GPL-3.0-only"));
        assert!(accepted("MIT/Apache-2.0"));
        assert!(!accepted("MIT AND GPL-3.0-only"));
        assert!(!accepted("GPL-3.0-or-later"));
        assert!(!accepted("LicenseRef-unknown"));
        assert!(!accepted(""));
    }

    #[test]
    fn follows_only_normal_edges_and_keeps_package_versions_distinct() {
        let meta = json!({
            "workspace_root": "/workspace",
            "workspace_members": ["root"],
            "packages": [
                {"id":"root", "name":"root", "version":"1", "license":"MPL-2.0"},
                {"id":"a", "name":"dep", "version":"1", "license":"MIT"},
                {"id":"b", "name":"dep", "version":"2", "license":"GPL-3.0-only"},
                {"id":"c", "name":"build", "version":"1", "license":"GPL-3.0-only"}
            ],
            "resolve": {"nodes": [
                {"id":"root", "deps":[{"pkg":"a", "dep_kinds":[{"kind":null}]}, {"pkg":"c", "dep_kinds":[{"kind":"build"}]}]},
                {"id":"a", "deps":[{"pkg":"b", "dep_kinds":[{"kind":null}]}]},
                {"id":"b", "deps":[]}, {"id":"c", "deps":[]}
            ]}
        });
        assert_eq!(
            check_metadata(&meta),
            vec!["root MPL closure: dep 2 has unapproved licence GPL-3.0-only"]
        );
    }
}
