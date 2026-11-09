//! Tests never write to the user's diagnostic event store or to the system journal. A binary records its events in the
//! user store (`$XDG_STATE_HOME/cupertino/events`, else under `$HOME`) or in the journal (the socket
//! `CUPERTINO_DIAG_JOURNAL_SOCKET` names, else the system's), as its `main` installs it. So every process a test starts
//! that may record events gets a store of the test's own (`XDG_STATE_HOME`, or a cleared environment with a `HOME` of
//! the test's own) and a journal socket of the test's own, as the binary needs: [`spawn::cleared`] sets both, and every
//! other spawn in the workspace's tests sets them itself, which the source check here keeps true.

#[allow(dead_code)]
#[path = "common/spawn.rs"]
mod spawn;

#[allow(dead_code)]
mod fixtures {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ci/test-fixtures.rs"
    ));
}

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

use cupertino_diagnostic_events::{self as diag, journal, Record};

/// Where a binary records its events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Sink {
    Store,
    Journal,
}

/// What a spawn may start and what it sets: `ok` when it keeps every sink its program needs to the test.
fn isolates(text: &str, need: &BTreeSet<Sink>) -> bool {
    // only what is set after the last `env_clear()` counts
    let text = text.rfind(".env_clear()").map_or(text, |at| &text[at..]);
    let cleared = text.starts_with(".env_clear()");
    let isolated = text.contains("isolated(") || text.contains("isolated_vars(");
    let sets = |name: &str| {
        text.match_indices(name).any(|(at, _)| {
            let before = &text[..at];
            let call = before.rfind('(').map_or("", |open| &before[..open]);
            !call.trim_end().ends_with(".env_remove")
        })
    };
    let store = isolated || sets("XDG_STATE_HOME") || (cleared && text.contains("\"HOME\""));
    let journal = isolated || sets("CUPERTINO_DIAG_JOURNAL_SOCKET") || sets("SOCKET_ENV");
    (!need.contains(&Sink::Store) || store) && (!need.contains(&Sink::Journal) || journal)
}

