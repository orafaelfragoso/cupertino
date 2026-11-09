//! What the command tests share: words to arguments, and the checks every envelope and JSON lines output must pass.

#![allow(dead_code)]

use cupertino_cli::Output;
use serde_json::Value;

pub fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// The one envelope line of a JSON output, after checking nothing went to stderr.
pub fn envelope(out: &Output) -> Value {
    assert_eq!(out.stderr, "", "nothing goes to stderr in JSON");
    assert_eq!(out.stdout.lines().count(), 1, "one line: {}", out.stdout);
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["schema"], "cupertino.cli.v1");
    assert_eq!(v["ok"], out.code == 0, "{v}");
    v
}

/// The records of a JSON lines output, after checking what every record shares: the schema, the command, `seq` from 0
/// without a gap, and one `end`, last, with the exit code.
pub fn records(out: &Output, command: &str) -> Vec<Value> {
    assert_eq!(out.stderr, "", "nothing goes to stderr in JSON lines");
    let records: Vec<Value> = out
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for (seq, r) in records.iter().enumerate() {
        assert_eq!(r["schema"], "cupertino.cli.stream.v1");
        assert_eq!(r["command"], command);
        assert_eq!(r["seq"], seq, "seq is gapless from 0");
    }
    assert_eq!(
        records.iter().filter(|r| r["type"] == "end").count(),
        1,
        "exactly one end: {records:?}"
    );
    let end = records.last().unwrap();
    assert_eq!(end["type"], "end", "end is last");
    assert_eq!(end["exit_code"], out.code);
    assert_eq!(end["ok"], out.code == 0);
    records
}

/// The `type` of each record, with the event name for events: `["event:check", "result", "end"]`.
pub fn kinds(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|r| match r["event"].as_str() {
            Some(event) => format!("event:{event}"),
            None => r["type"].as_str().unwrap().to_owned(),
        })
        .collect()
}
