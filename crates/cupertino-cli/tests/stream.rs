//! The stream rules of `--format jsonl` and of stream commands in every form, run in-process on a test registry.

use cupertino_cli::{
    Args, Command, Context, Failure, FailureKind, Group, Output, Reply, Run, Runner, Sink,
    Stability, StopFlag,
};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex};

const COMMANDS: &[Command] = &[
    command(&["t", "once"], Run::Once(once)),
    command(&["t", "events"], Run::Stream(events)),
    command(&["t", "fail"], Run::Stream(fail)),
    command(&["t", "follow"], Run::Stream(follow)),
    command(&["t", "finite"], Run::Stream(finite)),
    command(&["t", "finished"], Run::Stream(finished)),
    command(&["t", "flood"], Run::Stream(flood)),
    command(&["t", "scalar"], Run::Stream(scalar)),
    command(&["t", "report"], Run::Stream(report)),
    command(&["t", "warned"], Run::Stream(warned)),
    Command {
        stability: Stability::Deprecated {
            replacement: "cupertino t once",
        },
        ..command(&["t", "old"], Run::Once(once))
    },
];

const fn command(path: &'static [&'static str], run: Run) -> Command {
    Command {
        path,
        summary: "a test command",
        description: "A test command.",
        examples: &[("cupertino t", "Run it.")],
        stability: Stability::Preview,
        args: &[],
        run,
    }
}

const TEST: &[Group] = &[Group {
    name: "t",
    summary: "test commands",
    description: "Test commands.",
    commands: &[COMMANDS],
}];

fn once(_: &Args, _: &Context) -> Result<Reply, Failure> {
    Ok(Reply::new(&json!({"n": 1}), "one\n"))
}

fn events(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.event("first", &json!({"i": 0}), "event 0")?;
    sink.warn(Failure::new(
        FailureKind::Unavailable,
        "source_missing",
        "one source is missing",
    ))?;
    sink.event("second", &json!({"i": 1}), "event 1\n")?;
    sink.event("quiet", &json!({"i": 2}), "")?;
    Ok(Reply::new(&json!({"count": 3}), "done\n"))
}

fn fail(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.event("first", &json!({}), "event 0")?;
    Err(Failure::new(FailureKind::Validation, "bad", "it failed"))
}

/// Waits for a stop, polling the flag as a real follow stream would between its reads.
fn wait_for_stop(sink: &Sink) {
    while !sink.stop_requested() {
        std::thread::yield_now();
    }
}

fn follow(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.follow();
    sink.event("started", &json!({}), "started")?;
    wait_for_stop(sink);
    Ok(Reply::new(&json!({"seen": 0}), ""))
}

fn finite(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.event("started", &json!({}), "started")?;
    wait_for_stop(sink);
    Err(Failure::interrupted())
}

/// A finite stream that finished its work although a stop came: its result stands.
fn finished(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.event("started", &json!({}), "started")?;
    wait_for_stop(sink);
    Ok(Reply::new(&json!({"done": true}), "done\n"))
}

fn flood(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    for n in 0u64.. {
        sink.event("item", &json!({ "n": n }), &format!("item {n}"))?;
    }
    unreachable!()
}

fn scalar(_: &Args, _: &Context, _: &mut Sink) -> Result<Reply, Failure> {
    Ok(Reply::new(&json!({}), ""))
}

/// A report: its final data holds what its events said.
fn report(_: &Args, _: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.json_result_only();
    sink.event("check", &json!({"id": "a"}), "a: pass")?;
    Ok(Reply::new(
        &json!({"schema": 1, "checks": ["a"]}),
        "report\n",
    ))
}

