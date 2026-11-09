//! No test reaches the machine's PAM stack or its journal. The binaries that run PAM (`cupertino-login`, the lock screen,
//! and `cupertino-sessiond`, the session manager and its worker) are only ever started under the test shim
//! (`cupertino-test-pam`): it starts every PAM transaction from a private policy (and refuses one without), and refuses
//! syslog, libaudit and every connection to the journal's, syslog's and the real buses' sockets. A test that runs PAM in
//! its own process checks first that the shim is loaded. The source check here keeps that true for every test of the
//! workspace; the runtime checks show the shared helpers and the real binaries carry the shim.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cupertino_test_pam::{
    Pam, PamIsolated, Script, POLICY_VARIABLE, REFUSE_VARIABLE, REPORT_VARIABLE,
};

/// How a test names a binary that runs PAM.
const PAM_BINARIES: [&str; 6] = [
    "\"cupertino-login\"",
    "/cupertino-login\"",
    "CARGO_BIN_EXE_cupertino-login\"",
    "\"cupertino-sessiond\"",
    "/cupertino-sessiond\"",
    "CARGO_BIN_EXE_cupertino-sessiond\"",
];

/// What in a spawn's text puts the process under the shim.
const ISOLATED: [&str; 4] = [
    "cupertino_test_pam",
    "pam_isolated(",
    ".isolate(",
    ".env_words()",
];

/// What runs PAM in the calling process: the lock screen's authenticator, the session worker's entry point, the
/// binding's transaction, or libpam's own start (the binding's conversation trait alone runs nothing).
const IN_PROCESS: [&str; 6] = [
    "Authenticator::spawn(",
    "worker::run(",
    "auth::authenticate(",
    "pam_client2::Context",
    "pam_client2::{",
    "pam_start",
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

/// Where the function `at` is in begins: the start of the last line before `at` that declares one.
fn item_start(source: &str, at: usize) -> usize {
    let mut start = 0;
    let mut offset = 0;
    for line in source[..at].split_inclusive('\n') {
        let code = line.trim_start();
        let code = code
            .strip_prefix("pub(crate) ")
            .or_else(|| code.strip_prefix("pub "))
            .unwrap_or(code);
        if code.starts_with("fn ") || code.starts_with("unsafe fn ") {
            start = offset;
        }
        offset += line.len();
    }
    start
}

/// The names `source` binds to a binary that runs PAM (`let sessiond = dir.join("cupertino-sessiond");`, `const BINARY:
/// &str = env!("CARGO_BIN_EXE_cupertino-sessiond");`).
fn pam_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        // a command built from the binary is the spawn itself, not a name of it
        if !PAM_BINARIES.iter().any(|b| line.contains(b))
            || line.contains("Command::new(")
            || line.contains("Process::new(")
        {
            continue;
        }
        let code = line.trim_start();
        for keyword in ["let mut ", "let ", "const ", "static ", "pub const "] {
            if let Some(rest) = code.strip_prefix(keyword) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    names.push(name);
                }
                break;
            }
        }
    }
    names
}

/// Whether `text` names a binary that runs PAM, directly or by one of `names`.
fn names_pam_binary(text: &str, names: &[String]) -> bool {
    PAM_BINARIES.iter().any(|b| text.contains(b))
        || names.iter().any(|name| {
            text.match_indices(name.as_str()).any(|(at, _)| {
                let before = text[..at].chars().next_back();
                let after = text[at + name.len()..].chars().next();
                let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                !word(before) && !word(after)
            })
        })
}

