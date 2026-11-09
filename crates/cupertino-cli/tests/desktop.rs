//! `cupertino desktop start` with a stand-in compositor: a script that says it is ready on `--ready-fd` as the real
//! one does, then idles, exits or ignores SIGTERM as `FAKE_MODE` says.

mod support;

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use cupertino_cli::{Context, Output, Runner, StopFlag, StopGrace, REGISTRY};
use serde_json::{json, Value};
use support::{args, envelope, kinds, records};

/// How long a process test waits for an event before it fails instead of hanging.
const HANG_GUARD: Duration = Duration::from_secs(60);

/// The stand-in compositor. It writes its arguments to `$FAKE_ARGS`, one per line, then by `FAKE_MODE`:
/// - `wait`: says it is ready, writes a runtime file with its pid, and sleeps until a signal ends it;
/// - `stubborn`: says it is ready and ignores SIGTERM;
/// - `exit-0` and `exit-3`: says it is ready and exits with that status;
/// - `early`: exits 1 without saying it is ready.
const FAKE: &str = r#"#!/bin/bash
# the command passes its own PATH, which may be empty
PATH=/usr/bin:/bin
fd=
while [ $# -gt 0 ]; do
    printf '%s\n' "$1" >> "$FAKE_ARGS"
    if [ "$1" = --ready-fd ]; then fd=$2; printf '%s\n' "$2" >> "$FAKE_ARGS"; shift; fi
    shift
done
ready() {
    printf 'WAYLAND_DISPLAY=wayland-fake\n' >&"$fd"
    eval "exec $fd>&-"
}
case "$FAKE_MODE" in
    wait)
        mkdir -p "$XDG_RUNTIME_DIR/cupertino"
        printf '{"pid": %s}\n' "$$" > "$XDG_RUNTIME_DIR/cupertino/server-wayland-fake.json"
        ready
        exec sleep infinity ;;
    stubborn) trap '' TERM; ready; exec sleep infinity ;;
    exit-0) ready; exit 0 ;;
    exit-3) ready; exit 3 ;;
    early) exit 1 ;;
esac
exit 2
"#;

/// A temporary directory with the stand-in compositor and a runtime directory.
struct Bench {
    dir: tempfile::TempDir,
}

impl Bench {
    fn new() -> Self {
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let fake = dir.path().join("cupertino-server");
        std::fs::write(&fake, FAKE).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(dir.path().join("runtime")).unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// A context with the stand-in as `CUPERTINO_SERVER`, in `mode`, and a runtime directory.
    fn context(&self, mode: &str) -> Context<'static> {
        Context::isolated(self.dir.path())
            .with_var("CUPERTINO_SERVER", self.path("cupertino-server"))
            .with_var("FAKE_MODE", mode)
            .with_var("FAKE_ARGS", self.path("args"))
            .with_var("XDG_RUNTIME_DIR", self.path("runtime"))
            .with_var("PATH", "/usr/bin:/bin")
            .with_var("CUPERTINO_ASSET_SEARCH_PATH", "")
    }

    /// The arguments the stand-in got, its `--ready-fd` number replaced with `N`.
    fn args(&self) -> Vec<String> {
        let text = std::fs::read_to_string(self.path("args")).unwrap_or_default();
        let mut args: Vec<String> = text.lines().map(str::to_owned).collect();
        if let Some(i) = args.iter().position(|a| a == "--ready-fd") {
            args.truncate(i + 1);
            args.push("N".into());
        }
        args
    }
}

fn run(cx: Context, words: &[&str]) -> Output {
    Runner::new(REGISTRY).context(cx).run(&args(words), "r1")
}

/// Standard output that sets `stop` once a `ready` event has been written: what Ctrl-C does once the compositor runs.
struct StopWhenReady<'a> {
    out: Vec<u8>,
    stop: &'a StopFlag,
}

