//! `Wpctl` and `VolumeWorker` against a stand-in program that records its arguments.

use cupertino_pipewire::{Error, Sink, Target, Volume, VolumeWorker, Wpctl};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A directory of its own, so tests that run side by side never share a program or a log.
fn directory(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pipewire-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A program that appends its arguments to `calls` and then runs `body`.
fn program(dir: &std::path::Path, body: &str) -> (Wpctl, PathBuf) {
    let log = dir.join("calls");
    let path = dir.join("wpctl");
    fs::write(
        &path,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\n{body}\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    (Wpctl::new(path), log)
}

fn calls(log: &std::path::Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_volume_is_asked_of_the_default_sink_or_source() {
    let dir = directory("volume");
    let (wpctl, log) = program(&dir, "echo 'Volume: 0.55 [MUTED]'");
    assert_eq!(
        wpctl.volume(Target::DefaultSink).unwrap(),
        Volume {
            level: 0.55,
            muted: true
        }
    );
    wpctl.volume(Target::DefaultSource).unwrap();
    assert_eq!(
        calls(&log),
        [
            "get-volume @DEFAULT_AUDIO_SINK@",
            "get-volume @DEFAULT_AUDIO_SOURCE@"
        ]
    );
}

#[test]
fn a_level_is_clamped_and_written_with_four_decimals() {
    let dir = directory("level");
    let (wpctl, log) = program(&dir, "");
    for level in [0.5, 1.7, -3.0, f64::NAN, 1.0 / 3.0] {
        wpctl.set_level(Target::DefaultSink, level).unwrap();
    }
    assert_eq!(
        calls(&log),
        [
            "set-volume @DEFAULT_AUDIO_SINK@ 0.5000",
            "set-volume @DEFAULT_AUDIO_SINK@ 1.0000",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.0000",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.0000",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.3333",
        ]
    );
}

#[test]
fn muting_and_choosing_defaults_use_the_commands_of_wpctl() {
    let dir = directory("commands");
    let (wpctl, log) = program(&dir, "");
    wpctl.set_muted(Target::DefaultSource, true).unwrap();
    wpctl.set_muted(Target::DefaultSink, false).unwrap();
    wpctl.set_default_sink(52).unwrap();
    wpctl.set_default_source(47).unwrap();
    assert_eq!(
        calls(&log),
        [
            "set-mute @DEFAULT_AUDIO_SOURCE@ 1",
            "set-mute @DEFAULT_AUDIO_SINK@ 0",
            "set-default 52",
            "set-default 47",
        ]
    );
}

#[test]
fn sinks_and_sources_come_from_the_status_listing() {
    let dir = directory("status");
    let (wpctl, log) = program(
        &dir,
        "printf 'Audio\\n ├─ Sinks:\\n │  *   5. Speakers [vol: 1.00]\\n ├─ Sources:\\n │      6. Mic\\n'",
    );
    assert_eq!(
        wpctl.sinks().unwrap(),
        [Sink {
            id: 5,
            name: "Speakers".to_owned(),
            default: true
        }]
    );
    assert_eq!(
        wpctl.sources().unwrap(),
        [Sink {
            id: 6,
            name: "Mic".to_owned(),
            default: false
        }]
    );
    assert_eq!(calls(&log), ["status", "status"]);
}

#[test]
fn a_program_that_fails_reports_its_exit_code_and_trimmed_message() {
    let dir = directory("failure");
    let (wpctl, _) = program(&dir, "echo '  no default node  ' >&2\nexit 3");
    match wpctl.volume(Target::DefaultSink) {
        Err(Error::Failed { code, stderr }) => {
            assert_eq!(code, Some(3));
            assert_eq!(stderr, "no default node");
        }
        other => panic!("{other:?}"),
    }
    let error = wpctl.set_muted(Target::DefaultSink, true).unwrap_err();
    assert_eq!(error.to_string(), "wpctl exited with 3: no default node");
}

#[test]
fn a_program_ended_by_a_signal_has_no_exit_code() {
    let dir = directory("signal");
    let (wpctl, _) = program(&dir, "kill -9 $$");
    let error = wpctl.volume(Target::DefaultSink).unwrap_err();
    assert!(
        matches!(error, Error::Failed { code: None, .. }),
        "{error:?}"
    );
    assert!(error.to_string().starts_with("wpctl was killed"));
}

#[test]
fn output_that_is_not_a_volume_is_unparsed() {
    let dir = directory("unparsed");
    let (wpctl, _) = program(&dir, "echo hello");
    let error = wpctl.volume(Target::DefaultSink).unwrap_err();
    assert!(matches!(&error, Error::Unparsed(text) if text == "hello"));
    assert_eq!(error.to_string(), "unexpected wpctl output \"hello\"");
}

#[test]
fn a_program_that_does_not_answer_in_time_is_stopped() {
    let dir = directory("timeout");
    let (wpctl, _) = program(&dir, "sleep 30");
    let wpctl = wpctl.with_timeout(Duration::from_millis(200));
    let started = Instant::now();
    let error = wpctl.volume(Target::DefaultSink).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10));
    match error {
        Error::Failed { code: None, stderr } => assert_eq!(stderr, "no answer within 200 ms"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_program_that_is_not_there_fails_to_start_and_names_the_program() {
    let dir = directory("absent");
    let wpctl = Wpctl::new(dir.join("no-such-wpctl"));
    let error = wpctl.volume(Target::DefaultSink).unwrap_err();
    assert!(matches!(&error, Error::Spawn { program, .. } if program.ends_with("no-such-wpctl")));
    assert!(error.to_string().starts_with("cannot run "));
    assert!(std::error::Error::source(&error).is_some());
    assert!(std::error::Error::source(&Error::Unparsed(String::new())).is_none());
}

#[test]
fn the_worker_writes_the_last_value_asked_and_mute_only_when_it_changes() {
    let dir = directory("worker-write");
    let (wpctl, log) = program(&dir, "");
    let worker = VolumeWorker::new(wpctl, Target::DefaultSink);
    worker
        .set(Volume {
            level: 0.25,
            muted: false,
        })
        .unwrap();
    worker.wait_idle();
    worker
        .set(Volume {
            level: 0.5,
            muted: false,
        })
        .unwrap();
    worker.wait_idle();
    worker
        .set(Volume {
            level: 0.5,
            muted: true,
        })
        .unwrap();
    worker.wait_idle();
    assert_eq!(
        calls(&log),
        [
            "set-volume @DEFAULT_AUDIO_SINK@ 0.2500",
            "set-mute @DEFAULT_AUDIO_SINK@ 0",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.5000",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.5000",
            "set-mute @DEFAULT_AUDIO_SINK@ 1",
        ]
    );
}

#[test]
fn a_value_asked_shows_in_the_latest_reading_at_once() {
    let dir = directory("worker-latest");
    let (wpctl, _) = program(&dir, "");
    let worker = VolumeWorker::new(wpctl, Target::DefaultSink);
    assert_eq!(worker.latest(), None);
    let asked = Volume {
        level: 0.8,
        muted: false,
    };
    worker.set(asked).unwrap();
    let latest = worker.latest().unwrap();
    assert_eq!(latest.volume, Ok(asked));
    assert_eq!(latest.sinks, None);
}

#[test]
fn a_reading_asked_for_arrives_with_the_sinks_when_they_were_asked_for() {
    let dir = directory("worker-read");
    let (wpctl, log) = program(
        &dir,
        "case \"$1\" in get-volume) echo 'Volume: 0.60';; status) printf 'Audio\\n ├─ Sinks:\\n │  *   5. Speakers\\n';; esac",
    );
    let worker = VolumeWorker::new(wpctl, Target::DefaultSink);
    worker.request_read(false);
    worker.wait_idle();
    let reading = worker.latest().unwrap();
    assert_eq!(
        reading.volume,
        Ok(Volume {
            level: 0.6,
            muted: false
        })
    );
    assert_eq!(reading.sinks, None);

    worker.request_read(true);
    worker.wait_idle();
    let reading = worker.latest().unwrap();
    assert_eq!(
        reading.sinks,
        Some(Ok(vec![Sink {
            id: 5,
            name: "Speakers".to_owned(),
            default: true
        }]))
    );
    // a later reading without sinks keeps the list read before
    worker.request_read(false);
    worker.wait_idle();
    assert!(worker.latest().unwrap().sinks.is_some());
    assert_eq!(
        calls(&log),
        [
            "get-volume @DEFAULT_AUDIO_SINK@",
            "get-volume @DEFAULT_AUDIO_SINK@",
            "status",
            "get-volume @DEFAULT_AUDIO_SINK@"
        ]
    );
}

#[test]
fn a_failed_reading_is_the_message_of_the_program() {
    let dir = directory("worker-failed-read");
    let (wpctl, _) = program(&dir, "echo 'no server' >&2\nexit 1");
    let worker = VolumeWorker::new(wpctl, Target::DefaultSink);
    worker.request_read(false);
    worker.wait_idle();
    assert_eq!(
        worker.latest().unwrap().volume,
        Err("wpctl exited with 1: no server".to_owned())
    );
}

#[test]
fn a_failed_write_is_reported_by_the_next_call_once() {
    let dir = directory("worker-failed-write");
    let (wpctl, _) = program(&dir, "echo 'no server' >&2\nexit 1");
    let worker = VolumeWorker::new(wpctl, Target::DefaultSink);
    let volume = Volume {
        level: 0.5,
        muted: false,
    };
    worker.set(volume).unwrap();
    worker.wait_idle();
    let error = worker.set(volume).unwrap_err();
    assert!(matches!(error, Error::Failed { code: Some(1), .. }));
    worker.wait_idle();
    // the second write failed too; the first error was handed out once
    assert!(worker.set(volume).is_err());
}

#[test]
fn dropping_the_worker_writes_what_is_still_asked() {
    let dir = directory("worker-drop");
    let (wpctl, log) = program(&dir, "");
    let worker = VolumeWorker::new(wpctl, Target::DefaultSource);
    worker
        .set(Volume {
            level: 0.7,
            muted: false,
        })
        .unwrap();
    drop(worker);
    let written = calls(&log);
    assert!(written
        .iter()
        .any(|line| line == "set-volume @DEFAULT_AUDIO_SOURCE@ 0.7000"));
}
