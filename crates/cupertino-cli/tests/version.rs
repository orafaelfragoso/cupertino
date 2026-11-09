//! `cupertino version` on a fixture root: every part installed, then each one missing or unreadable.

mod support;

use std::path::PathBuf;

use cupertino_cli::{Context, Output, Runner, REGISTRY};
use serde_json::{json, Value};
use support::{args, envelope, kinds, records};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// The order of the entries, which scripts may rely on.
const NAMES: &[&str] = &[
    "cli",
    "runtime-package",
    "data-package",
    "compositor-running",
    "c-abi",
    "theme",
    "asset-pack",
    "schema-cli",
    "schema-cli-stream",
    "schema-diagnose",
    "os-image",
    "kernel",
];

/// A root with every part installed, and a runtime directory whose compositor is this test process.
struct Machine {
    dir: tempfile::TempDir,
}

impl Machine {
    fn new() -> Self {
        let m = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        m.file(
            "usr/share/cupertino/build-info.json",
            &format!(r#"{{"package": "cupertino", "version": "0.1.0-1", "commit": "{COMMIT}", "source_date_epoch": 1}}"#),
        );
        m.file(
            "usr/share/cupertino/assets/build-info.json",
            r#"{"package": "cupertino-data", "version": "0.1.0-1", "commit": null}"#,
        );
        m.file(
            "usr/lib/pkgconfig/cupertinokit.pc",
            "Name: cupertinokit\nVersion: 0.1.0\n",
        );
        m.file(
            "usr/include/cupertinokit-1/cupertinokit.h",
            "#pragma once\n#define CUPERTINOKIT_ABI_MAJOR 1\n#define CUPERTINOKIT_ABI_MINOR 2\n",
        );
        m.file(
            "usr/share/cupertino/theme/tokens.json",
            r#"{"version": "3", "colors": {}}"#,
        );
        m.file(
            "usr/share/cupertino/assets/manifest.json",
            r#"{"version": 2, "name": "default"}"#,
        );
        m.file(
            "etc/os-release",
            "NAME=\"Cupertino OS\"\nID=cupertino\nIMAGE_ID=desktop\nIMAGE_VERSION=2026.11\n",
        );
        m.file("proc/sys/kernel/osrelease", "7.2.0-test\n");
        let pid = std::process::id();
        std::fs::create_dir_all(m.path(&format!("proc/{pid}"))).unwrap();
        m.file(
            "runtime/cupertino/server-wayland-5.json",
            &format!(
                r#"{{"schema": 1, "version": "0.1.0", "commit": "{COMMIT}", "pid": {pid}, "backend": "headless",
                    "renderer": "gles", "wayland_display": "wayland-5", "started": 1}}"#
            ),
        );
        m
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    fn file(&self, relative: &str, text: &str) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn remove(&self, relative: &str) {
        std::fs::remove_file(self.path(relative)).unwrap();
    }

    fn context(&self) -> Context<'static> {
        Context::isolated(self.dir.path())
            .with_var("WAYLAND_DISPLAY", "wayland-5")
            .with_var("XDG_RUNTIME_DIR", self.path("runtime"))
    }

    fn run(&self, cx: Context, words: &[&str]) -> Output {
        Runner::new(REGISTRY).context(cx).run(&args(words), "r1")
    }

    /// The entries of `cupertino --format json version`, by name.
    fn entries(&self, cx: Context) -> Vec<Value> {
        let out = self.run(cx, &["--format", "json", "version"]);
        assert_eq!(out.code, 0, "a part that is not ok never fails the command");
        let v = envelope(&out);
        let entries = v["data"]["components"].as_array().unwrap().clone();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, NAMES);
        for e in &entries {
            for field in ["name", "version", "source", "status", "detail", "commit"] {
                assert!(e.get(field).is_some(), "{field} in {e}");
            }
        }
        entries
    }

    fn entry(&self, cx: Context, name: &str) -> Value {
        self.entries(cx)
            .into_iter()
            .find(|e| e["name"] == name)
            .unwrap()
    }
}

fn status(e: &Value) -> (&str, &str) {
    (e["status"].as_str().unwrap(), e["detail"].as_str().unwrap())
}

