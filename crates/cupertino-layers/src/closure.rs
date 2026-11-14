//! The licence closure of the MPL-2.0 crates: every crate an MPL-2.0 workspace crate links, through normal and build
//! dependencies and external crates included, must have an accepted licence, and the toolkit and transport roots must
//! exist and be MPL-2.0.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{is_mpl, Problem};

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

/// The crates that must be in the workspace under MPL-2.0: the transport, the toolkit and the C library.
pub const REQUIRED_MPL_ROOTS: &[&str] = &[
    "cupertino-wayland-client",
    "cupertino-kit",
    "cupertino-ui-abi",
    "cupertino-ui-menus",
    "cupertino-ui-wayland",
    "cupertino-ui-controls",
    "cupertino-ui-core",
    "cupertino-ui-layout",
    "cupertino-ui-platform",
    "cupertino-ui-gpu",
    "cupertino-ui-render",
    "cupertino-ui-test",
    "cupertino-ui-text",
    "cupertino-ui-theme",
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

/// The closure problems of `meta` (the full `cargo metadata` of the workspace, all features), sorted by their text and
/// without repeats. `required_roots` must exist and be MPL-2.0. Metadata without the fields cargo writes is an error.
pub(crate) fn check_roots(meta: &Value, required_roots: &[&str]) -> Result<Vec<Problem>, String> {
    if !well_formed(meta) {
        return Err(
            "cargo metadata lacks the packages, members or resolved nodes it always has".to_owned(),
        );
    }
    let problems = closure_problems(meta, required_roots);
    let mut sorted: BTreeMap<String, Problem> = BTreeMap::new();
    for problem in problems {
        sorted.entry(problem.to_string()).or_insert(problem);
    }
    Ok(sorted.into_values().collect())
}

/// Whether `meta` has the fields [`closure_problems`] reads, so it cannot panic on them: every member and dependency
/// names a known package and node.
fn well_formed(meta: &Value) -> bool {
    let (Some(packages), Some(members), Some(nodes)) = (
        meta["packages"].as_array(),
        meta["workspace_members"].as_array(),
        meta["resolve"]["nodes"].as_array(),
    ) else {
        return false;
    };
    let ids: BTreeSet<&str> = packages.iter().filter_map(|p| p["id"].as_str()).collect();
    let node_ids: BTreeSet<&str> = nodes.iter().filter_map(|n| n["id"].as_str()).collect();
    let deps_known = nodes.iter().all(|n| {
        n["deps"].as_array().is_some_and(|deps| {
            deps.iter().all(|d| {
                d["pkg"]
                    .as_str()
                    .is_some_and(|id| ids.contains(id) && node_ids.contains(id))
                    && d["dep_kinds"].as_array().is_some()
            })
        })
    });
    ids.len() == packages.len()
        && packages.iter().all(|p| p["name"].as_str().is_some())
        && meta["workspace_root"].as_str().is_some()
        && members
            .iter()
            .all(|m| m.as_str().is_some_and(|id| ids.contains(id)))
        && deps_known
}

fn closure_problems(meta: &Value, required_roots: &[&str]) -> Vec<Problem> {
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
    let mut problems = Vec::new();
    for name in required_roots {
        match workspace.iter().find_map(|id| {
            let package = packages.get(id).expect("workspace package");
            (package["name"].as_str() == Some(name)).then_some(package)
        }) {
            None => problems.push(Problem::MissingMplRoot((*name).to_owned())),
            Some(package) if !package["license"].as_str().is_some_and(is_mpl) => {
                problems.push(Problem::NonMplRoot(
                    (*name).to_owned(),
                    package["license"]
                        .as_str()
                        .unwrap_or("<missing>")
                        .to_owned(),
                ))
            }
            Some(_) => {}
        }
    }
    for root in &workspace {
        let package = packages.get(root).expect("workspace package");
        if !package["license"].as_str().is_some_and(is_mpl) {
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
                    problems.push(Problem::UnapprovedLicence {
                        root: packages[root]["name"]
                            .as_str()
                            .expect("root name")
                            .to_owned(),
                        package: package["name"].as_str().expect("package name").to_owned(),
                        version: package["version"].as_str().unwrap_or_default().to_owned(),
                        licence: licence.unwrap_or("<missing>").to_owned(),
                    });
                }
            }
            // a package cargo resolved no node for has no dependencies to follow
            let Some(node) = nodes.get(id) else {
                continue;
            };
            for dep in node["deps"].as_array().expect("resolved deps") {
                if dep["dep_kinds"]
                    .as_array()
                    .expect("dependency kinds")
                    .iter()
                    .any(|k| k["kind"].is_null() || k["kind"].as_str() == Some("build"))
                {
                    todo.push(dep["pkg"].as_str().expect("dependency id"));
                }
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn texts(problems: Result<Vec<Problem>, String>) -> Vec<String> {
        problems.unwrap().iter().map(Problem::to_string).collect()
    }

    #[test]
    fn metadata_without_its_fields_is_an_error_not_a_panic() {
        assert!(check_roots(&json!({"packages": []}), &[]).is_err());
    }

    #[test]
    fn required_roots_cover_the_denied_kit_crates() {
        for (name, _) in crate::DENIED_CRATES {
            if *name == "cupertino-kit"
                || *name == "cupertino-wayland-client"
                || name.starts_with("cupertino-ui-")
            {
                assert!(
                    REQUIRED_MPL_ROOTS.contains(name),
                    "{name} is missing from REQUIRED_MPL_ROOTS"
                );
            }
        }
    }

    #[test]
    fn evaluates_expressions_and_rejects_unknown_terms() {
        assert!(accepted("MIT OR GPL-3.0-only"));
        assert!(accepted("MIT/Apache-2.0"));
        assert!(accepted("Apache-2.0 WITH LLVM-exception"));
        assert!(!accepted("MIT AND GPL-3.0-only"));
        assert!(!accepted("GPL-3.0-or-later"));
        assert!(!accepted("LicenseRef-unknown"));
        assert!(!accepted(""));
    }

    #[test]
    fn follows_normal_and_build_edges_and_keeps_package_versions_distinct() {
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
            texts(check_roots(&meta, &["root"])),
            vec![
                "root MPL closure: build 1 has unapproved licence GPL-3.0-only",
                "root MPL closure: dep 2 has unapproved licence GPL-3.0-only"
            ]
        );
    }

    #[test]
    fn required_roots_must_exist_and_be_mpl() {
        let meta = json!({"workspace_root":"/workspace", "workspace_members":["root"],
            "packages":[{"id":"root", "name":"root", "license":"MIT"}], "resolve":{"nodes":[]}});
        assert_eq!(
            texts(check_roots(&meta, &["root", "absent"])),
            vec![
                "required MPL root absent is missing from workspace",
                "required MPL root root has non-MPL licence MIT",
            ]
        );
    }

    #[test]
    fn missing_licence_requires_the_reviewed_xkbcommon_text() {
        let root = env!("CARGO_MANIFEST_DIR");
        let root = std::path::Path::new(root)
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let root = root.to_str().unwrap();
        let package =
            json!({"manifest_path": root.to_owned() + "/third-party/xkbcommon-rs/Cargo.toml"});
        assert!(clarified(&package, root));
        assert!(!clarified(
            &json!({"manifest_path":"/elsewhere/Cargo.toml"}),
            root
        ));
        let meta = json!({"workspace_root":root, "workspace_members":["root"],
            "packages":[{"id":"root", "name":"root", "license":"MPL-2.0"},
                {"id":"x", "name":"xkbcommon", "version":"1", "license":null, "manifest_path":package["manifest_path"]}],
            "resolve":{"nodes":[{"id":"root", "deps":[{"pkg":"x", "dep_kinds":[{"kind":null}]}]}, {"id":"x", "deps":[]}]}});
        assert!(check_roots(&meta, &["root"]).unwrap().is_empty());
        let mut unclarified = meta;
        unclarified["packages"][1]["manifest_path"] = json!("/elsewhere/Cargo.toml");
        assert_eq!(
            texts(check_roots(&unclarified, &["root"])),
            vec!["root MPL closure: xkbcommon 1 has unapproved licence <missing>"]
        );
    }
}
