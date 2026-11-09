//! `cupertino dev layers` on fixture checkouts with a policy given through `Context::provide`, and `cupertino dev
//! manual`.

mod support;

use std::path::{Path, PathBuf};

use cupertino_cli::{Context, Output, Runner, REGISTRY};
use cupertino_layers::{Policy, DENIED_PATHS, RULES};
use serde_json::{json, Value};
use support::{args, envelope, kinds, records};

/// The repository's layer rules, with `kit` as the one crate that must be MPL-2.0.
const POLICY: Policy = Policy {
    rules: RULES,
    exceptions: &[],
    denied_paths: DENIED_PATHS,
    denied_crates: &[],
    mpl_roots: &["kit"],
};

/// A checkout: a workspace of `kit` (foundation, MPL-2.0), `app` (apps) and `cupertino-layers` (tools), with `edges`
/// as path dependencies.
struct Checkout {
    dir: tempfile::TempDir,
}

impl Checkout {
    fn new(edges: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        for (name, layer, licence) in [
            ("kit", "foundation", "MPL-2.0"),
            ("app", "apps", "GPL-3.0-or-later"),
            ("cupertino-layers", "tools", "GPL-3.0-or-later"),
        ] {
            let mut manifest = format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"{licence}\"\n\n\
                 [package.metadata.cupertino]\nlayer = \"{layer}\"\n\n[dependencies]\n"
            );
            for (_, to) in edges.iter().filter(|(from, _)| *from == name) {
                manifest.push_str(&format!("{to} = {{ path = \"../{to}\" }}\n"));
            }
            let krate = root.join("crates").join(name);
            std::fs::create_dir_all(krate.join("src")).unwrap();
            std::fs::write(krate.join("Cargo.toml"), manifest).unwrap();
            std::fs::write(krate.join("src/lib.rs"), "").unwrap();
            if licence == "MPL-2.0" {
                std::fs::write(
                    krate.join("LICENSE"),
                    "Mozilla Public License Version 2.0\n",
                )
                .unwrap();
            }
        }
        let lock = std::process::Command::new(env!("CARGO"))
            .args(["generate-lockfile", "--offline"])
            .current_dir(root)
            .env("CARGO_HOME", root.join("cargo-home"))
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
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
        self.dir.path()
    }

    /// A context in `current_dir` with Cargo from the build, its own Cargo home and a temporary `HOME`. A Cargo that
    /// rustup installed finds its toolchain through the test's `RUSTUP_HOME` and `RUSTUP_TOOLCHAIN`.
    fn context(&self, current_dir: &Path) -> Context<'static> {
        let mut cx = Context::isolated(self.root())
            .with_current_dir(current_dir)
            .with_var("CARGO", env!("CARGO"))
            .with_var("CARGO_HOME", self.root().join("cargo-home"))
            .with_var("HOME", self.root().join("home"))
            .with_var("PATH", "/usr/bin:/bin")
            .with_var("CUPERTINO_ASSET_SEARCH_PATH", "");
        let rustup_home = std::env::var_os("RUSTUP_HOME").or_else(|| {
            Some(
                Path::new(&std::env::var_os("HOME")?)
                    .join(".rustup")
                    .into_os_string(),
            )
        });
        if let Some(home) = rustup_home {
            cx = cx.with_var("RUSTUP_HOME", home);
        }
        if let Some(toolchain) = std::env::var_os("RUSTUP_TOOLCHAIN") {
            cx = cx.with_var("RUSTUP_TOOLCHAIN", toolchain);
        }
        cx
    }

    fn run(&self, words: &[&str]) -> Output {
        self.run_in(self.root(), words)
    }

    fn run_in(&self, current_dir: &Path, words: &[&str]) -> Output {
        Runner::new(REGISTRY)
            .context(self.context(current_dir).provide(&POLICY))
            .run(&args(words), "r1")
    }
}

#[test]
fn a_clean_checkout_gives_its_crates_and_layers_in_every_form() {
    let c = Checkout::new(&[("app", "kit")]);
    let out = c.run(&["--format", "json", "dev", "layers"]);
    assert_eq!(out.code, 0, "{}", out.stdout);
    let v = envelope(&out);
    assert_eq!(v["command"], "dev layers");
    assert_eq!(
        v["data"],
        json!({
            "root": c.root(),
            "crates": 3,
            "edges": 1,
            "exceptions": 0,
            "by_layer": {"foundation": ["kit"], "apps": ["app"], "tools": ["cupertino-layers"]},
            "problems": [],
        })
    );

    let out = c.run(&["dev", "layers"]);
    assert_eq!(
        (out.code, out.stdout.as_str(), out.stderr.as_str()),
        (
            0,
            "3 crates, 1 internal dependencies, 0 exceptions, 0 problems\n",
            ""
        )
    );

    let out = c.run(&["--format", "jsonl", "dev", "layers"]);
    let records = records(&out, "dev layers");
    assert_eq!(kinds(&records), ["result", "end"]);
    assert_eq!(records[0]["data"]["crates"], 3);
}