impl Write for StopWhenReady<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.out.extend_from_slice(buf);
        if String::from_utf8_lossy(&self.out).contains(r#""event":"ready""#) {
            self.stop.set();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `words` and asks the command to stop once the compositor is ready.
fn run_until_ready(cx: Context, words: &[&str]) -> Output {
    let stop = StopFlag::new();
    let mut out = StopWhenReady {
        out: Vec::new(),
        stop: &stop,
    };
    let mut err = Vec::new();
    let code = Runner::new(REGISTRY).context(cx).stop(&stop).run_to(
        &args(words),
        "r1",
        &mut out,
        &mut err,
    );
    Output {
        code,
        stdout: String::from_utf8(out.out).unwrap(),
        stderr: String::from_utf8(err).unwrap(),
    }
}

#[test]
fn headless_passes_its_options_and_reports_starting_ready_exited_in_order() {
    let b = Bench::new();
    let out = run_until_ready(
        b.context("wait"),
        &[
            "--format",
            "jsonl",
            "desktop",
            "start",
            "--backend",
            "headless",
            "--size",
            "320x200",
            "--scale",
            "1.5",
            "--spawn",
            "foot -e top",
            "--spawn",
            "true",
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    let records = records(&out, "desktop start");
    assert_eq!(
        kinds(&records),
        [
            "event:starting",
            "event:ready",
            "event:exited",
            "result",
            "end"
        ]
    );
    let args = [
        "--headless",
        "--size",
        "320x200",
        "--scale",
        "1.5",
        "--spawn",
        "foot -e top",
        "--spawn",
        "true",
    ];
    assert_eq!(
        records[0]["data"],
        json!({"backend": "headless", "server": b.path("cupertino-server"), "args": args})
    );
    let mut got = args.map(String::from).to_vec();
    got.extend(["--ready-fd".into(), "N".into()]);
    assert_eq!(b.args(), got, "the ready descriptor comes last");
    assert_eq!(
        records[1]["data"],
        json!({"wayland_display": "wayland-fake"})
    );
    assert_eq!(
        records[2]["data"]["signal"], 15,
        "ended with SIGTERM: {}",
        records[2]
    );
    assert!(records[2]["data"]["seconds"].as_f64().unwrap() >= 0.0);
    assert_eq!(records[2]["data"].get("status"), None);
    assert_eq!(records[3]["data"]["backend"], "headless");
    assert_eq!(records[3]["data"]["wayland_display"], "wayland-fake");
    assert_eq!(records[3]["data"]["signal"], 15);
    assert_eq!(records[4]["reason"], "stopped");
    assert!(
        !b.path("runtime/cupertino/server-wayland-fake.json")
            .exists(),
        "the runtime file of a compositor ended by a signal is removed"
    );
}

#[test]
fn nested_needs_a_host_display_and_runs_the_compositor_in_a_window() {
    let b = Bench::new();
    let out = run(
        b.context("exit-0"),
        &[
            "--format",
            "json",
            "desktop",
            "start",
            "--backend",
            "nested",
        ],
    );
    assert_eq!(out.code, 3);
    assert_eq!(envelope(&out)["error"]["code"], "no_host_display");
    assert_eq!(b.args(), Vec::<String>::new(), "nothing started");

    for (name, value) in [("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")] {
        let _ = std::fs::remove_file(b.path("args"));
        let out = run(
            b.context("exit-0").with_var(name, value),
            &[
                "--format",
                "json",
                "desktop",
                "start",
                "--backend",
                "nested",
            ],
        );
        assert_eq!(out.code, 0, "{}", out.stdout);
        let v = envelope(&out);
        let data = &v["data"];
        assert_eq!(
            (&data["backend"], &data["wayland_display"], &data["status"]),
            (&json!("nested"), &json!("wayland-fake"), &json!(0))
        );
        let events: Vec<&str> = data["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["event"].as_str().unwrap())
            .collect();
        assert_eq!(
            events,
            ["starting", "ready", "exited"],
            "the JSON form keeps the events"
        );
        assert_eq!(b.args(), ["--winit", "--ready-fd", "N"]);
    }
}

#[test]
fn a_compositor_that_ends_on_its_own_ends_the_command() {
    let b = Bench::new();
    let start = &["desktop", "start", "--backend", "headless"];
    let jsonl = |mode: &str| {
        let words: Vec<&str> = ["--format", "jsonl"].iter().chain(start).copied().collect();
        run(b.context(mode), &words)
    };

    let out = jsonl("exit-0");
    assert_eq!(out.code, 0);
    let r = records(&out, "desktop start");
    assert_eq!(
        kinds(&r),
        [
            "event:starting",
            "event:ready",
            "event:exited",
            "result",
            "end"
        ]
    );
    assert_eq!(r[2]["data"]["status"], 0);
    assert_eq!(r[4].get("reason"), None, "it was not stopped");

    let out = jsonl("exit-3");
    assert_eq!(out.code, 3);
    let r = records(&out, "desktop start");
    assert_eq!(
        kinds(&r),
        ["event:starting", "event:ready", "event:exited", "end"]
    );
    assert_eq!(r[3]["error"]["code"], "server_failed");
    assert_eq!(
        r[3]["error"]["message"],
        "the compositor exited with status 3 after it was ready"
    );

    let out = jsonl("early");
    assert_eq!(out.code, 3);
    let r = records(&out, "desktop start");
    assert_eq!(kinds(&r), ["event:starting", "event:exited", "end"]);
    assert_eq!(
        r[2]["error"]["message"],
        "the compositor exited with status 1 before it was ready"
    );

    let out = run(b.context("early"), start);
    assert_eq!(out.code, 3);
    assert!(out.stdout.starts_with(&format!(
        "starting {} --headless\n",
        b.path("cupertino-server").display()
    )));
    assert!(
        out.stdout
            .contains("\nthe compositor exited with status 1 after "),
        "{}",
        out.stdout
    );
    assert_eq!(
        out.stderr,
        "cupertino: the compositor exited with status 1 before it was ready\n"
    );
}

#[test]
fn a_compositor_that_ignores_sigterm_is_killed_after_the_grace() {
    let b = Bench::new();
    let grace = StopGrace(Duration::from_millis(100));
    let out = run_until_ready(
        b.context("stubborn").provide(&grace),
        &[
            "--format",
            "jsonl",
            "desktop",
            "start",
            "--backend",
            "headless",
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stdout);
    let r = records(&out, "desktop start");
    assert_eq!(
        kinds(&r),
        [
            "event:starting",
            "event:ready",
            "event:exited",
            "result",
            "end"
        ]
    );
    assert_eq!(r[2]["data"]["signal"], 9, "killed: {}", r[2]);
    assert_eq!(r[4]["reason"], "stopped");
}

type MakeContext = fn(&Bench) -> Context<'static>;

#[test]
fn each_wrong_start_has_its_code() {
    let b = Bench::new();
    let wait: MakeContext = |b| b.context("wait");
    let cases: &[(&[&str], MakeContext, u8, &str)] = &[
        (&[], wait, 2, "missing_option"),
        (&["--backend", "window"], wait, 2, "invalid_value"),
        (
            &["--backend", "headless", "--size", "0x10"],
            wait,
            2,
            "invalid_value",
        ),
        (
            &["--backend", "headless", "--size", "big"],
            wait,
            2,
            "invalid_value",
        ),
        (
            &["--backend", "headless", "--scale", "0"],
            wait,
            2,
            "invalid_value",
        ),
        (
            &["--backend", "headless", "--scale", "NaN"],
            wait,
            2,
            "invalid_value",
        ),
        (
            &["--backend", "tty", "--size", "10x10"],
            wait,
            2,
            "option_not_for_backend",
        ),
        (
            &["--backend", "tty", "--spawn", "foot"],
            wait,
            2,
            "option_not_for_backend",
        ),
        (
            &["--backend", "tty"],
            |b| b.context("wait").with_var("XDG_SESSION_TYPE", "wayland"),
            4,
            "inside_graphical_session",
        ),
        (
            &["--backend", "tty"],
            |b| b.context("wait").with_var("XDG_SESSION_TYPE", "x11"),
            4,
            "inside_graphical_session",
        ),
        (
            &["--backend", "headless"],
            |b| {
                Context::isolated(b.dir.path())
                    .with_var("CUPERTINO_SERVER", b.path("cupertino-server"))
            },
            3,
            "no_runtime_dir",
        ),
        (
            &["--backend", "headless"],
            |b| {
                b.context("wait")
                    .with_var("CUPERTINO_SERVER", b.path("absent"))
            },
            3,
            "server_not_installed",
        ),
        (
            &["--backend", "headless"],
            // a directory is not a program
            |b| {
                b.context("wait")
                    .with_var("CUPERTINO_SERVER", b.path("runtime"))
            },
            3,
            "server_not_installed",
        ),
        (
            &["--backend", "headless"],
            // neither beside the test program nor in /usr/bin of the context's root
            |b| Context::isolated(b.dir.path()).with_var("XDG_RUNTIME_DIR", b.path("runtime")),
            3,
            "server_not_installed",
        ),
    ];
    for (rest, cx, code, name) in cases {
        let mut words = vec!["--format", "json", "desktop", "start"];
        words.extend_from_slice(rest);
        let out = run(cx(&b), &words);
        assert_eq!(out.code, *code, "{rest:?}: {}", out.stdout);
        assert_eq!(envelope(&out)["error"]["code"], *name, "{rest:?}");
    }
    assert_eq!(
        b.args(),
        Vec::<String>::new(),
        "no case started the compositor"
    );
}

/// Whether the test's standard input is a text console (it answers the keyboard type request).
fn stdin_is_vt() -> bool {
    let mut kind: libc::c_char = 0;
    // SAFETY: KDGKBTYPE (0x4B33) writes one byte into `kind`; on another file it fails and writes nothing.
    unsafe { libc::ioctl(0, 0x4B33, &mut kind) == 0 }
}

#[test]
fn tty_outside_a_text_console_is_not_a_vt() {
    if stdin_is_vt() {
        eprintln!("skipped: the test runs on a text console, where tty would start the session");
        return;
    }
    let b = Bench::new();
    for session in [None, Some("tty")] {
        let mut cx = b.context("wait");
        if let Some(session) = session {
            cx = cx.with_var("XDG_SESSION_TYPE", session);
        }
        let out = run(
            cx,
            &["--format", "jsonl", "desktop", "start", "--backend", "tty"],
        );
        assert_eq!(out.code, 3);
        let r = records(&out, "desktop start");
        assert_eq!(kinds(&r), ["end"]);
        assert_eq!(r[0]["error"]["code"], "not_a_vt");
    }
}

/// The `cupertino` binary linked beside the stand-in compositor, so it finds it as the one installed beside it.
fn installed_pair(b: &Bench) -> PathBuf {
    let cli = b.path("cupertino");
    if std::fs::hard_link(env!("CARGO_BIN_EXE_cupertino"), &cli).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_cupertino"), &cli).unwrap();
    }
    cli
}

#[test]
fn the_installed_command_finds_the_compositor_beside_it_and_forwards_sigterm() {
    let b = Bench::new();
    let cli = installed_pair(&b);
    let mut child = Command::new(&cli)
        .args([
            "--format",
            "jsonl",
            "desktop",
            "start",
            "--backend",
            "headless",
        ])
        .env_clear()
        .env("PATH", "")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env("FAKE_MODE", "wait")
        .env("FAKE_ARGS", b.path("args"))
        .env("XDG_RUNTIME_DIR", b.path("runtime"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut lines = Vec::new();
    loop {
        let line = rx.recv_timeout(HANG_GUARD).expect("the ready event");
        let ready = line.contains(r#""event":"ready""#);
        lines.push(line);
        if ready {
            break;
        }
    }
    // SAFETY: a signal to the child, which is not reaped yet.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    while let Ok(line) = rx.recv_timeout(HANG_GUARD) {
        lines.push(line);
    }
    reader.join().unwrap();
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
    assert_eq!(status.code(), Some(0), "{stderr}");
    assert_eq!(stderr, "", "the stand-in writes nothing");
    let records: Vec<Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        kinds(&records),
        [
            "event:starting",
            "event:ready",
            "event:exited",
            "result",
            "end"
        ]
    );
    assert_eq!(
        records[0]["data"]["server"],
        json!(b.path("cupertino-server"))
    );
    assert_eq!(records[2]["data"]["signal"], 15);
    assert_eq!(records[4]["reason"], "stopped");
    assert!(!b
        .path("runtime/cupertino/server-wayland-fake.json")
        .exists());
}
