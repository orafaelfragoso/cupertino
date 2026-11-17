//! `desktop-control.conf`: the person's answer for each app that asked to control the desktop or capture the screen.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use cupertino_defaults::desktop_control::*;
use cupertino_defaults::lock::FileLock;

fn terminal() -> App {
    App::Desktop("foot".into())
}

fn agent() -> App {
    App::Executable("/home/me/bin/agent".into())
}

#[test]
fn a_line_holds_one_answer_and_the_first_answer_stands() {
    let (c, problems) = DesktopControlConf::parse(
        "# c\nallow = control app:foot\nallow = capture app:foot\ndeny = control exe:/home/me/bin/agent\n\
         allow = control app:foot\n",
    );
    assert_eq!(c.entries.len(), 3);
    assert_eq!(
        c.decision(Service::Control, &terminal()),
        Some(Decision::Allow)
    );
    assert_eq!(
        c.decision(Service::Capture, &terminal()),
        Some(Decision::Allow)
    );
    assert_eq!(c.decision(Service::Control, &agent()), Some(Decision::Deny));
    assert_eq!(c.decision(Service::Capture, &agent()), None);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].line, 5);
    assert!(problems[0].message.contains("already decided"));
}

#[test]
fn a_bad_line_decides_nothing_and_is_reported_by_number() {
    for (line, expected) in [
        ("allow = listen app:foot", "expected `control` or `capture`"),
        ("allow = control foot", "app:<id>"),
        ("allow = control exe:rel", "not an absolute path"),
        ("allow = control app:", "cannot be written"),
        ("allow control", "expected `allow = "),
        ("silence = control app:x", ""),
        ("bare", "expected `allow = "),
        ("allow = control app:a b", ""),
    ] {
        let (c, problems) = DesktopControlConf::parse(&format!("# c\n{line}\n"));
        assert!(c.entries.is_empty(), "{line}");
        assert_eq!(problems.len(), 1, "{line}");
        assert_eq!(problems[0].line, 2, "{line}");
        assert!(
            problems[0].message.contains(expected),
            "{line}: {}",
            problems[0].message
        );
    }
}

#[test]
fn an_answer_is_per_service_and_app_replaced_in_place_and_asked_again_once_forgotten() {
    let mut c = DesktopControlConf::default();
    c.set(Service::Control, &terminal(), Decision::Allow)
        .unwrap();
    c.set(Service::Capture, &terminal(), Decision::Deny)
        .unwrap();
    c.set(Service::Control, &agent(), Decision::Allow).unwrap();
    c.set(Service::Control, &terminal(), Decision::Deny)
        .unwrap();
    assert_eq!(c.entries.len(), 3);
    assert_eq!(
        c.entries[0].decision,
        Decision::Deny,
        "replaced in its place"
    );
    assert_eq!(c.service(Service::Control).count(), 2);
    assert_eq!(c.service(Service::Capture).count(), 1);
    assert!(c.forget(Service::Control, &terminal()));
    assert!(!c.forget(Service::Control, &terminal()));
    assert_eq!(c.decision(Service::Control, &terminal()), None);
    assert_eq!(
        c.decision(Service::Capture, &terminal()),
        Some(Decision::Deny)
    );
}

#[test]
fn an_app_a_line_cannot_hold_is_not_remembered() {
    let mut c = DesktopControlConf::default();
    for (app, rememberable) in [
        (App::Desktop("x".repeat(MAX_APP)), true),
        (App::Desktop(String::new()), false),
        (App::Desktop("a b".into()), false),
        (App::Desktop("a#b".into()), false),
        (App::Desktop("x".repeat(MAX_APP + 1)), false),
        (App::Executable("relative".into()), false),
        (App::Executable("/a b".into()), false),
    ] {
        assert_eq!(app.is_rememberable(), rememberable, "{app:?}");
        assert_eq!(
            c.set(Service::Control, &app, Decision::Allow).is_ok(),
            rememberable,
            "{app:?}"
        );
    }
    assert_eq!(c.entries.len(), 1);
}

#[test]
fn the_file_is_written_private_read_back_and_refused_when_others_can_write_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-control.conf");
    assert_eq!(
        DesktopControlConf::load(&path).unwrap(),
        (DesktopControlConf::default(), vec![]),
        "a missing file decides nothing"
    );
    let bad = DesktopControlConf {
        entries: vec![Entry {
            service: Service::Control,
            app: App::Desktop("a b".into()),
            decision: Decision::Allow,
        }],
    };
    assert_eq!(
        bad.store(&path).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert!(
        !path.exists(),
        "a store of an app a line cannot hold writes nothing"
    );

    let mut c = DesktopControlConf::default();
    c.set(Service::Control, &terminal(), Decision::Allow)
        .unwrap();
    c.set(Service::Capture, &agent(), Decision::Deny).unwrap();
    c.store(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "allow = control app:foot\ndeny = capture exe:/home/me/bin/agent\n"
    );
    assert_eq!(DesktopControlConf::load(&path).unwrap(), (c, vec![]));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
    assert!(DesktopControlConf::load(&path).is_err());
}

#[test]
fn an_update_reads_changes_and_writes_back_and_a_failed_one_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-control.conf");
    let err =
        DesktopControlConf::update::<()>(&path, |_| Err(std::io::Error::other("no"))).unwrap_err();
    assert_eq!(err.to_string(), "no");
    assert!(!path.exists());
    let seen = DesktopControlConf::update(&path, |c| {
        c.set(Service::Control, &terminal(), Decision::Allow)
            .map_err(std::io::Error::other)?;
        Ok(c.entries.len())
    })
    .unwrap();
    assert_eq!(seen, 1);
    let n = DesktopControlConf::update(&path, |c| {
        assert_eq!(c.entries.len(), 1, "the update reads the stored answer");
        c.forget(Service::Control, &terminal());
        Ok(c.entries.len())
    })
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}

#[test]
fn an_update_while_another_writer_keeps_the_lock_fails_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-control.conf");
    let _held = FileLock::acquire(&path).unwrap();
    let start = std::time::Instant::now();
    let err = DesktopControlConf::update(&path, |c| {
        c.set(Service::Control, &terminal(), Decision::Allow)
            .map_err(std::io::Error::other)
    })
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert!(!path.exists());
}

#[test]
fn concurrent_updates_keep_every_answer_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("desktop-control.conf");
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                DesktopControlConf::update(&path, |c| {
                    c.set(
                        Service::Control,
                        &App::Desktop(format!("app{i}")),
                        Decision::Allow,
                    )
                    .map_err(std::io::Error::other)
                })
                .unwrap();
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let (c, problems) = DesktopControlConf::load(&path).unwrap();
    assert!(problems.is_empty());
    assert_eq!(c.entries.len(), 8);
}