/// Spawns the check knows start no binary that records events, and why.
const NOT_RECORDING: [(&str, &str, &str); 8] = [
    (
        "crates/cupertino-server/src/data_control.rs",
        "Command::new(&sleep)",
        "the system's sleep",
    ),
    (
        "crates/cupertino-server/src/supervisor.rs",
        "vec![...]",
        "the supervisor's unit tests run shell scripts of their own (exit, kill, sleep)",
    ),
    (
        "crates/cupertino-server/src/config.rs",
        "vec![...]",
        "a parsed configuration compared with its value, never started",
    ),
    (
        "crates/cupertino-defaults/src/compositor.rs",
        "vec![...]",
        "a parsed configuration compared with its value, never started",
    ),
    (
        "crates/cupertino-sessiond/tests/secret_service.rs",
        "Command::new(&self.program)",
        "the installed keyring daemon",
    ),
    (
        "crates/cupertino-sessiond/tests/secret_service.rs",
        "Command::new(tool)",
        "the installed keyring's command line tools",
    ),
    (
        "crates/cupertino-wayland-client/tests/mutter.rs",
        "Command::new(binary)",
        "the installed compositor and the client crate's example window",
    ),
    (
        "crates/cupertino-wayland-client/tests/weston.rs",
        "Command::new(simple_window_binary())",
        "the client crate's example window",
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

/// The items of a list's text, split at its top-level commas.
fn top_level_items(list: &str) -> Vec<&str> {
    let (mut items, mut depth, mut start, mut quoted) = (Vec::new(), 0i32, 0, false);
    let mut previous = ' ';
    for (at, c) in list.char_indices() {
        match c {
            '"' if previous != '\\' => quoted = !quoted,
            '(' | '[' | '{' if !quoted => depth += 1,
            ')' | ']' | '}' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                items.push(list[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
        previous = c;
    }
    items.push(list[start..].trim());
    items.retain(|item| !item.is_empty());
    items
}

/// Whether a list item is a plain string literal, maybe converted (`"sleep".into()`).
fn is_literal(item: &str) -> bool {
    let item = [".into()", ".to_string()", ".to_owned()"]
        .iter()
        .find_map(|suffix| item.strip_suffix(suffix))
        .unwrap_or(item);
    item.len() >= 2
        && item.starts_with('"')
        && item.ends_with('"')
        && !item[1..item.len() - 1].contains('"')
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

/// The sink each binary of the workspace records its events in, from the `diag::init(` of its `main`: the crate's
/// `src/main.rs` (the binary named after the crate) and each `src/bin/<name>.rs`.
fn binary_sinks(root: &Path) -> BTreeMap<String, Sink> {
    let mut sinks = BTreeMap::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        let krate = krate.unwrap().path();
        let name = krate.file_name().unwrap().to_string_lossy().into_owned();
        let mut mains = vec![(name, krate.join("src/main.rs"))];
        if let Ok(bins) = std::fs::read_dir(krate.join("src/bin")) {
            for bin in bins {
                let bin = bin.unwrap().path();
                if bin.extension().is_some_and(|e| e == "rs") {
                    let stem = bin.file_stem().unwrap().to_string_lossy().into_owned();
                    mains.push((stem, bin));
                }
            }
        }
        for (binary, main) in mains {
            let Ok(source) = std::fs::read_to_string(&main) else {
                continue;
            };
            if let Some(sink) = sink_of(&source) {
                sinks.insert(binary, sink);
            }
        }
    }
    sinks
}

/// The sink of the `diag::init(` call in `source`.
fn sink_of(source: &str) -> Option<Sink> {
    let at = source.find("diag::init(")?;
    let call = bracketed(&source[at + "diag::init(".len()..], '(', ')');
    Some(if call.contains("Sink::Journal") {
        Sink::Journal
    } else {
        Sink::Store
    })
}

/// The sinks the test code of each crate installs in its own process (`Sink::UserStore`, `Sink::Journal`): such a
/// crate's test binary, started again by a test, records like a program and needs the same keeping.
fn test_binary_sinks(root: &Path) -> BTreeMap<String, BTreeSet<Sink>> {
    let mut out = BTreeMap::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        let krate = krate.unwrap().path();
        let name = krate.file_name().unwrap().to_string_lossy().into_owned();
        let mut files = Vec::new();
        rust_files(&krate.join("tests"), &mut files);
        let mut sources = Vec::new();
        rust_files(&krate.join("src"), &mut sources);
        let this = Path::new(file!()).file_name().unwrap();
        let mut texts: Vec<String> = files
            .iter()
            .filter(|f| f.file_name() != Some(this))
            .map(|f| std::fs::read_to_string(f).unwrap())
            .collect();
        for f in sources {
            let source = std::fs::read_to_string(&f).unwrap();
            if let Some((_, part)) = test_part(&f, &source) {
                texts.push(part);
            }
        }
        let mut sinks = BTreeSet::new();
        for text in &texts {
            // given as the sink of an emitter: `Sink::UserStore)`
            if text.contains("Sink::UserStore)") {
                sinks.insert(Sink::Store);
            }
            if text.contains("Sink::Journal)") {
                sinks.insert(Sink::Journal);
            }
        }
        if !sinks.is_empty() {
            out.insert(name, sinks);
        }
    }
    out
}

/// The sinks of the binaries `text` names: `"<binary>"` or `CARGO_BIN_EXE_<binary>"`.
fn named(text: &str, sinks: &BTreeMap<String, Sink>) -> BTreeSet<Sink> {
    sinks
        .iter()
        .filter(|(binary, _)| {
            text.contains(&format!("\"{binary}\""))
                || text.contains(&format!("CARGO_BIN_EXE_{binary}\""))
        })
        .map(|(_, sink)| *sink)
        .collect()
}

/// The spawns in `source` that may start a binary recording events into a sink they do not keep to the test, each as
/// its line number and what it starts. `test_binary` is what this crate's test binary records into, when a test starts
/// it again.
///
/// - A spawn's text runs from `Command::new(` (or `Process::new(`) to its first `.spawn()`, `.output()` or `.status()`,
///   or to the end of the item it is in (a helper that returns the command). Its program needs what the binaries its
///   text names need; a program given by a constant (`const BINARY: &str = env!("CARGO_BIN_EXE_…")`) is looked up. A
///   literal program, the test binary and a fixture need nothing more; any other program (a path built at run time)
///   needs both.
/// - A command vector the server starts (`vec!["env", …]`, or one given to a `command`) is a spawn whose text runs to the
///   end of the item it is in (a `splice` after it counts). Its program needs both unless every item is a literal or a
///   `NAME=value` setting.
fn unisolated(
    source: &str,
    sinks: &BTreeMap<String, Sink>,
    test_binary: &BTreeSet<Sink>,
) -> Vec<(usize, String)> {
    let line_of = |at: usize| source[..at].matches('\n').count() + 1;
    let both: BTreeSet<Sink> = [Sink::Store, Sink::Journal].into();
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
            let program = constant(source, arg.trim_start_matches('&')).unwrap_or(arg);
            let mut need = named(text, sinks);
            need.extend(named(program, sinks));
            if program.contains("current_exe") {
                need.extend(test_binary);
            } else if need.is_empty()
                && !(program.starts_with('"')
                    || program.starts_with("env!(\"CARGO")
                    || program.contains("fixtures::artifact("))
            {
                // a path built at run time may be any binary
                need = both.clone();
            }
            if !isolates(text, &need) {
                found.push((line_of(at), format!("{constructor}{arg})")));
            }
        }
    }
    for (at, _) in source.match_indices("vec![") {
        let items = bracketed(&source[at + "vec![".len()..], '[', ']');
        let list = top_level_items(items);
        let env = list
            .first()
            .is_some_and(|first| is_literal(first) && first.starts_with("\"env\""));
        let before = source[..at].trim_end();
        let command = before.ends_with("command:") || before.ends_with("command =");
        if !env && !command {
            continue;
        }
        let rest = &source[at..];
        let text = &rest[..rest.find("\n}").unwrap_or(rest.len())];
        let mut need = named(items, sinks);
        let program = list
            .iter()
            .any(|item| !is_literal(item) && !item.contains('=') && !item.starts_with("env("));
        if program && need.is_empty() {
            need = both.clone();
        }
        if !isolates(text, &need) {
            found.push((line_of(at), "vec![...]".to_string()));
        }
    }
    found.sort();
    found
}

/// The value of `const <name>: &str = <value>;` in `source`.
fn constant<'s>(source: &'s str, name: &str) -> Option<&'s str> {
    let at = source.find(&format!("const {name}: &str = "))?;
    let value = &source[at..];
    let value = &value[value.find("= ")? + 2..];
    Some(value[..value.find(';')?].trim())
}

