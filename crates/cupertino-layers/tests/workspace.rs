//! `check_workspace_with` on fixture workspaces: a clean one, one with an edge the layers deny, and one whose MPL-2.0
//! crate links a crate under a licence the closure does not accept.

use std::path::{Path, PathBuf};
use std::process::Command;

use cupertino_layers::{
    check_workspace_with, Layer, LayersError, Policy, Problem, DENIED_PATHS, GRAPH_SNAPSHOT, RULES,
};

/// The repository's rules, with `kit` as the one crate that must be MPL-2.0.
const POLICY: Policy = Policy {
    rules: RULES,
    exceptions: &[],
    denied_paths: DENIED_PATHS,
    denied_crates: &[],
    mpl_roots: &["kit"],
};

const MPL_TEXT: &str = "Mozilla Public License Version 2.0\n==================================\n";

/// A workspace of `crates/<name>` members, plus crates under `vendor/` that are path dependencies but not members.
struct Fixture {
    dir: PathBuf,
}

/// `(name, layer, licence, path dependencies)`; a dependency `vendor/x` is a crate outside the members.
type Krate<'a> = (&'a str, &'a str, &'a str, &'a [&'a str]);

impl Fixture {
    /// A fresh workspace in the directory `name` under the target's temporary directory.
    fn new(name: &str, members: &[Krate], vendored: &[(&str, &str)]) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("layers-workspace")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.as_path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"vendor\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        for (name, layer, licence, deps) in members {
            let mut manifest = format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"{licence}\"\n\n\
                 [package.metadata.cupertino]\nlayer = \"{layer}\"\n\n[dependencies]\n"
            );
            for dep in *deps {
                let (dep_name, path) = match dep.strip_prefix("vendor/") {
                    Some(v) => (v, format!("../../vendor/{v}")),
                    None => (*dep, format!("../{dep}")),
                };
                manifest.push_str(&format!("{dep_name} = {{ path = \"{path}\" }}\n"));
            }
            let krate = root.join("crates").join(name);
            write_crate(&krate, &manifest);
            if licence.starts_with("MPL-2.0") {
                std::fs::write(krate.join("LICENSE"), MPL_TEXT).unwrap();
            }
        }
        for (name, licence) in vendored {
            let manifest = format!(
                "[package]\nname = \"{name}\"\nversion = \"0.2.0\"\nedition = \"2021\"\nlicense = \"{licence}\"\n"
            );
            write_crate(&root.join("vendor").join(name), &manifest);
        }
        let lock = cargo(root)
            .args(["generate-lockfile", "--offline"])
            .output()
            .unwrap();
        assert!(
            lock.status.success(),
            "{}",
            String::from_utf8_lossy(&lock.stderr)
        );
        Self { dir }
    }

    fn root(&self) -> &Path {
        &self.dir
    }

    fn check(&self, policy: &Policy) -> Result<cupertino_layers::Findings, LayersError> {
        let home = self.root().join("cargo-home");
        check_workspace_with(
            &self.root().join("crates"),
            &|| {
                let mut c = Command::new(env!("CARGO"));
                c.env("CARGO_HOME", &home);
                c
            },
            policy,
        )
    }
}

/// `app` depending on `kit`, which the layers allow, and `cupertino-layers`, the crate whose folder holds the graph
/// snapshot.
const WITH_SNAPSHOT_CRATE: &[Krate] = &[
    ("kit", "foundation", "MPL-2.0", &[]),
    ("app", "apps", "GPL-3.0-or-later", &["kit"]),
    ("cupertino-layers", "tools", "GPL-3.0-or-later", &[]),
];

fn write_snapshot(root: &Path, graph: &str) {
    let path = root.join(GRAPH_SNAPSHOT);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, graph).unwrap();
}

fn write_crate(dir: &Path, manifest: &str) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "").unwrap();
}

/// Cargo in `dir` with its own home, so the user's configuration is not read.
fn cargo(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO"));
    c.current_dir(dir).env("CARGO_HOME", dir.join("cargo-home"));
    c
}

fn texts<'a>(problems: impl Iterator<Item = &'a Problem>) -> Vec<String> {
    problems.map(Problem::to_string).collect()
}

#[test]
fn a_clean_workspace_has_its_layers_edges_and_no_problem() {
    let w = Fixture::new(
        "clean",
        &[
            ("kit", "foundation", "MPL-2.0", &[]),
            ("app", "apps", "GPL-3.0-or-later", &["kit"]),
        ],
        &[],
    );
    let findings = w.check(&POLICY).unwrap();
    assert!(findings.is_clean(), "{:?}", texts(findings.all_problems()));
    assert_eq!(findings.layers.get("kit"), Some(&Layer::Foundation));
    assert_eq!(findings.layers.get("app"), Some(&Layer::Apps));
    assert_eq!(findings.edges.len(), 1);
    assert_eq!(
        findings.summary(),
        "2 crates, 1 internal dependencies, 0 exceptions, 0 problems"
    );
    assert_eq!(
        findings.by_layer().get(&Layer::Apps).map(Vec::as_slice),
        Some(&["app"][..])
    );
    assert!(
        findings.dot().contains("\"app\" -> \"kit\""),
        "{}",
        findings.dot()
    );
}