#[test]
fn every_part_installed_is_ok_with_its_version_and_source() {
    let m = Machine::new();
    let e = m.entries(m.context());
    let pid = std::process::id();
    let runtime_file = m.path("runtime/cupertino/server-wayland-5.json");
    assert_eq!(e[0]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(e[0]["source"], "built-in");
    assert_eq!(e[0]["commit"], json!(cupertino_build_info::COMMIT));
    assert_eq!(
        e[1],
        json!({"name": "runtime-package", "version": "0.1.0-1", "source": "/usr/share/cupertino/build-info.json",
               "status": "ok", "detail": "package cupertino", "commit": COMMIT})
    );
    assert_eq!(e[2]["commit"], Value::Null);
    assert_eq!(e[2]["detail"], "package cupertino-data");
    assert_eq!(
        e[3],
        json!({"name": "compositor-running", "version": "0.1.0", "source": runtime_file, "status": "ok",
               "detail": format!("backend headless, renderer gles, process {pid}"), "commit": COMMIT})
    );
    assert_eq!(
        (e[4]["version"].as_str(), status(&e[4])),
        (Some("0.1.0"), ("ok", "header ABI 1.2"))
    );
    assert_eq!(e[5]["version"], "3");
    assert_eq!(
        (e[6]["version"].as_str(), status(&e[6])),
        (Some("2"), ("ok", "default"))
    );
    assert_eq!(e[7]["version"], "cupertino.cli.v1");
    assert_eq!(e[8]["version"], "cupertino.cli.stream.v1");
    assert_eq!(e[9]["version"], "1");
    assert_eq!(
        (e[10]["version"].as_str(), status(&e[10])),
        (Some("2026.11"), ("ok", "Cupertino OS, desktop, cupertino"))
    );
    assert_eq!(e[11]["version"], "7.2.0-test");
}

#[test]
fn the_human_form_is_a_table_and_json_lines_give_the_result() {
    let m = Machine::new();
    let out = m.run(m.context(), &["version"]);
    assert_eq!((out.code, out.stderr.as_str()), (0, ""));
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), NAMES.len());
    // name, version, status, then the short commit and the detail, in columns of 20, 26 and 14
    assert_eq!(
        lines[1],
        format!(
            "{:<20} {:<26} {:<14} 0123456789ab; package cupertino",
            "runtime-package", "0.1.0-1", "ok"
        )
    );
    assert_eq!(
        lines[7],
        format!("{:<20} {:<26} ok", "schema-cli", "cupertino.cli.v1")
    );
    for (line, name) in lines.iter().zip(NAMES) {
        assert!(line.starts_with(name), "{line}");
        assert_eq!(line.trim_end(), *line, "no trailing space");
    }

    let out = m.run(m.context(), &["--format", "jsonl", "version"]);
    let r = records(&out, "version");
    assert_eq!(kinds(&r), ["result", "end"]);
    assert_eq!(
        r[0]["data"]["components"].as_array().unwrap().len(),
        NAMES.len()
    );
}

#[test]
fn a_missing_part_is_not_installed() {
    let m = Machine::new();
    for (file, name) in [
        ("usr/share/cupertino/build-info.json", "runtime-package"),
        ("usr/share/cupertino/assets/build-info.json", "data-package"),
        ("usr/lib/pkgconfig/cupertinokit.pc", "c-abi"),
        ("usr/share/cupertino/theme/tokens.json", "theme"),
        ("usr/share/cupertino/assets/manifest.json", "asset-pack"),
    ] {
        m.remove(file);
        let e = m.entry(m.context(), name);
        assert_eq!(e["status"], "not_installed", "{e}");
        assert_eq!(e["version"], Value::Null);
        assert_eq!(e["detail"], format!("/{file} does not exist"));
    }
    m.remove("usr/include/cupertinokit-1/cupertinokit.h");
    m.file("usr/lib/pkgconfig/cupertinokit.pc", "Version: 0.1.0\n");
    assert_eq!(
        status(&m.entry(m.context(), "c-abi")),
        (
            "not_installed",
            "/usr/include/cupertinokit-1/cupertinokit.h does not exist"
        )
    );
    m.remove("etc/os-release");
    assert_eq!(m.entry(m.context(), "os-image")["status"], "not_installed");
    m.remove("proc/sys/kernel/osrelease");
    assert_eq!(m.entry(m.context(), "kernel")["status"], "unreadable");
}