/// The spawns in `source` that can start a binary that runs PAM without the shim, each as its line number and what it
/// starts. A spawn is a `Command::new(` (to its first `.spawn()`, `.output()` or `.status()`, or the end of the item it
/// is in) or a command vector the server runs (`vec!["env", ...]`, or one given to a `command`, to the end of its item).
/// It can start such a binary when its text, or the function it is in up to it, names one.
fn unisolated_spawns(source: &str) -> Vec<(usize, String)> {
    let line_of = |at: usize| source[..at].matches('\n').count() + 1;
    let names = pam_names(source);
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
            let context = &source[item_start(source, at)..at + constructor.len() + end];
            if names_pam_binary(context, &names) && !ISOLATED.iter().any(|m| text.contains(m)) {
                found.push((line_of(at), format!("Command::new({arg})")));
            }
        }
    }
    for (at, _) in source.match_indices("vec![") {
        let items = bracketed(&source[at + "vec![".len()..], '[', ']');
        let first = items.trim_start();
        let env = first.starts_with(r#""env".into()"#) || first.starts_with(r#""env".to_string()"#);
        let before = source[..at].trim_end();
        let command = before.ends_with("command:")
            || before.ends_with("command =")
            || before.ends_with("let mut command =");
        if !env && !command {
            continue;
        }
        let rest = &source[at..];
        let end = rest.find("\n}").unwrap_or(rest.len());
        let text = &rest[..end];
        let context = &source[item_start(source, at)..at + end];
        if names_pam_binary(context, &names) && !ISOLATED.iter().any(|m| text.contains(m)) {
            found.push((line_of(at), "vec![...]".to_string()));
        }
    }
    found.sort();
    found
}

/// The functions `source` calls with a binary that runs PAM as an argument, by their paths
/// (`real_process("cupertino-login", ...)`, `common::shell::process("cupertino-login", ...)`).
fn helpers_called_with_a_pam_binary(source: &str) -> Vec<String> {
    let mut helpers = Vec::new();
    for binary in ["(\"cupertino-login\"", "(\"cupertino-sessiond\""] {
        for (at, _) in source.match_indices(binary) {
            let path: String = source[..at]
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if !path.is_empty() {
                helpers.push(path);
            }
        }
    }
    helpers.sort();
    helpers.dedup();
    helpers
}

/// The body of the function `name` in `source`, if it defines one.
fn function_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let at = source.find(&format!("fn {name}("))?;
    let open = at + source[at..].find('{')?;
    Some(bracketed(&source[open + 1..], '{', '}'))
}

/// Whether a function body starts a process: a `Command`, or a command vector the server runs.
fn spawns(body: &str) -> bool {
    body.contains("Command::new(")
        || body.contains("Process::new(")
        || body.contains("\"env\".to_string()")
        || body.contains("\"env\".into()")
}

/// Where the helper at `path` (called from `file`) is defined: in `file` itself for a bare name, else in the file of its
/// module in the same crate (`common::shell::process` is `shell.rs` or `shell/mod.rs`).
fn helper_body<'a>(
    sources: &'a [(String, usize, String)],
    file: &str,
    path: &str,
) -> Option<&'a str> {
    let segments: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
    let (name, module) = match segments.as_slice() {
        [.., module, name] => (*name, Some(*module)),
        [name] => (*name, None),
        [] => return None,
    };
    let krate = file.split('/').take(2).collect::<Vec<_>>().join("/");
    sources
        .iter()
        .filter(|(other, _, _)| match module {
            None => other == file,
            Some(module) => {
                other.starts_with(&format!("{krate}/"))
                    && (other.ends_with(&format!("/{module}.rs"))
                        || other.ends_with(&format!("/{module}/mod.rs")))
            }
        })
        .find_map(|(_, _, source)| function_body(source, name))
}

/// Every test source of the workspace: the `tests` directories, and what tests compile of the production files.
fn test_sources() -> Vec<(String, usize, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (mut tests, mut sources) = (Vec::new(), Vec::new());
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        let krate = krate.unwrap().path();
        rust_files(&krate.join("tests"), &mut tests);
        rust_files(&krate.join("src"), &mut sources);
    }
    assert!(tests.len() > 100, "the scan finds the test sources");
    let this = Path::new(file!()).file_name().unwrap();
    let mut checked: Vec<(String, usize, String)> = tests
        .into_iter()
        .filter(|f| f.file_name() != Some(this))
        .map(|f| {
            let source = std::fs::read_to_string(&f).unwrap();
            (
                f.strip_prefix(&root).unwrap().display().to_string(),
                0,
                source,
            )
        })
        .collect();
    for f in sources {
        let source = std::fs::read_to_string(&f).unwrap();
        if let Some((skipped, part)) = test_part(&f, &source) {
            checked.push((
                f.strip_prefix(&root).unwrap().display().to_string(),
                skipped,
                part,
            ));
        }
    }
    checked.sort();
    checked
}

