//! Tests never read the asset packs installed on the machine. Every process a test starts gets the empty asset search
//! path (`CUPERTINO_ASSET_SEARCH_PATH=`, the built-in fallback alone): [`spawn::cleared`] sets it, and every other
//! spawn in the workspace's tests sets it itself, which the source check here keeps true.

#[allow(dead_code)]
#[path = "common/spawn.rs"]
mod spawn;

use std::path::{Path, PathBuf};

/// The variable that replaces the asset search list; empty, it leaves the built-in fallback alone.
const SEARCH_PATH: &str = "CUPERTINO_ASSET_SEARCH_PATH";

/// A pack no test may open: its manifest is not JSON, so a process that searches it reports it on stderr.
fn broken_pack(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("manifest.json"), "not a manifest").unwrap();
}

/// The server's stderr when it starts from `command` with a broken pack at the user pack places of `home`
/// (`$XDG_DATA_HOME` and `~/.local/share`), standing in for an installed system pack. The server reads its packs before
/// its settings, so a start with an unknown appearance ends right after that read.
fn start(mut command: std::process::Command, home: &Path) -> String {
    broken_pack(&home.join(".local/share/cupertino/assets"));
    broken_pack(&home.join("data/cupertino/assets"));
    let output = command
        .args(["--headless", "--appearance", "no-such-appearance"])
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_DATA_DIRS", home.join("data"))
        .env("XDG_RUNTIME_DIR", home)
        .output()
        .expect("the server starts");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("unknown appearance \"no-such-appearance\""),
        "the server ends at its settings:\n{stderr}"
    );
    stderr
}

#[test]
fn a_binary_started_by_the_helper_never_opens_a_pack_on_the_default_search_path() {
    let home = tempfile::tempdir().unwrap();
    let server = env!("CARGO_BIN_EXE_cupertino-server");
    let stderr = start(spawn::cleared(server, home.path()), home.path());
    assert!(!stderr.contains("cupertino-assets"), "{stderr}");
    // the probe sees a search: without the empty search path the same start reports the broken packs
    let mut searching = spawn::cleared(server, home.path());
    searching.env_remove(SEARCH_PATH);
    let stderr = start(searching, home.path());
    assert!(
        stderr.contains("cupertino-assets: skipping the pack"),
        "{stderr}"
    );
}

/// Test sources whose spawns may leave the variable unset, each with the reason: what they start never reads an
/// asset pack.
const EXEMPT: &[(&str, &str)] = &[
    (
        "crates/cupertino-server/tests/asset-isolation.rs",
        "the examples of this check",
    ),
    (
        "crates/cupertino-diagnostics/",
        "the diagnostics command draws nothing",
    ),
    (
        "crates/cupertino-layers/",
        "the layer check reads only Cargo manifests",
    ),
    (
        "crates/cupertino-sessiond/",
        "the session manager draws nothing",
    ),
    (
        "crates/cupertino-wayland-client/",
        "the protocol client's test clients and compositors draw no text or icons of ours",
    ),
];

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The text from `open` (just after an opening parenthesis) to its closing parenthesis.
fn argument(text: &str) -> &str {
    let mut depth = 0;
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return &text[..at],
            ')' => depth -= 1,
            _ => {}
        }
    }
    text
}

/// The spawns in `source` that start something other than a system tool named by a literal and do not set the
/// variable before they start it, each as its line number and argument. The text a spawn is checked in runs from
/// `Command::new(` to its first `.spawn()`, `.output()` or `.status()`, or to the end of the item it is in (a helper
/// that returns the command).
fn unset_spawns(source: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for constructor in ["Command::new(", "Process::new("] {
        for (at, _) in source.match_indices(constructor) {
            let rest = &source[at + constructor.len()..];
            let arg = argument(rest).trim();
            // a system tool (`sleep`, `dbus-send`) or Cargo itself
            if arg.starts_with('"') || arg == r#"env!("CARGO")"# {
                continue;
            }
            let end = [".spawn()", ".output()", ".status()", "\n}"]
                .iter()
                .filter_map(|stop| rest.find(stop))
                .min()
                .unwrap_or(rest.len());
            if !rest[..end].contains(SEARCH_PATH) {
                found.push((source[..at].matches('\n').count() + 1, arg.to_string()));
            }
        }
    }
    found
}

#[test]
fn every_test_spawn_in_the_workspace_sets_the_empty_asset_search_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        rust_files(&krate.unwrap().path().join("tests"), &mut files);
    }
    files.sort();
    assert!(files.len() > 100, "the scan finds the test sources");
    let mut unset = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if EXEMPT.iter().any(|(prefix, _)| name.starts_with(prefix)) {
            continue;
        }
        let source = std::fs::read_to_string(&file).unwrap();
        for (line, arg) in unset_spawns(&source) {
            unset.push(format!("{name}:{line}: Command::new({arg})"));
        }
    }
    assert!(
        unset.is_empty(),
        "these spawns leave {SEARCH_PATH} unset, so the process reads the packs installed on the machine; set it \
         to an empty value (or start through common::spawn::cleared):\n{}",
        unset.join("\n")
    );
}

#[test]
fn the_source_check_finds_a_spawn_without_the_variable() {
    let without = "fn f() {\n    let o = Command::new(env!(\"CARGO_BIN_EXE_x\")).env(\"A\", \"b\").output();\n}\n";
    assert_eq!(
        unset_spawns(without),
        [(2, r#"env!("CARGO_BIN_EXE_x")"#.to_string())]
    );
    let with = "fn f() {\n    let mut c = Command::new(bin());\n    c.env(\"CUPERTINO_ASSET_SEARCH_PATH\", \"\");\n    c.status();\n}\n";
    assert!(unset_spawns(with).is_empty());
    let helper = "fn f() -> Command {\n    let mut c = Process::new(bin());\n    c.env(\"CUPERTINO_ASSET_SEARCH_PATH\", \"\");\n    c\n}\n";
    assert!(unset_spawns(helper).is_empty());
    let late = "fn f() {\n    Command::new(bin()).status();\n    x.env(\"CUPERTINO_ASSET_SEARCH_PATH\", \"\");\n}\n";
    assert_eq!(
        unset_spawns(late).len(),
        1,
        "set after the start is too late"
    );
    assert!(unset_spawns("fn f() { Command::new(\"sleep\").spawn(); }").is_empty());
}