#[test]
fn every_test_spawn_in_the_workspace_keeps_its_diagnostic_events_to_the_test() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sinks = binary_sinks(&root);
    // the check knows which binaries record and where
    assert_eq!(sinks.get("cupertino-server"), Some(&Sink::Store));
    assert_eq!(sinks.get("cupertino-sessiond"), Some(&Sink::Journal));
    assert!(sinks.len() >= 18, "{sinks:?}");
    let test_binaries = test_binary_sinks(&root);
    let (mut tests, mut sources) = (Vec::new(), Vec::new());
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        let krate = krate.unwrap().path();
        rust_files(&krate.join("tests"), &mut tests);
        rust_files(&krate.join("src"), &mut sources);
    }
    assert!(tests.len() > 100, "the scan finds the test sources");
    // this check and the asset one hold example sources, not spawns
    let checks = [
        Path::new(file!()).file_name().unwrap(),
        std::ffi::OsStr::new("asset-isolation.rs"),
    ];
    let mut checked: Vec<(PathBuf, usize, String)> = tests
        .into_iter()
        .filter(|f| !checks.iter().any(|c| f.file_name() == Some(*c)))
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
    let mut known = BTreeSet::new();
    for (file, skipped, source) in &checked {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .display()
            .to_string()
            .replace("cupertino-server/../../", "");
        let krate = name.split('/').nth(1).unwrap_or_default();
        let test_binary = test_binaries.get(krate).cloned().unwrap_or_default();
        for (line, what) in unisolated(source, &sinks, &test_binary) {
            match NOT_RECORDING
                .iter()
                .find(|(f, w, _)| name == *f && what == *w)
            {
                Some(entry) => {
                    known.insert(*entry);
                }
                None => unset.push(format!("{name}:{}: {what}", line + skipped)),
            }
        }
    }
    assert!(
        unset.is_empty(),
        "these spawns may start a binary that records diagnostic events without a store and a journal socket of the \
         test's own, so its events reach the user's store or the system journal; set XDG_STATE_HOME and \
         CUPERTINO_DIAG_JOURNAL_SOCKET (common::events::isolated, or start through common::spawn::cleared):\n{}",
        unset.join("\n")
    );
    let stale: Vec<_> = NOT_RECORDING
        .iter()
        .filter(|entry| !known.contains(*entry))
        .collect();
    assert!(
        stale.is_empty(),
        "these exceptions no longer match a spawn: {stale:?}"
    );
}