#[test]
fn an_edge_the_layers_deny_is_a_problem_with_its_ends() {
    let w = Fixture::new(
        "denied",
        &[
            ("kit", "foundation", "MPL-2.0", &["app"]),
            ("app", "apps", "GPL-3.0-or-later", &[]),
        ],
        &[],
    );
    let findings = w.check(&POLICY).unwrap();
    assert!(!findings.is_clean());
    let forbidden = findings
        .problems
        .iter()
        .find(|p| p.kind() == "forbidden_edge")
        .expect("the denied edge");
    assert_eq!((forbidden.from(), forbidden.to()), ("kit", Some("app")));
    assert_eq!(
        forbidden.to_string(),
        "kit (foundation) must not depend on app (apps), normal dependency"
    );
    assert!(
        findings
            .problems
            .iter()
            .any(|p| p.kind() == "gpl_dependency"),
        "{:?}",
        texts(findings.all_problems())
    );
}

#[test]
fn an_mpl_crate_linking_an_unaccepted_licence_breaks_the_closure() {
    let w = Fixture::new(
        "closure",
        &[("kit", "foundation", "MPL-2.0", &["vendor/copyleft"])],
        &[("copyleft", "GPL-3.0-only")],
    );
    let findings = w.check(&POLICY).unwrap();
    assert!(
        findings.problems.is_empty(),
        "{:?}",
        texts(findings.problems.iter())
    );
    assert_eq!(
        texts(findings.closure.iter()),
        ["kit MPL closure: copyleft 0.2.0 has unapproved licence GPL-3.0-only"]
    );
    assert_eq!(findings.closure[0].kind(), "unapproved_licence");
    assert_eq!(findings.closure[0].to(), Some("copyleft"));
    assert!(findings.summary().ends_with(", 1 problems"));
}

#[test]
fn a_missing_root_a_crate_without_a_layer_and_no_cargo_are_reported() {
    let w = Fixture::new("missing", &[("kit", "foundation", "MPL-2.0", &[])], &[]);
    let policy = Policy {
        mpl_roots: &["kit", "absent"],
        ..POLICY
    };
    let findings = w.check(&policy).unwrap();
    assert_eq!(
        texts(findings.closure.iter()),
        ["required MPL root absent is missing from workspace"]
    );

    let manifest = w.root().join("crates/kit/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, text.replace("layer = \"foundation\"", "")).unwrap();
    match w.check(&POLICY) {
        Err(LayersError::Manifest(m)) => {
            assert_eq!(
                m,
                "kit: no `[package.metadata.cupertino] layer` in its manifest"
            )
        }
        other => panic!("{other:?}"),
    }

    let missing = PathBuf::from("/nonexistent/cargo");
    let no_cargo = || {
        let mut c = Command::new(&missing);
        c.env("CUPERTINO_ASSET_SEARCH_PATH", "");
        c
    };
    let result = check_workspace_with(w.root(), &no_cargo, &POLICY);
    assert!(matches!(result, Err(LayersError::Cargo(_))), "{result:?}");
}

#[test]
fn a_snapshot_that_differs_only_in_its_counts_is_current() {
    let w = Fixture::new("snapshot-current", WITH_SNAPSHOT_CRATE, &[]);
    let graph = w.check(&POLICY).unwrap().dot();
    let recounted = graph
        .lines()
        .map(|l| match l {
            l if l.ends_with(" internal dependencies") => "// 9 crates, 99 internal dependencies",
            l if l.starts_with("// per layer: ") => "// per layer: apps 9",
            l => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_ne!(recounted, graph, "the graph has both count lines");
    write_snapshot(w.root(), &recounted);
    let findings = w.check(&POLICY).unwrap();
    assert!(findings.is_clean(), "{:?}", texts(findings.all_problems()));
}

#[test]
fn a_snapshot_that_lost_an_edge_is_out_of_date_except_for_the_run_that_prints_the_graph() {
    let w = Fixture::new("snapshot-stale", WITH_SNAPSHOT_CRATE, &[]);
    let graph = w.check(&POLICY).unwrap().dot();
    let edge = "\"app\" -> \"kit\"";
    let stale: String = graph
        .lines()
        .filter(|l| !l.contains(edge))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_ne!(stale, graph, "the graph has the edge");
    write_snapshot(w.root(), &stale);

    let findings = w.check(&POLICY).unwrap();
    assert!(!findings.is_clean());
    assert_eq!(
        texts(findings.all_problems()),
        [format!(
            "{GRAPH_SNAPSHOT} is out of date: cargo run -q -p cupertino-layers -- --graph > {GRAPH_SNAPSHOT}"
        )]
    );
    assert_eq!(findings.snapshot[0].kind(), "stale_graph");
    assert!(
        findings.summary().ends_with(", 1 problems"),
        "{}",
        findings.summary()
    );
    assert_eq!(findings.problems_printing_the_graph().count(), 0);

    write_snapshot(w.root(), &graph);
    assert!(w.check(&POLICY).unwrap().is_clean());
}
