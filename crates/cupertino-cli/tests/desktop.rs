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
/// - `early`: exits 1 without saying it is ready;
/// - `silent`: never says it is ready, keeps the descriptor open and sleeps until a signal ends it;
/// - `partial`: writes `WAYLAND_DISPLAY=` without the line end, keeps the descriptor open and sleeps.
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
    silent) exec sleep infinity ;;
    partial) printf 'WAYLAND_DISPLAY=wayland-fake' >&"$fd"; exec sleep infinity ;;
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

    /// The stand-ins of this bench still running: the processes whose environment names its `FAKE_ARGS`.
    fn stand_ins(&self) -> Vec<libc::pid_t> {
        let mut mark = b"FAKE_ARGS=".to_vec();
        mark.extend_from_slice(self.path("args").as_os_str().as_encoded_bytes());
        mark.push(0);
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<libc::pid_t>().ok())
            .filter(|pid| {
                std::fs::read(format!("/proc/{pid}/environ"))
                    .is_ok_and(|env| env.windows(mark.len()).any(|w| w == mark))
            })
            // a zombie has an empty environment and holds nothing
            .collect()
    }
}

impl Drop for Bench {
    /// A stand-in that outlived its test, because the test failed or what ends it is broken, is killed: it would hold
    /// the descriptors it inherited (a build tool's lock among them) until the machine restarts.
    fn drop(&mut self) {
        for pid in self.stand_ins() {
            // SAFETY: a signal to a process this bench started, found by the environment only it has.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

/// The installed command run by a process test: killed and reaped when the test ends, also when it fails.
struct Job(std::process::Child);

impl Drop for Job {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

impl std::ops::Deref for Job {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Job {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Bench {
    /// A context with the stand-in as `CUPERTINO_SERVER`, in `mode`, and a runtime directory.
    fn context(&self, mode: &str) -> Context<'static> {
        bench_context(self.dir.path(), mode)
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

/// The context of the bench in `dir`: what [`Bench::context`] gives, for a thread that cannot borrow the bench.
fn bench_context(dir: &std::path::Path, mode: &str) -> Context<'static> {
    Context::isolated(dir)
        .with_var("CUPERTINO_SERVER", dir.join("cupertino-server"))
        .with_var("FAKE_MODE", mode)
        .with_var("FAKE_ARGS", dir.join("args"))
        .with_var("XDG_RUNTIME_DIR", dir.join("runtime"))
        .with_var("PATH", "/usr/bin:/bin")
        .with_var("CUPERTINO_ASSET_SEARCH_PATH", "")
}

fn run(cx: Context, words: &[&str]) -> Output {
    Runner::new(REGISTRY).context(cx).run(&args(words), "r1")
}

/// Standard output that sets `stop` once the event `when` has been written: what Ctrl-C does at that point.
struct StopWhen<'a> {
    out: Vec<u8>,
    stop: &'a StopFlag,
    when: &'static str,
}

impl Write for StopWhen<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.out.extend_from_slice(buf);
        if String::from_utf8_lossy(&self.out).contains(&format!(r#""event":"{}""#, self.when)) {
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
    run_until(cx, words, "ready")
}

/// Runs `words` and asks the command to stop once it wrote the event `when`.
fn run_until(cx: Context, words: &[&str], when: &'static str) -> Output {
    let stop = StopFlag::new();
    let mut out = StopWhen {
        out: Vec::new(),
        stop: &stop,
        when,
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
    let mut child = Job(Command::new(&cli)
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
        .env("XDG_STATE_HOME", b.path("state"))
        .env("CUPERTINO_DIAG_JOURNAL_SOCKET", b.path("journal"))
        .env("FAKE_MODE", "wait")
        .env("FAKE_ARGS", b.path("args"))
        .env("XDG_RUNTIME_DIR", b.path("runtime"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap());
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
    assert_eq!(
        b.stand_ins(),
        Vec::<libc::pid_t>::new(),
        "nothing outlives it"
    );
}

/// A compositor that never says it is ready, or says half of it and keeps the pipe open, is stopped all the same: the
/// command never waits on the pipe or on the process, only for one of them with a bound.
#[test]
fn a_stop_ends_a_compositor_that_never_said_it_was_ready() {
    for mode in ["silent", "partial"] {
        // the bench stays with the test, so its stand-in is killed when the test fails
        let b = Bench::new();
        let dir = b.dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let words = [
                "--format",
                "jsonl",
                "desktop",
                "start",
                "--backend",
                "headless",
            ];
            let _ = tx.send(run_until(bench_context(&dir, mode), &words, "starting"));
        });
        let out = rx
            .recv_timeout(HANG_GUARD)
            .unwrap_or_else(|_| panic!("{mode}: the command hung"));
        assert_eq!(out.code, 0, "{mode}: {}", out.stdout);
        let r = records(&out, "desktop start");
        assert_eq!(
            kinds(&r),
            ["event:starting", "event:exited", "result", "end"],
            "{mode}"
        );
        assert_eq!(r[1]["data"]["signal"], 15, "{mode}: {}", r[1]);
        assert_eq!(r[2]["data"]["wayland_display"], Value::Null, "{mode}");
        assert_eq!(r[3]["reason"], "stopped", "{mode}");
        assert_eq!(b.stand_ins(), Vec::<libc::pid_t>::new(), "{mode}");
    }
}

/// The installed command started like a terminal's job: in a process group of its own, with the stand-in in `mode`.
fn start_job(b: &Bench, mode: &str) -> (Job, mpsc::Receiver<String>) {
    use std::os::unix::process::CommandExt;
    let cli = installed_pair(b);
    let mut child = Job(Command::new(&cli)
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
        .env("XDG_STATE_HOME", b.path("state"))
        .env("CUPERTINO_DIAG_JOURNAL_SOCKET", b.path("journal"))
        .env("FAKE_MODE", mode)
        .env("FAKE_ARGS", b.path("args"))
        .env("XDG_RUNTIME_DIR", b.path("runtime"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap());
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    (child, rx)
}

/// The lines up to and with the `ready` event.
fn until_ready(rx: &mpsc::Receiver<String>) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        let line = rx.recv_timeout(HANG_GUARD).expect("the ready event");
        let ready = line.contains(r#""event":"ready""#);
        lines.push(line);
        if ready {
            return lines;
        }
    }
}

/// Ctrl-C at a terminal sends SIGINT to the foreground process group, the command's. The compositor runs in a group of
/// its own, so it does not get the SIGINT: the command asks it to end with SIGTERM and reports that.
#[test]
fn ctrl_c_at_the_terminal_reaches_the_command_which_ends_the_compositor_with_sigterm() {
    let b = Bench::new();
    let (mut child, rx) = start_job(&b, "wait");
    let mut lines = until_ready(&rx);
    // SAFETY: SIGINT to the process group the command leads, as a terminal sends it.
    assert_eq!(
        unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGINT) },
        0
    );
    while let Ok(line) = rx.recv_timeout(HANG_GUARD) {
        lines.push(line);
    }
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    let records: Vec<Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        records[2]["data"]["signal"], 15,
        "the compositor got the command's SIGTERM, not the terminal's SIGINT: {}",
        records[2]
    );
    assert_eq!(records.last().unwrap()["reason"], "stopped");
    assert_eq!(
        b.stand_ins(),
        Vec::<libc::pid_t>::new(),
        "nothing outlives it"
    );
}

/// A command killed outright cannot ask the compositor to end; its parent-death signal does.
#[test]
fn a_killed_command_takes_the_compositor_with_it() {
    let b = Bench::new();
    let (mut child, rx) = start_job(&b, "wait");
    until_ready(&rx);
    let info: Value = serde_json::from_slice(
        &std::fs::read(b.path("runtime/cupertino/server-wayland-fake.json")).unwrap(),
    )
    .unwrap();
    let server = info["pid"].as_i64().unwrap() as libc::pid_t;
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, server, 0) };
    assert!(pidfd >= 0, "the compositor runs");
    // SAFETY: a signal to the child, which is not reaped yet.
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGKILL) },
        0
    );
    child.wait().unwrap();
    let mut poll = libc::pollfd {
        fd: pidfd as i32,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd; the pidfd becomes readable when the process ends.
    let ended = unsafe { libc::poll(&mut poll, 1, HANG_GUARD.as_millis() as libc::c_int) };
    // SAFETY: closing the pidfd opened above.
    unsafe { libc::close(pidfd as i32) };
    assert_eq!(ended, 1, "the compositor {server} ended with the command");
    assert_eq!(
        b.stand_ins(),
        Vec::<libc::pid_t>::new(),
        "nothing outlives it"
    );
}

/// The compositor gets the standard streams and the ready descriptor alone, whatever the command inherited: a
/// descriptor the command's parent left open (a build tool's lock, say) never reaches it, so a compositor that outlived
/// everything could not hold it. The stand-in has closed the ready descriptor by the time it sleeps.
#[test]
fn the_compositor_inherits_no_descriptor_but_the_ready_one() {
    let b = Bench::new();
    let mut fds = [0; 2];
    // SAFETY: a pipe without close-on-exec, which the command started below inherits, as a build tool's lock is.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (job, rx) = start_job(&b, "wait");
    // SAFETY: closing the pipe this test made; the command has its own copy.
    unsafe {
        libc::close(fds[0]);
        libc::close(fds[1]);
    }
    until_ready(&rx);
    let info: Value = serde_json::from_slice(
        &std::fs::read(b.path("runtime/cupertino/server-wayland-fake.json")).unwrap(),
    )
    .unwrap();
    let server = info["pid"].as_i64().unwrap();
    // the stand-in writes the file as a script, then closes the ready descriptor and becomes `sleep`
    let since = std::time::Instant::now();
    while std::fs::read_to_string(format!("/proc/{server}/comm")).unwrap_or_default() != "sleep\n" {
        assert!(since.elapsed() < HANG_GUARD, "the stand-in never slept");
        std::thread::sleep(Duration::from_millis(5));
    }
    let held = |pid: i64| -> Vec<i32> {
        let mut fds: Vec<i32> = std::fs::read_dir(format!("/proc/{pid}/fd"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_str().unwrap().parse().unwrap())
            .collect();
        fds.sort();
        fds
    };
    assert!(
        held(i64::from(job.id())).contains(&fds[1]),
        "the command itself inherited the pipe, as the test means"
    );
    assert_eq!(held(server), [0, 1, 2], "the compositor holds nothing else");
    drop(job);
    // the guard killed the command; its parent-death signal ends the stand-in, and the bench kills one left over
    let _ = b.stand_ins();
}
