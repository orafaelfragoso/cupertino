//! `Wpctl` and `VolumeWorker` against a stand-in program that records its arguments.

use cupertino_pipewire::{Error, Sink, Target, Volume, VolumeWorker, Wpctl};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A directory of its own, so tests that run side by side never share a program or a log.
fn directory(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pipewire-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The built stand-in program (`examples/wpctl-stand-in.rs`): the `examples` directory of the
/// nearest enclosing directory of this test executable (or of the target tmp directory) that
/// has one, whatever layout cargo gives its build directories.
fn stand_in() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    exe.ancestors()
        .chain(tmp.ancestors())
        .map(|dir| dir.join("examples").join("wpctl-stand-in"))
        .find(|path| path.is_file())
        .expect("examples/wpctl-stand-in is not built; build the examples of cupertino-pipewire")
}

/// A copy of the stand-in program in `dir` that records its arguments in `calls` and follows
/// `behaviour`: lines of `COMMAND<TAB>ACTION[<TAB>TEXT]` (see the example).
fn program(dir: &std::path::Path, behaviour: &str) -> (Wpctl, PathBuf) {
    let path = dir.join("wpctl");
    fs::copy(stand_in(), &path).unwrap();
    fs::write(dir.join("behaviour"), behaviour).unwrap();
    (Wpctl::new(path), dir.join("calls"))
}

fn calls(log: &std::path::Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn each_request_runs_the_wpctl_command_for_its_target() {
    let dir = directory("commands");
    let (wpctl, log) = program(
        &dir,
        "get-volume\tstdout\tVolume: 0.55 [MUTED]\\n\n\
         status\tstdout\tAudio\\n ├─ Sinks:\\n │  *   5. Speakers [vol: 1.00]\\n ├─ Sources:\\n │      6. Mic\\n\n",
    );
    assert_eq!(
        wpctl.volume(Target::DefaultSink).unwrap(),
        Volume {
            level: 0.55,
            muted: true
        }
    );
    wpctl.volume(Target::DefaultSource).unwrap();
    // a level is clamped to zero to one and written with four decimals
    for level in [0.5, 1.7, -3.0, f64::NAN, 1.0 / 3.0] {
        wpctl.set_level(Target::DefaultSink, level).unwrap();
    }
    wpctl.set_muted(Target::DefaultSource, true).unwrap();
    wpctl.set_muted(Target::DefaultSink, false).unwrap();
    wpctl.set_default_sink(52).unwrap();
    wpctl.set_default_source(47).unwrap();
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
    assert_eq!(
        calls(&log),
        [
            "get-volume @DEFAULT_AUDIO_SINK@",
            "get-volume @DEFAULT_AUDIO_SOURCE@",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.5000",
            "set-volume @DEFAULT_AUDIO_SINK@ 1.0000",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.0000",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.0000",
            "set-volume @DEFAULT_AUDIO_SINK@ 0.3333",
            "set-mute @DEFAULT_AUDIO_SOURCE@ 1",
            "set-mute @DEFAULT_AUDIO_SINK@ 0",
            "set-default 52",
            "set-default 47",
            "status",
            "status",
        ]
    );
}

#[test]
fn a_program_that_fails_is_reported_by_how_it_failed() {
    let timeout = Duration::from_millis(200);
    for (name, behaviour) in [
        ("exit", "*\tstderr\t  no default node  \\n\n*\texit\t3\n"),
        ("signal", "*\tabort\n"),
        ("unparsed", "*\tstdout\thello\\n\n"),
        ("timeout", "*\thang\n"),
    ] {
        let dir = directory(&format!("failure-{name}"));
        let (wpctl, _) = program(&dir, behaviour);
        let started = Instant::now();
        let error = wpctl
            .with_timeout(timeout)
            .volume(Target::DefaultSink)
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10), "{name}");
        match (name, &error) {
            ("exit", Error::Failed { code, stderr }) => {
                assert_eq!((*code, stderr.as_str()), (Some(3), "no default node"));
            }
            ("signal", Error::Failed { code, .. }) => assert_eq!(*code, None),
            ("unparsed", Error::Unparsed(text)) => assert_eq!(text, "hello"),
            ("timeout", Error::Failed { code, stderr }) => {
                assert_eq!((*code, stderr.as_str()), (None, "no answer within 200 ms"));
            }
            _ => panic!("{name}: {error:?}"),
        }
    }

    let dir = directory("absent");
    let error = Wpctl::new(dir.join("no-such-wpctl"))
        .volume(Target::DefaultSink)
        .unwrap_err();
    assert!(
        matches!(&error, Error::Spawn { program, .. } if program.ends_with("no-such-wpctl")),
        "{error:?}"
    );
}

#[test]
fn the_worker_shows_a_value_at_once_and_writes_the_level_and_mute_only_when_it_changes() {
    let dir = directory("worker-write");
    let (wpctl, log) = program(&dir, "");
    let worker = VolumeWorker::new(wpctl, Target::DefaultSink);
    assert_eq!(worker.latest(), None);
    for (level, muted) in [(0.25, false), (0.5, false), (0.5, true)] {
        let asked = Volume { level, muted };
        worker.set(asked).unwrap();
        let latest = worker.latest().unwrap();
        assert_eq!(latest.volume, Ok(asked));
        assert_eq!(latest.sinks, None);
        worker.wait_idle();
    }
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
fn a_reading_asked_for_arrives_with_the_sinks_when_they_were_asked_for() {
    let dir = directory("worker-read");
    let (wpctl, log) = program(
        &dir,
        "get-volume\tstdout\tVolume: 0.60\\n\nstatus\tstdout\tAudio\\n ├─ Sinks:\\n │  *   5. Speakers\\n\n",
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
fn the_workers_failures_reach_the_caller_as_a_failed_reading_or_once_from_the_next_set() {
    let dir = directory("worker-failed");
    let (wpctl, _) = program(&dir, "*\tstderr\tno server\\n\n*\texit\t1\n");
    let worker = VolumeWorker::new(wpctl, Target::DefaultSink);
    worker.request_read(false);
    worker.wait_idle();
    assert_eq!(
        worker.latest().unwrap().volume,
        Err("wpctl exited with 1: no server".to_owned())
    );

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
