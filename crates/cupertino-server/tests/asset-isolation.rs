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

/// The text from just after an opening bracket (`open`, closed by `close`) to its closing bracket.
fn bracketed(text: &str, open: char, close: char) -> &str {
    let mut depth = 0;
    for (at, c) in text.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return &text[..at];
            }
            depth -= 1;
        }
    }
    text
}

/// What in the text a spawn is checked in says it starts a binary of the workspace: the test binary itself, a binary
/// Cargo built, or a helper that names one.
const WORKSPACE_BINARY: [&str; 3] = ["current_exe", "CARGO_BIN_EXE", "_binary("];

/// The spawns in `source` that start a binary of the workspace and do not set the variable (after any `env_clear()`)
/// before they start it, each
/// as its line number and what it starts. A spawn starts one unless it names a program by a literal (`sleep`,
/// `dbus-send`, Cargo itself) and its text names no binary of the workspace either (a `sh` that runs the test binary
/// does). The text a spawn is checked in runs from `Command::new(` to its first `.spawn()`, `.output()` or `.status()`,
/// or to the end of the item it is in (a helper that returns the command). A shell process the server starts for a test
/// through `env` (a `vec!["env", ...]` command) counts as a spawn whose text is the vector.
fn unset_spawns(source: &str) -> Vec<(usize, String)> {
    let line_of = |at: usize| source[..at].matches('\n').count() + 1;
    let mut found = Vec::new();
    for constructor in ["Command::new(", "Process::new("] {
        for (at, _) in source.match_indices(constructor) {
            let rest = &source[at + constructor.len()..];
            let arg = bracketed(rest, '(', ')').trim();
            let end = [".spawn()", ".output()", ".status()", "\n}"]
                .iter()
                .filter_map(|stop| rest.find(stop))
                .min()
                .unwrap_or(rest.len());
            let text = &rest[..end];
            let literal = arg.starts_with('"') || arg == r#"env!("CARGO")"#;
            if literal && !WORKSPACE_BINARY.iter().any(|b| text.contains(b)) {
                continue;
            }
            // the variable counts only when no `env_clear()` after it takes it away again
            let set = text.rfind(SEARCH_PATH);
            if set.is_none() || text.rfind(".env_clear()") > set {
                found.push((line_of(at), format!("Command::new({arg})")));
            }
        }
    }
    for (at, _) in source.match_indices("vec![") {
        let items = bracketed(&source[at + "vec![".len()..], '[', ']');
        let first = items.trim_start();
        if (first.starts_with(r#""env".into()"#) || first.starts_with(r#""env".to_string()"#))
            && !items.contains(SEARCH_PATH)
        {
            found.push((line_of(at), "vec![\"env\", ...]".to_string()));
        }
    }
    found.sort();
    found
}

/// The part of a production source file that only tests compile: from its first `#[cfg(test)]` module on; all of a
/// `tests.rs` file.
fn test_part(path: &Path, source: &str) -> Option<(usize, String)> {
    if path.file_name().is_some_and(|n| n == "tests.rs") {
        return Some((0, source.to_string()));
    }
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        if line.trim() == "#[cfg(test)]" {
            let after = source[offset + line.len()..].trim_start();
            if after.starts_with("mod ") || after.starts_with("pub mod ") {
                let skipped = source[..offset].matches('\n').count();
                return Some((skipped, source[offset..].to_string()));
            }
        }
        offset += line.len();
    }
    None
}

#[test]
fn every_test_spawn_in_the_workspace_sets_the_empty_asset_search_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (mut tests, mut sources) = (Vec::new(), Vec::new());
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        let krate = krate.unwrap().path();
        rust_files(&krate.join("tests"), &mut tests);
        rust_files(&krate.join("src"), &mut sources);
    }
    assert!(tests.len() > 100, "the scan finds the test sources");
    let this = Path::new(file!()).file_name().unwrap();
    let mut checked: Vec<(PathBuf, usize, String)> = tests
        .into_iter()
        .filter(|f| f.file_name() != Some(this))
        .map(|f| {
            let source = std::fs::read_to_string(&f).unwrap();
            (f, 0, source)
        })
        .collect();
    for f in sources {
        let source = std::fs::read_to_string(&f).unwrap();
        if let Some((skipped, part)) = test_part(&f, &source) {
            checked.push((f, skipped, part));
        }
    }
    checked.sort();
    let mut unset = Vec::new();
    for (file, skipped, source) in &checked {
        let name = file.strip_prefix(&root).unwrap().display().to_string();
        for (line, what) in unset_spawns(source) {
            unset.push(format!("{name}:{}: {what}", line + skipped));
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
        [(2, r#"Command::new(env!("CARGO_BIN_EXE_x"))"#.to_string())]
    );
    let with = "fn f() {\n    let mut c = Command::new(bin());\n    c.env(\"CUPERTINO_ASSET_SEARCH_PATH\", \"\");\n    c.status();\n}\n";
    assert!(unset_spawns(with).is_empty());
    let helper = "fn f() -> Command {\n    let mut c = Process::new(bin());\n    c.env(\"CUPERTINO_ASSET_SEARCH_PATH\", \"\");\n    c\n}\n";
    assert!(unset_spawns(helper).is_empty());
    let cleared = "fn f() {\n    Command::new(bin()).env(\"CUPERTINO_ASSET_SEARCH_PATH\", \"\").env_clear().status();\n}\n";
    assert_eq!(
        unset_spawns(cleared).len(),
        1,
        "a later env_clear() takes it away"
    );
    let late = "fn f() {\n    Command::new(bin()).status();\n    x.env(\"CUPERTINO_ASSET_SEARCH_PATH\", \"\");\n}\n";
    assert_eq!(
        unset_spawns(late).len(),
        1,
        "set after the start is too late"
    );
    assert!(unset_spawns("fn f() { Command::new(\"sleep\").spawn(); }").is_empty());
    // a shell that runs the test binary starts one of the workspace's binaries
    let shell = "fn f() {\n    Command::new(\"sh\").env(\"EXE\", std::env::current_exe().unwrap()).spawn();\n}\n";
    assert_eq!(
        unset_spawns(shell),
        [(2, "Command::new(\"sh\")".to_string())]
    );
    // a shell process the server starts through `env`
    let process = "fn f() -> P {\n    P { command: vec![\n        \"env\".into(),\n        files_binary().display().to_string(),\n    ] }\n}\n";
    assert_eq!(
        unset_spawns(process),
        [(2, "vec![\"env\", ...]".to_string())]
    );
    let isolated = "fn f() -> P {\n    P { command: vec![\"env\".into(), \"CUPERTINO_ASSET_SEARCH_PATH=\".into(), b()] }\n}\n";
    assert!(unset_spawns(isolated).is_empty());
    // only what tests compile of a production file is checked
    let production = "fn run() { Command::new(program()).spawn(); }\n#[cfg(test)]\nmod tests {\n    fn t() { Command::new(std::env::current_exe().unwrap()).status(); }\n}\n";
    let (skipped, part) = test_part(Path::new("lib.rs"), production).unwrap();
    assert_eq!(skipped, 1);
    let found = unset_spawns(&part);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0 + skipped, 4);
}