#[test]
fn every_test_that_starts_a_binary_that_runs_pam_starts_it_under_the_shim() {
    let sources = test_sources();
    let mut problems = Vec::new();
    for (name, skipped, source) in &sources {
        for (line, what) in unisolated_spawns(source) {
            problems.push(format!("{name}:{}: {what}", line + skipped));
        }
        for helper in helpers_called_with_a_pam_binary(source) {
            let body = helper_body(&sources, name, &helper);
            if body.is_some_and(|body| spawns(body) && !ISOLATED.iter().any(|m| body.contains(m))) {
                problems.push(format!(
                    "{name}: {helper}(\"cupertino-…\") starts a binary that runs PAM, and {helper} does not load the shim"
                ));
            }
        }
        if IN_PROCESS.iter().any(|call| source.contains(call))
            && !source.contains("require_loaded()")
        {
            problems.push(format!(
                "{name}: runs PAM in the test's own process without cupertino_test_pam::require_loaded()"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "these tests can reach the machine's PAM stack and journal; start the process with \
         cupertino_test_pam (Pam::isolate, PamIsolated::pam_isolated or Pam::env_words), or run PAM in a child \
         started by Pam::child that calls require_loaded():\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_source_check_finds_a_spawn_without_the_shim() {
    let bare = "fn f() {\n    let o = Command::new(env!(\"CARGO_BIN_EXE_cupertino-login\")).arg(\"--greeter\").output();\n}\n";
    assert_eq!(
        unisolated_spawns(bare),
        [(
            2,
            r#"Command::new(env!("CARGO_BIN_EXE_cupertino-login"))"#.to_string()
        )]
    );
    let isolated = "fn f() {\n    let o = Command::new(env!(\"CARGO_BIN_EXE_cupertino-login\")).pam_isolated(&pam).output();\n}\n";
    assert!(unisolated_spawns(isolated).is_empty());
    // a binary named before the spawn, in the same function or by a constant of the file
    let bound = "fn f() {\n    let sessiond = dir.join(\"cupertino-sessiond\");\n    Command::new(&sessiond).arg(\"worker\").spawn();\n}\n";
    assert_eq!(unisolated_spawns(bound).len(), 1);
    let constant = "const BINARY: &str = env!(\"CARGO_BIN_EXE_cupertino-sessiond\");\nfn f() {\n    Command::new(BINARY).spawn();\n}\n";
    assert_eq!(unisolated_spawns(constant).len(), 1);
    let elsewhere = "fn f() {\n    let sessiond = dir.join(\"cupertino-sessiond\");\n}\nfn g() {\n    Command::new(\"sleep\").spawn();\n}\n";
    assert!(
        unisolated_spawns(elsewhere).is_empty(),
        "a spawn in another function"
    );
    // a command vector the server runs
    let vector = "fn f() -> P {\n    P { command: vec![\"env\".into(), dir.join(\"cupertino-login\").display().to_string()] }\n}\n";
    assert_eq!(unisolated_spawns(vector).len(), 1);
    let shimmed = "fn f() -> P {\n    let mut command = vec![\"env\".into()];\n    command.extend(cupertino_test_pam::shared().env_words());\n    command.push(dir.join(\"cupertino-login\").display().to_string());\n    P { command }\n}\n";
    assert!(unisolated_spawns(shimmed).is_empty());
    // a helper given the binary's name must load the shim itself
    let call = "fn t() {\n    let p = real_process(\"cupertino-login\", &bus);\n}\n";
    assert_eq!(helpers_called_with_a_pam_binary(call), ["real_process"]);
    let helper = "fn real_process(binary: &str) -> P {\n    P { command: vec![\"env\".into(), binary.into()] }\n}\n";
    let sources = [
        (
            "crates/x/tests/t.rs".to_owned(),
            0,
            format!("{call}{helper}"),
        ),
        (
            "crates/x/tests/common/shell.rs".to_owned(),
            0,
            helper.replace("real_process", "process"),
        ),
    ];
    let body = helper_body(&sources, "crates/x/tests/t.rs", "real_process").unwrap();
    assert!(spawns(body) && !ISOLATED.iter().any(|m| body.contains(m)));
    let body = helper_body(&sources, "crates/x/tests/t.rs", "common::shell::process").unwrap();
    assert!(spawns(body));
    assert!(
        helper_body(&sources, "crates/y/tests/t.rs", "common::shell::process").is_none(),
        "another crate's helper is not this one"
    );
    // other binaries are not this check's
    assert!(unisolated_spawns(
        "fn f() {\n    Command::new(env!(\"CARGO_BIN_EXE_cupertino-dock\")).spawn();\n}\n"
    )
    .is_empty());
}

/// The shared helpers put every shell process the compositor's tests start under the shim with a private policy.
#[test]
fn the_shell_process_helper_starts_the_lock_screen_under_the_shim() {
    let login = common::shell::process("cupertino-login", "unix:path=/nonexistent", &[]);
    let shim = cupertino_test_pam::shim().display().to_string();
    let program = login.command.last().unwrap().clone();
    assert!(program.ends_with("/cupertino-login"), "{program}");
    for wanted in [
        format!("LD_PRELOAD={shim}"),
        format!(
            "{POLICY_VARIABLE}={}",
            cupertino_test_pam::shared().policy_dir().display()
        ),
        format!(
            "{REPORT_VARIABLE}={}",
            cupertino_test_pam::shared().report_path().display()
        ),
    ] {
        assert!(
            login.command.contains(&wanted),
            "{wanted} in {:?}",
            login.command
        );
    }
}

/// Wait until `pam`'s report says `what`.
fn wait_for_report(pam: &Pam, what: &str) -> String {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let reached = pam.reached();
        if reached.contains(what) {
            return pam.take_reached();
        }
        assert!(Instant::now() < end, "no {what:?} in the report: {reached}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The real lock screen and session manager worker started under the shim run with it: the login screen's connection
/// to its manager is refused when the shim is told to refuse that socket, and the worker's PAM transaction is refused
/// when the policy is missing. What the source check asks of every spawn does keep the binaries off the machine's
/// services.
#[test]
fn the_real_login_binaries_started_under_the_shim_run_with_it() {
    let pam = Pam::new(&Script::default());
    let dir = tempfile::tempdir().unwrap();

    // the login screen's first connection is to its manager's socket
    let socket = dir.path().join("greeter.sock");
    let manager = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    manager.set_nonblocking(true).unwrap();
    let mut login = Command::new(common::shell::bin_dir().join("cupertino-login"))
        .pam_isolated(&pam)
        .arg("--greeter")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("CUPERTINO_LOGIN_SOCK", &socket)
        .env(REFUSE_VARIABLE, &socket)
        .env("WAYLAND_DISPLAY", dir.path().join("no-compositor"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let reached = wait_for_report(&pam, "refused a connection to");
    assert!(reached.contains(&*socket.to_string_lossy()), "{reached}");
    assert!(
        manager.accept().is_err(),
        "the refused connection never arrived"
    );
    let _ = login.kill();
    let _ = login.wait();

    // a worker under the shim with no policy: its transaction is refused before any PAM configuration is read
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    ours.set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let sessiond = common::fixtures::artifact("cupertino-sessiond", "bin", "cupertino-sessiond");
    let mut worker = Command::new(&sessiond)
        .arg("worker")
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .pam_isolated(&pam)
        .env_remove(POLICY_VARIABLE)
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs)))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let begin = r#"{"type":"Begin","service":"cupertino-session","login":"cupertino-guard","tty":"/dev/tty9","vt":9,"class":"user","mode":"Open","command":["/bin/true"],"environment":[],"path":"/usr/bin","screen_reader":false}"#;
    let mut channel = std::io::BufWriter::new(&ours);
    use std::io::Write as _;
    channel
        .write_all(&(begin.len() as u32).to_le_bytes())
        .unwrap();
    channel.write_all(begin.as_bytes()).unwrap();
    channel.flush().unwrap();
    drop(channel);
    let status = worker.wait().unwrap();
    assert!(!status.success(), "the worker gives up");
    let reached = pam.take_reached();
    assert!(
        reached.contains(
            "refused to start PAM service cupertino-session: no private policy directory"
        ),
        "{reached}"
    );
}
