//! `cupertino diagnose` on fixture machines given through `Context::provide`: the report in each form, the areas, and
//! `--strict`.

mod support;

use std::collections::HashMap;
use std::path::Path;

use cupertino_cli::{Context, Output, Runner, REGISTRY};
use cupertino_diagnostics::{Area, Sources};
use serde_json::json;
use support::{args, envelope, kinds, records};

/// A machine with nothing installed under `dir`, the environment `env`, and no unit installed.
fn sources(dir: &Path, env: &[(&str, &str)]) -> Sources {
    Sources {
        sysfs: dir.join("sys"),
        root: dir.to_path_buf(),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
        unit_state: Box::new(|_, _| None),
    }
}

/// A session whose compositor is gone: nothing answers on its Wayland socket, so `session.wayland` fails.
fn broken_session(dir: &Path) -> Sources {
    let runtime = dir.join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    sources(
        dir,
        &[
            ("XDG_SESSION_ID", "cupertino-test-no-such-session"),
            ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
            ("WAYLAND_DISPLAY", "wayland-gone"),
        ],
    )
}

fn diagnose(sources: &Sources, words: &[&str]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    Runner::new(REGISTRY)
        .context(Context::isolated(dir.path()).provide(sources))
        .run(&args(words), "r1")
}

#[test]
fn the_json_envelope_holds_the_report_the_library_makes() {
    let dir = tempfile::tempdir().unwrap();
    let s = sources(dir.path(), &[]);
    let out = diagnose(&s, &["--format", "json", "diagnose", "--area", "session"]);
    assert_eq!(out.code, 0);
    let v = envelope(&out);
    assert_eq!(v["command"], "diagnose");
    let expected = cupertino_diagnostics::run(&s, &[Area::Session]);
    assert_eq!(v["data"], serde_json::to_value(&expected).unwrap());
    assert_eq!(v["data"]["schema"], 1);
    assert_eq!(
        v["data"]["checks"],
        json!([{"id": "session.logind", "area": "session", "status": "skip",
                "summary": "not run from a logind session"}]),
        "outside a session the session checks are skipped"
    );
}

#[test]
fn the_human_form_is_the_report_text() {
    let dir = tempfile::tempdir().unwrap();
    let s = broken_session(dir.path());
    let out = diagnose(&s, &["diagnose", "--area", "session", "--area", "crash"]);
    assert_eq!((out.code, out.stderr.as_str()), (0, ""));
    let expected = cupertino_diagnostics::run(&s, &[Area::Session, Area::Crash]);
    assert_eq!(out.stdout, expected.to_string());
}

#[test]
fn json_lines_give_each_check_as_an_event_then_the_counts() {
    let dir = tempfile::tempdir().unwrap();
    let s = broken_session(dir.path());
    // the areas run in their own order, whichever order they are given in, and a repeated one runs once
    let out = diagnose(
        &s,
        &[
            "--format", "jsonl", "diagnose", "--area", "crash", "--area", "session", "--area",
            "crash",
        ],
    );
    assert_eq!(out.code, 0);
    let records = records(&out, "diagnose");
    let expected = cupertino_diagnostics::run(&s, &[Area::Session, Area::Crash]);
    let n = expected.checks.len();
    assert!(n >= 2, "{expected}");
    let mut want = vec!["event:check".to_owned(); n];
    want.extend(["result".to_owned(), "end".to_owned()]);
    assert_eq!(kinds(&records), want);
    for (record, check) in records.iter().zip(&expected.checks) {
        assert_eq!(record["data"], serde_json::to_value(check).unwrap());
    }
    let count = |status: &str| {
        expected
            .checks
            .iter()
            .filter(|c| serde_json::to_value(c.status).unwrap() == status)
            .count()
    };
    assert_eq!(
        records[n]["data"],
        json!({"skip": count("skip"), "ok": count("ok"), "warn": count("warn"), "fail": count("fail")})
    );
    assert!(count("fail") >= 1, "the gone compositor fails a check");
}

#[test]
fn strict_makes_a_failed_check_exit_5_in_every_form() {
    let dir = tempfile::tempdir().unwrap();
    let s = broken_session(dir.path());
    let out = diagnose(
        &s,
        &[
            "--format", "json", "diagnose", "--area", "session", "--strict",
        ],
    );
    assert_eq!(out.code, 5);
    let v = envelope(&out);
    assert_eq!(v["error"]["kind"], "validation");
    assert_eq!(v["error"]["code"], "checks_failed");
    assert_eq!(v["error"]["message"], "1 check failed");

    let out = diagnose(
        &s,
        &[
            "--format", "jsonl", "diagnose", "--area", "session", "--strict",
        ],
    );
    assert_eq!(out.code, 5);
    let records = records(&out, "diagnose");
    assert_eq!(records.last().unwrap()["error"]["code"], "checks_failed");
    assert!(records.iter().any(|r| r["data"]["status"] == "fail"));

    let out = diagnose(&s, &["diagnose", "--area", "session", "--strict"]);
    assert_eq!(out.code, 5);
    assert!(out.stdout.contains("session.wayland"), "{}", out.stdout);
    assert_eq!(out.stderr, "cupertino: 1 check failed\n");

    let lenient = diagnose(&s, &["diagnose", "--area", "session"]);
    assert_eq!(
        lenient.code, 0,
        "without --strict a failed check is a finding"
    );

    let quiet = tempfile::tempdir().unwrap();
    let ok = diagnose(
        &sources(quiet.path(), &[]),
        &["diagnose", "--area", "session", "--strict"],
    );
    assert_eq!(ok.code, 0, "skipped checks are not failures");
}

#[test]
fn an_unknown_area_is_invalid_usage() {
    let dir = tempfile::tempdir().unwrap();
    let s = sources(dir.path(), &[]);
    let out = diagnose(&s, &["--format", "json", "diagnose", "--area", "network"]);
    assert_eq!(out.code, 2);
    let v = envelope(&out);
    assert_eq!(v["error"]["kind"], "invalid_usage");
    assert_eq!(v["error"]["code"], "invalid_value");
}

#[test]
fn without_provided_sources_the_context_is_the_machine() {
    // an isolated context: no variables, so no session, and a root without installed files
    let dir = tempfile::tempdir().unwrap();
    let out = Runner::new(REGISTRY)
        .context(Context::isolated(dir.path()))
        .run(
            &args(&["--format", "json", "diagnose", "--area", "session"]),
            "r1",
        );
    assert_eq!(out.code, 0);
    assert_eq!(envelope(&out)["data"]["checks"][0]["status"], "skip");
}