fn warned(a: &Args, cx: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    sink.json_result_only();
    sink.warn(Failure::new(
        FailureKind::Unavailable,
        "partial",
        "one area was skipped",
    ))?;
    report(a, cx, sink)
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn run(words: &[&str]) -> Output {
    Runner::new(TEST)
        .context(Context::isolated("/nonexistent"))
        .run(&args(words), "r1")
}

/// The records of a JSON lines output, after checking what every record shares: the schema, the command, the request,
/// `seq` from 0 without a gap, and one `end`, last.
fn records(out: &Output, command: &str) -> Vec<Value> {
    assert_eq!(out.stderr, "", "nothing goes to stderr in JSON lines");
    let records: Vec<Value> = out
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for (seq, r) in records.iter().enumerate() {
        assert_eq!(r["schema"], "cupertino.cli.stream.v1");
        assert_eq!(r["command"], command);
        assert_eq!(r["request_id"], "r1");
        assert_eq!(r["seq"], seq, "seq is gapless from 0");
    }
    let ends = records.iter().filter(|r| r["type"] == "end").count();
    assert_eq!(ends, 1, "exactly one end: {records:?}");
    assert_eq!(records.last().unwrap()["type"], "end", "end is last");
    let end = records.last().unwrap();
    assert_eq!(end["exit_code"], out.code);
    assert_eq!(end["ok"], out.code == 0);
    records
}

fn kinds(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|r| r["type"].as_str().unwrap())
        .collect()
}

#[test]
fn an_event_record_carries_its_name_and_data_in_field_order() {
    let out = run(&["--format", "jsonl", "t", "events"]);
    let first = out.stdout.lines().next().unwrap();
    assert_eq!(
        first,
        r#"{"schema":"cupertino.cli.stream.v1","command":"t events","request_id":"r1","seq":0,"type":"event","event":"first","data":{"i":0}}"#
    );
    let records = records(&out, "t events");
    assert_eq!(
        kinds(&records),
        ["event", "error", "event", "event", "result", "end"]
    );
    assert_eq!(
        (&records[2]["event"], &records[2]["data"]),
        (&json!("second"), &json!({"i": 1}))
    );
    assert_eq!(
        records[3]["event"], "quiet",
        "an event without a human line is still a record"
    );
}

#[test]
fn an_error_record_is_a_problem_the_command_continued_past() {
    let out = run(&["--format", "jsonl", "t", "events"]);
    let records = records(&out, "t events");
    assert_eq!(
        records[1],
        json!({
            "schema": "cupertino.cli.stream.v1", "command": "t events", "request_id": "r1", "seq": 1,
            "type": "error",
            "error": {"kind": "unavailable", "code": "source_missing", "message": "one source is missing"}
        })
    );
    assert_eq!(out.code, 0, "a warning does not fail the command");
}

#[test]
fn a_result_record_carries_the_final_data_and_end_closes_the_stream() {
    let out = run(&["--format", "jsonl", "t", "events"]);
    let records = records(&out, "t events");
    assert_eq!(records[4]["data"], json!({"count": 3}));
    let end = out.stdout.lines().last().unwrap();
    assert_eq!(
        end,
        r#"{"schema":"cupertino.cli.stream.v1","command":"t events","request_id":"r1","seq":5,"type":"end","ok":true,"exit_code":0}"#
    );
}

#[test]
fn a_one_shot_command_prints_a_result_and_an_end() {
    let out = run(&["t", "once", "--format=jsonl"]);
    let records = records(&out, "t once");
    assert_eq!(kinds(&records), ["result", "end"]);
    assert_eq!(records[0]["data"], json!({"n": 1}));
    assert_eq!(out.code, 0);
}

#[test]
fn a_usage_error_before_the_command_starts_prints_only_the_end() {
    for (words, command, code) in [
        (
            &["--format", "jsonl", "t", "events", "--nope"][..],
            "t events",
            "unknown_option",
        ),
        (
            &["--format", "jsonl", "t", "nothing"],
            "t nothing",
            "unknown_command",
        ),
        (&["--format", "jsonl", "t"], "t", "missing_subcommand"),
    ] {
        let out = run(words);
        assert_eq!(out.code, 2, "{words:?}");
        let records = records(&out, command);
        assert_eq!(kinds(&records), ["end"]);
        assert_eq!(records[0]["ok"], false);
        assert_eq!(
            (&records[0]["error"]["kind"], &records[0]["error"]["code"]),
            (&json!("invalid_usage"), &json!(code))
        );
    }
}