#[test]
fn the_source_check_finds_a_spawn_that_does_not_keep_its_events() {
    let sinks: BTreeMap<String, Sink> = [
        ("cupertino-dock".to_string(), Sink::Store),
        ("cupertino-netd".to_string(), Sink::Journal),
    ]
    .into();
    let none = BTreeSet::new();
    let check = |source: &str| unisolated(source, &sinks, &none);
    let store = "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-dock\")).env(\"A\", \"b\").output();\n}\n";
    assert_eq!(check(store).len(), 1, "a store binary without a store");
    let kept = "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-dock\")).env(\"XDG_STATE_HOME\", d).output();\n}\n";
    assert!(check(kept).is_empty());
    let home = "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-dock\")).env_clear().env(\"HOME\", h).output();\n}\n";
    assert!(
        check(home).is_empty(),
        "a cleared environment with a home of the test's"
    );
    let inherited = "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-dock\")).env(\"HOME\", h).output();\n}\n";
    assert_eq!(
        check(inherited).len(),
        1,
        "XDG_STATE_HOME may come from the caller"
    );
    let cleared_after = "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-dock\")).env(\"XDG_STATE_HOME\", d).env_clear().output();\n}\n";
    assert_eq!(
        check(cleared_after).len(),
        1,
        "a later env_clear() takes it away"
    );
    let journal = "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-netd\")).env(\"XDG_STATE_HOME\", d).output();\n}\n";
    assert_eq!(check(journal).len(), 1, "a journal binary without a socket");
    let socket = "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-netd\")).env(journal::SOCKET_ENV, p).output();\n}\n";
    assert!(check(socket).is_empty());
    let removed = "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-netd\")).env_remove(journal::SOCKET_ENV).output();\n}\n";
    assert_eq!(
        check(removed).len(),
        1,
        "removing the socket is not keeping it"
    );
    let constant = "const BINARY: &str = env!(\"CARGO_BIN_EXE_cupertino-netd\");\nfn f() {\n    Command::new(BINARY).output();\n}\n";
    assert_eq!(check(constant), [(3, "Command::new(BINARY)".to_string())]);
    let other =
        "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-layers\")).output();\n}\n";
    assert!(check(other).is_empty(), "a binary that records nothing");
    assert!(check("fn f() { Command::new(\"sleep\").spawn(); }").is_empty());
    let built = "fn f(p: &Path) {\n    Command::new(p.join(\"x\")).env(\"XDG_STATE_HOME\", d).output();\n}\n";
    assert_eq!(check(built).len(), 1, "a path built at run time needs both");
    let helper = "fn f() -> Command {\n    let mut c = Command::new(bin());\n    c.envs(common::events::isolated_vars());\n    c\n}\n";
    assert!(check(helper).is_empty());
    // the test binary records only when its tests install a sink of their own
    let again = "fn f() {\n    Command::new(std::env::current_exe().unwrap()).output();\n}\n";
    assert!(check(again).is_empty());
    let recording: BTreeSet<Sink> = [Sink::Store].into();
    assert_eq!(unisolated(again, &sinks, &recording).len(), 1);
    // a process the server starts from a command vector
    let process = "fn f() -> P {\n    P { command: vec![\n        \"env\".into(),\n        files_binary().display().to_string(),\n    ] }\n}\n";
    assert_eq!(check(process), [(2, "vec![...]".to_string())]);
    let isolated = "fn f() -> P {\n    P { command: [vec![\"env\".into()], common::events::isolated().to_vec(), vec![b()]].concat() }\n}\n";
    assert!(check(isolated).is_empty());
    let spliced = "fn f(s: &mut S) {\n    s.screenshot_command = vec![\"env\".into(), b()];\n    s.screenshot_command.splice(1..1, common::events::isolated());\n}\n";
    assert!(
        check(spliced).is_empty(),
        "isolation spliced in later counts"
    );
    let named =
        "fn f() -> P {\n    P { command: vec![\"cupertino-dock\".into(), \"--bar\".into()] }\n}\n";
    assert_eq!(check(named).len(), 1, "a recording binary found on PATH");
    let fixed = "fn f() -> P {\n    P { command: vec![\"sleep\".into(), \"30\".into()] }\n}\n";
    assert!(check(fixed).is_empty());
    let data = "fn f() { let v = vec![a(), b()]; }";
    assert!(check(data).is_empty(), "a vector that is no command");
}