#[test]
fn graph_prints_the_dot_graph() {
    let c = Checkout::new(&[("app", "kit")]);
    let out = c.run(&["dev", "layers", "--graph"]);
    assert_eq!(out.code, 0);
    assert!(
        out.stdout.starts_with("// cupertino-layers --graph\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("digraph \"layers\" {"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("  \"app\" -> \"kit\";\n"),
        "{}",
        out.stdout
    );
    let json = c.run(&["--format", "json", "dev", "layers", "--graph"]);
    assert_eq!(envelope(&json)["data"]["dot"], out.stdout);
}

#[test]
fn the_checkout_is_found_above_the_working_directory() {
    let c = Checkout::new(&[]);
    let deep = c.root().join("crates/app/src");
    let out = c.run_in(&deep, &["--format", "json", "dev", "layers"]);
    assert_eq!(out.code, 0, "{}", out.stdout);
    assert_eq!(envelope(&out)["data"]["root"], json!(c.root()));
    let relative = c.run_in(
        &c.root().join("crates"),
        &["--format", "json", "dev", "layers", "--root", ".."],
    );
    assert_eq!(
        envelope(&relative)["data"]["root"],
        json!(c.root().join("crates/.."))
    );
}

#[test]
fn problems_exit_5_with_each_problem_as_data_and_as_an_event() {
    // a foundation crate depending on an application: a denied edge, and an MPL-2.0 crate depending on a GPL one
    let c = Checkout::new(&[("kit", "app")]);
    let out = c.run(&["--format", "json", "dev", "layers"]);
    assert_eq!(out.code, 5);
    let v = envelope(&out);
    assert_eq!(v["error"]["kind"], "validation");
    assert_eq!(v["error"]["code"], "layer_violations");
    assert_eq!(
        v["error"]["message"],
        format!("2 layer and licence problems in {}", c.root().display())
    );
    let problems = v["data"]["problems"].as_array().unwrap();
    assert_eq!(
        problems[0],
        json!({
            "kind": "forbidden_edge",
            "from": "kit",
            "to": "app",
            "detail": "kit (foundation) must not depend on app (apps), normal dependency",
        })
    );
    assert_eq!(problems[1]["kind"], "gpl_dependency");
    assert_eq!(
        v["data"]["crates"], 3,
        "the counts are there beside the problems"
    );

    let out = c.run(&["--format", "jsonl", "dev", "layers"]);
    assert_eq!(out.code, 5);
    let records = records(&out, "dev layers");
    assert_eq!(kinds(&records), ["event:problem", "event:problem", "end"]);
    assert_eq!(records[0]["data"], problems[0]);
    assert_eq!(records[2]["error"]["code"], "layer_violations");

    let out = c.run(&["dev", "layers"]);
    assert_eq!(out.code, 5);
    assert_eq!(
        out.stdout,
        "kit (foundation) must not depend on app (apps), normal dependency\n\
         kit is MPL-2.0 and must not depend on app (GPL-3.0-or-later), normal dependency\n"
    );
    assert_eq!(
        out.stderr,
        format!(
            "cupertino: 2 layer and licence problems in {}\n",
            c.root().display()
        )
    );
}

#[test]
fn outside_a_checkout_is_exit_3_no_checkout() {
    let elsewhere = tempfile::tempdir().unwrap();
    let c = Checkout::new(&[]);
    for (dir, words) in [
        (elsewhere.path(), &["--format", "json", "dev", "layers"][..]),
        (
            c.root(),
            &[
                "--format",
                "json",
                "dev",
                "layers",
                "--root",
                "/nonexistent",
            ][..],
        ),
        (
            c.root(),
            &["--format", "json", "dev", "layers", "--root", "crates/app"][..],
        ),
    ] {
        let out = c.run_in(dir, words);
        assert_eq!(out.code, 3, "{words:?}");
        let v = envelope(&out);
        assert_eq!(v["error"]["kind"], "unavailable");
        assert_eq!(v["error"]["code"], "no_checkout");
    }
    let out = c.run_in(elsewhere.path(), &["dev", "layers"]);
    assert_eq!(out.code, 3);
    assert_eq!(
        out.stderr,
        format!(
            "cupertino: {} is not inside a checkout of the desktop's sources; give one with --root\n",
            elsewhere.path().display()
        )
    );
}

#[test]
fn without_cargo_is_exit_3_no_cargo() {
    let c = Checkout::new(&[]);
    let bare = |cargo: Option<&str>, path: &str| {
        let mut cx = Context::isolated(c.root()).with_var("PATH", path);
        if let Some(cargo) = cargo {
            cx = cx.with_var("CARGO", cargo);
        }
        Runner::new(REGISTRY)
            .context(cx.provide(&POLICY))
            .run(&args(&["--format", "jsonl", "dev", "layers"]), "r1")
    };
    let empty = tempfile::tempdir().unwrap();
    for out in [
        bare(None, ""),
        bare(None, empty.path().to_str().unwrap()),
        bare(Some("/nonexistent/cargo"), ""),
    ] {
        assert_eq!(out.code, 3);
        let records = records(&out, "dev layers");
        assert_eq!(
            records.last().unwrap()["error"]["code"],
            "no_cargo",
            "{records:?}"
        );
    }
}

#[test]
fn a_workspace_cargo_cannot_read_is_exit_5_metadata_failed() {
    let c = Checkout::new(&[]);
    let manifest = c.root().join("crates/app/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replace("layer = \"apps\"", "layer = \"widgets\""),
    )
    .unwrap();
    let out = c.run(&["--format", "json", "dev", "layers"]);
    assert_eq!(out.code, 5);
    let v = envelope(&out);
    assert_eq!(v["error"]["code"], "metadata_failed");
    assert_eq!(
        v["error"]["message"],
        "app: `widgets` is not a layer in RULES"
    );

    std::fs::write(&manifest, "[package\n").unwrap();
    let out = c.run(&["--format", "json", "dev", "layers"]);
    assert_eq!(out.code, 5);
    let v = envelope(&out);
    assert_eq!(v["error"]["code"], "metadata_failed");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("cargo metadata failed: "),
        "{v}"
    );
}