#[test]
fn a_failed_stream_ends_with_its_error_after_its_events_and_no_result() {
    let out = run(&["--format", "jsonl", "t", "fail"]);
    assert_eq!(out.code, 5);
    let records = records(&out, "t fail");
    assert_eq!(kinds(&records), ["event", "end"]);
    assert_eq!(
        records[1]["error"],
        json!({"kind": "validation", "code": "bad", "message": "it failed"})
    );
}

#[test]
fn in_human_form_events_print_their_lines_warnings_go_to_stderr_and_the_reply_follows() {
    let out = run(&["t", "events"]);
    assert_eq!(
        out,
        Output {
            code: 0,
            stdout: "event 0\nevent 1\ndone\n".into(),
            stderr: "cupertino: warning: one source is missing\n".into(),
        }
    );
    let failed = run(&["t", "fail"]);
    assert_eq!(
        (failed.code, failed.stdout.as_str(), failed.stderr.as_str()),
        (5, "event 0\n", "cupertino: it failed\n")
    );
}

#[test]
fn in_json_form_a_stream_collects_its_events_and_warnings_into_the_envelope() {
    let out = run(&["--format", "json", "t", "events"]);
    assert_eq!(out.stderr, "");
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(out.stdout.lines().count(), 1);
    assert_eq!(
        v,
        json!({
            "schema": "cupertino.cli.v1", "command": "t events", "request_id": "r1", "ok": true,
            "data": {
                "count": 3,
                "events": [
                    {"event": "first", "data": {"i": 0}},
                    {"event": "second", "data": {"i": 1}},
                    {"event": "quiet", "data": {"i": 2}}
                ],
                "warnings": [{"kind": "unavailable", "code": "source_missing", "message": "one source is missing"}]
            }
        })
    );
    let empty = run(&["--format", "json", "t", "scalar"]);
    let v: Value = serde_json::from_str(&empty.stdout).unwrap();
    assert_eq!(v["data"], json!({"events": [], "warnings": []}));
    let failed = run(&["--format", "json", "t", "fail"]);
    let v: Value = serde_json::from_str(&failed.stdout).unwrap();
    assert_eq!(
        (failed.code, &v["ok"], v.get("data")),
        (5, &json!(false), None)
    );
}

#[test]
fn a_deprecated_command_still_runs_with_a_notice_outside_stdout_or_as_an_error_record() {
    let human = run(&["t", "old"]);
    assert_eq!(
        human,
        Output {
            code: 0,
            stdout: "one\n".into(),
            stderr: "cupertino: warning: `t old` is deprecated and will be removed in a later release; use \
                     `cupertino t once`\n"
                .into(),
        }
    );
    let lines = run(&["--format", "jsonl", "t", "old"]);
    let records = records(&lines, "t old");
    assert_eq!(kinds(&records), ["error", "result", "end"]);
    assert_eq!(
        (&records[0]["error"]["kind"], &records[0]["error"]["code"]),
        (&json!("unsupported"), &json!("deprecated"))
    );
    let json = run(&["--format", "json", "t", "old"]);
    assert_eq!((json.code, json.stderr.as_str()), (0, ""));
}

/// A stdout that takes `limit` bytes, then fails every write with `error`.
struct Failing {
    limit: usize,
    written: Vec<u8>,
    error: i32,
}