/// A binary started through [`spawn::cleared`] records into the store under the test's home and the test's journal
/// socket: the server (a store binary) asked to panic, and the session manager (a journal binary) given a wrong
/// argument.
#[test]
fn a_binary_started_through_the_helper_records_into_the_tests_own_store_and_journal() {
    let home = tempfile::tempdir().unwrap();
    let server = env!("CARGO_BIN_EXE_cupertino-server");
    let out = spawn::cleared(server, home.path())
        .env(diag::TEST_PANIC_ENV, "server")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(101));
    let store = home.path().join(".local/state/cupertino/events");
    let records = diag::store::read(&store).unwrap().records;
    assert!(
        records.iter().any(|r| r.record.id == "server.panic"),
        "the panic is in the test's store: {records:?}"
    );

    // the journal socket of this test process, read back here
    let sessiond = fixtures::artifact("cupertino-sessiond", "bin", "cupertino-sessiond");
    let out = spawn::cleared(&sessiond, home.path())
        .arg("--no-such-argument")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("diagnostic event"),
        "the event reached the socket: {stderr}"
    );
    let socket = spawn::diagnostic_journal_socket();
    socket.set_nonblocking(true).unwrap();
    let mut buf = vec![0u8; 65536];
    let n = socket.recv(&mut buf).unwrap();
    let fields = journal::parse(&buf[..n]).unwrap();
    let json = &fields
        .iter()
        .find(|(k, _)| k == "CUPERTINO_DIAG_JSON")
        .unwrap()
        .1;
    let record = Record::parse(std::str::from_utf8(json).unwrap()).unwrap();
    assert_eq!(record.id, "login.exit_error");
    assert!(spawn::diagnostic_journal().exists());
}

/// Set in the child process of [`a_test_process_removes_its_journal_socket_when_it_ends`].
const CHILD: &str = "DIAGNOSTIC_ISOLATION_CHILD";

/// Runs [`a_test_process_removes_its_journal_socket_when_it_ends`] in a child that binds its journal socket and ends;
/// the socket's path, as it printed it.
fn child_journal() -> PathBuf {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_test_process_removes_its_journal_socket_when_it_ends",
            "--nocapture",
        ])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("JOURNAL="))
        .unwrap_or_else(|| panic!("the child names its socket: {stdout}"));
    PathBuf::from(path)
}

#[test]
fn a_test_process_removes_its_journal_socket_when_it_ends() {
    if std::env::var_os(CHILD).is_some() {
        let journal = spawn::diagnostic_journal();
        assert!(journal.exists());
        println!("JOURNAL={}", journal.display());
        return;
    }
    let journal = child_journal();
    assert!(!journal.exists(), "{} is left behind", journal.display());
    // the socket of a process killed before it could remove its own goes when the next test process starts
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    let stale = journal.with_file_name(format!("journal-{pid}"));
    drop(UnixDatagram::bind(&stale).unwrap());
    assert!(stale.exists());
    let journal = child_journal();
    assert!(!journal.exists());
    assert!(!stale.exists(), "{} is left behind", stale.display());
}