fn manual(dir: &Path, epoch: Option<&str>, words: &[&str]) -> Output {
    let mut cx = Context::isolated(dir);
    if let Some(epoch) = epoch {
        cx = cx.with_var("SOURCE_DATE_EPOCH", epoch);
    }
    Runner::new(REGISTRY).context(cx).run(&args(words), "r1")
}

fn page_names() -> Vec<String> {
    let mut names = vec!["cupertino.1".to_owned()];
    names.extend(REGISTRY.iter().map(|g| format!("cupertino-{}.1", g.name)));
    names
}

#[test]
fn manual_writes_a_page_per_group_and_one_for_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let out = manual(
        dir.path(),
        Some("86400"),
        &["--format", "json", "dev", "manual", "man"],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    let v = envelope(&out);
    let man: PathBuf = dir.path().join("man");
    assert_eq!(v["data"], json!({"dir": man, "pages": page_names()}));
    let mut written: Vec<String> = std::fs::read_dir(&man)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    written.sort();
    let mut expected = page_names();
    expected.sort();
    assert_eq!(written, expected);
    let main = std::fs::read_to_string(man.join("cupertino.1")).unwrap();
    assert!(
        main.starts_with(".TH CUPERTINO 1 1970-01-02 "),
        "SOURCE_DATE_EPOCH dates the page: {main}"
    );

    let human = manual(dir.path(), None, &["dev", "manual", "again"]);
    assert_eq!(human.code, 0);
    let lines: Vec<String> = page_names()
        .iter()
        .map(|n| format!("{}\n", dir.path().join("again").join(n).display()))
        .collect();
    assert_eq!(human.stdout, lines.concat());
    let undated = std::fs::read_to_string(dir.path().join("again/cupertino.1")).unwrap();
    assert!(
        undated.starts_with(".TH CUPERTINO 1 1970-01-01 "),
        "a fixed date without SOURCE_DATE_EPOCH"
    );
}

#[test]
fn manual_into_a_file_is_exit_6_write_failed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("taken"), "").unwrap();
    let out = manual(
        dir.path(),
        None,
        &["--format", "json", "dev", "manual", "taken"],
    );
    assert_eq!(out.code, 6);
    let v: Value = envelope(&out);
    assert_eq!(v["error"]["kind"], "temporary");
    assert_eq!(v["error"]["code"], "write_failed");
}