impl Write for Failing {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = self.limit - self.written.len();
        if room == 0 {
            return Err(io::Error::from_raw_os_error(self.error));
        }
        let n = room.min(buf.len());
        self.written.extend_from_slice(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn run_into(words: &[&str], limit: usize, error: i32) -> (u8, Vec<u8>, String) {
    let mut out = Failing {
        limit,
        written: Vec::new(),
        error,
    };
    let mut err = Vec::new();
    let code = Runner::new(TEST)
        .context(Context::isolated("/nonexistent"))
        .run_to(&args(words), "r1", &mut out, &mut err);
    (code, out.written, String::from_utf8(err).unwrap())
}

#[test]
fn a_closed_pipe_at_any_byte_stops_the_stream_with_exit_0_and_nothing_on_stderr() {
    let whole = run(&["--format", "jsonl", "t", "events"]).stdout;
    let first_line = whole.find('\n').unwrap() + 1;
    for limit in [0, first_line / 2, first_line, whole.len() - 3] {
        let (code, written, stderr) =
            run_into(&["--format", "jsonl", "t", "events"], limit, libc::EPIPE);
        assert_eq!(
            (code, stderr.as_str()),
            (0, ""),
            "closed after {limit} bytes"
        );
        assert_eq!(
            written,
            whole.as_bytes()[..limit],
            "nothing after the close"
        );
    }
    // a stream that never ends by itself ends when its reader goes, in every form that writes as it runs
    for format in ["jsonl", "human"] {
        for limit in [0, 10, 64] {
            let (code, written, stderr) =
                run_into(&["--format", format, "t", "flood"], limit, libc::EPIPE);
            assert_eq!(
                (code, written.len(), stderr.as_str()),
                (0, limit, ""),
                "{format} {limit}"
            );
        }
    }
    // a one-shot command's output and a usage error's end record too
    let (code, _, stderr) = run_into(&["t", "once"], 1, libc::EPIPE);
    assert_eq!((code, stderr.as_str()), (0, ""));
    let (code, _, stderr) = run_into(&["--format", "jsonl", "t", "nope"], 5, libc::EPIPE);
    assert_eq!((code, stderr.as_str()), (0, ""));
}

#[test]
fn a_full_disk_is_exit_6_with_one_stderr_line_in_human_form_only() {
    for format in ["jsonl", "json", "human"] {
        let (code, _, stderr) = run_into(&["--format", format, "t", "events"], 3, libc::ENOSPC);
        assert_eq!(code, 6, "{format}");
        match format {
            "human" => assert_eq!(
                stderr,
                // the first event fails, so the command stops before its warning
                format!(
                    "cupertino: cannot write the output: {}\n",
                    io::Error::from_raw_os_error(libc::ENOSPC)
                )
            ),
            _ => assert_eq!(stderr, "", "{format}"),
        }
    }
    let (code, _, _) = run_into(&["--format", "jsonl", "t", "flood"], 100, libc::ENOSPC);
    assert_eq!(
        code, 6,
        "a stream that never ends by itself stops on a full disk too"
    );
    let (code, _, stderr) = run_into(&["t", "once"], 0, libc::EIO);
    assert_eq!(code, 6);
    assert!(
        stderr.starts_with("cupertino: cannot write the output: "),
        "{stderr}"
    );
}

/// A stdout another thread can wait on.
#[derive(Clone, Default)]
struct Shared(Arc<(Mutex<Vec<u8>>, Condvar)>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 .0.lock().unwrap().extend_from_slice(buf);
        self.0 .1.notify_all();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn wait_for(&self, text: &str) {
        let (lock, changed) = &*self.0;
        let mut bytes = lock.lock().unwrap();
        while !String::from_utf8_lossy(&bytes).contains(text) {
            bytes = changed.wait(bytes).unwrap();
        }
    }

    fn text(&self) -> String {
        String::from_utf8(self.0 .0.lock().unwrap().clone()).unwrap()
    }
}

/// Runs `words` with a stop flag another thread sets once the `started` event is out.
fn stopped(words: &[&str]) -> Output {
    let stop = StopFlag::new();
    let out = Shared::default();
    let watcher = out.clone();
    let mut err = Vec::new();
    let code = std::thread::scope(|scope| {
        let flag = &stop;
        scope.spawn(move || {
            watcher.wait_for("started");
            flag.set();
        });
        Runner::new(TEST)
            .context(Context::isolated("/nonexistent"))
            .stop(&stop)
            .run_to(&args(words), "r1", &mut out.clone(), &mut err)
    });
    Output {
        code,
        stdout: out.text(),
        stderr: String::from_utf8(err).unwrap(),
    }
}

#[test]
fn a_stop_from_another_thread_ends_a_follow_stream_with_reason_stopped_and_exit_0() {
    let out = stopped(&["--format", "jsonl", "t", "follow"]);
    assert_eq!(out.code, 0);
    let records = records(&out, "t follow");
    assert_eq!(kinds(&records), ["event", "result", "end"]);
    assert_eq!(
        records[2],
        json!({
            "schema": "cupertino.cli.stream.v1", "command": "t follow", "request_id": "r1", "seq": 2,
            "type": "end", "ok": true, "exit_code": 0, "reason": "stopped"
        })
    );
    let human = stopped(&["t", "follow"]);
    assert_eq!(
        (human.code, human.stdout.as_str(), human.stderr.as_str()),
        (0, "started\n", "")
    );
}

#[test]
fn a_stop_interrupts_a_finite_stream_with_exit_6() {
    let out = stopped(&["--format", "jsonl", "t", "finite"]);
    assert_eq!(out.code, 6);
    let records = records(&out, "t finite");
    assert_eq!(kinds(&records), ["event", "end"]);
    assert_eq!(
        records[1]["error"],
        json!({"kind": "temporary", "code": "interrupted", "message": "stopped before it finished"})
    );
    assert_eq!(records[1].get("reason"), None);
    let human = stopped(&["t", "finite"]);
    assert_eq!(
        (human.code, human.stderr.as_str()),
        (6, "cupertino: stopped before it finished\n")
    );
}

#[test]
fn a_finite_stream_that_finished_despite_a_stop_reports_its_result_without_a_reason() {
    let out = stopped(&["--format", "jsonl", "t", "finished"]);
    assert_eq!(out.code, 0);
    let records = records(&out, "t finished");
    assert_eq!(records[2].get("reason"), None);
    assert_eq!(records[1]["data"], json!({"done": true}));
}

#[test]
fn the_version_and_the_help_in_json_lines_are_a_result_and_an_end() {
    let version = cupertino_cli::run(&args(&["--format", "jsonl", "--version"]), "r1");
    let version = records(&version, "version");
    assert_eq!(kinds(&version), ["result", "end"]);
    assert_eq!(version[0]["data"]["cli"], env!("CARGO_PKG_VERSION"));
    let help = cupertino_cli::run(&args(&["help", "--format", "jsonl"]), "r1");
    let help = records(&help, "help");
    assert_eq!(kinds(&help), ["result", "end"]);
    assert_eq!(help[0]["data"][0]["path"], "appearance assets check");
}

#[test]
fn a_report_stream_keeps_its_own_data_in_json_form_and_streams_in_the_others() {
    let json_form = run(&["--format", "json", "t", "report"]);
    assert_eq!(
        json_form.stdout,
        "{\"schema\":\"cupertino.cli.v1\",\"command\":\"t report\",\"request_id\":\"r1\",\"ok\":true,\
         \"data\":{\"checks\":[\"a\"],\"schema\":1}}\n"
    );
    let warned = run(&["--format", "json", "t", "warned"]);
    let v: Value = serde_json::from_str(&warned.stdout).unwrap();
    assert_eq!(
        v["data"],
        json!({"schema": 1, "checks": ["a"], "warnings": [
            {"kind": "unavailable", "code": "partial", "message": "one area was skipped"}
        ]}),
        "a warning is never dropped"
    );
    let lines = run(&["--format", "jsonl", "t", "report"]);
    assert_eq!(
        kinds(&records(&lines, "t report")),
        ["event", "result", "end"]
    );
    let human = run(&["t", "report"]);
    assert_eq!(human.stdout, "a: pass\nreport\n");
}