#[test]
fn a_part_that_cannot_be_parsed_is_unreadable() {
    let m = Machine::new();
    for (file, text, name, detail) in [
        (
            "usr/share/cupertino/build-info.json",
            "not json",
            "runtime-package",
            "is not a JSON object",
        ),
        (
            "usr/share/cupertino/build-info.json",
            "[1]",
            "runtime-package",
            "is not a JSON object",
        ),
        (
            "usr/share/cupertino/build-info.json",
            r#"{"package": "cupertino"}"#,
            "runtime-package",
            "has no version",
        ),
        (
            "usr/share/cupertino/assets/manifest.json",
            r#"{"name": "x"}"#,
            "asset-pack",
            "has no version",
        ),
        (
            "usr/share/cupertino/theme/tokens.json",
            "{",
            "theme",
            "is not a JSON object",
        ),
        (
            "usr/lib/pkgconfig/cupertinokit.pc",
            "Name: cupertinokit\n",
            "c-abi",
            "has no Version",
        ),
    ] {
        m.file(file, text);
        let e = m.entry(m.context(), name);
        assert_eq!(e["status"], "unreadable", "{e}");
        assert_eq!(e["detail"], format!("/{file} {detail}"), "{e}");
    }
    m.file("usr/lib/pkgconfig/cupertinokit.pc", "Version: 0.1.0\n");
    m.file(
        "usr/include/cupertinokit-1/cupertinokit.h",
        "#define CUPERTINOKIT_ABI_MAJOR 1\n",
    );
    assert_eq!(
        status(&m.entry(m.context(), "c-abi")),
        (
            "unreadable",
            "/usr/include/cupertinokit-1/cupertinokit.h has no ABI version"
        )
    );
    m.file("etc/os-release", "# nothing\n");
    assert_eq!(
        status(&m.entry(m.context(), "os-image")),
        ("unreadable", "os-release names nothing")
    );
}

/// A record is read from a file: a commit that is not a hash is shown as it is, and the table never fails on it.
#[test]
fn a_record_whose_commit_is_not_a_hash_still_prints() {
    let m = Machine::new();
    m.file(
        "usr/share/cupertino/build-info.json",
        r#"{"package": "cupertino", "version": "0.1.0-1", "commit": "ééééééééééééééé"}"#,
    );
    let out = m.run(m.context(), &["version"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("éééééééééééé"), "{}", out.stdout);
}

#[test]
fn tokens_without_a_version_are_named_by_their_digest() {
    let m = Machine::new();
    m.file("usr/share/cupertino/theme/tokens.json", "{}");
    let e = m.entry(m.context(), "theme");
    // sha256 of `{}`
    assert_eq!(e["version"], "sha256:44136fa355b3");
    assert_eq!(e["status"], "ok");
}

#[test]
fn the_compositor_is_the_one_of_wayland_display_while_it_runs() {
    let m = Machine::new();
    let file = m.path("runtime/cupertino/server-wayland-5.json");
    let cases: Vec<(Context, &str, String)> = vec![
        (
            Context::isolated(m.dir.path()).with_var("XDG_RUNTIME_DIR", m.path("runtime")),
            "not_running",
            "WAYLAND_DISPLAY is not set".into(),
        ),
        (
            Context::isolated(m.dir.path()).with_var("WAYLAND_DISPLAY", "wayland-5"),
            "not_running",
            "XDG_RUNTIME_DIR is not set".into(),
        ),
        (
            m.context().with_var("WAYLAND_DISPLAY", "wayland-6"),
            "not_running",
            "no Cupertino compositor runs on wayland-6".into(),
        ),
    ];
    for (cx, want, detail) in cases {
        assert_eq!(
            status(&m.entry(cx, "compositor-running")),
            (want, detail.as_str())
        );
    }
    // a display given as a path names the socket by its file name
    let by_path = m
        .context()
        .with_var("WAYLAND_DISPLAY", m.path("runtime/wayland-5"));
    assert_eq!(m.entry(by_path, "compositor-running")["status"], "ok");

    std::fs::remove_dir(m.path(&format!("proc/{}", std::process::id()))).unwrap();
    let gone = m.entry(m.context(), "compositor-running");
    assert_eq!(gone["status"], "not_running");
    assert_eq!(
        gone["detail"],
        format!(
            "the compositor that wrote it, process {}, is gone",
            std::process::id()
        )
    );

    std::fs::write(&file, r#"{"version": "0.1.0"}"#).unwrap();
    let e = m.entry(m.context(), "compositor-running");
    assert_eq!(
        status(&e),
        (
            "unreadable",
            format!("{} has no version or pid", file.display()).as_str()
        )
    );
    std::fs::write(&file, "{").unwrap();
    let e = m.entry(m.context(), "compositor-running");
    assert_eq!(
        status(&e),
        (
            "unreadable",
            format!("{} is not a JSON object", file.display()).as_str()
        )
    );
}

#[test]
fn version_flag_and_command_agree_on_the_cli_version() {
    let m = Machine::new();
    let flag = m.run(m.context(), &["--format", "json", "--version"]);
    let flag = envelope(&flag);
    let entry = m.entry(m.context(), "cli");
    assert_eq!(flag["data"]["cli"], entry["version"]);
    assert_eq!(
        flag["data"].get("commit").cloned().unwrap_or(Value::Null),
        entry["commit"]
    );
}
