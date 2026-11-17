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
fn a_line_holds_one_answer_and_anything_else_is_a_problem() {
    let (c, problems) = DesktopControlConf::parse(
        "# c\nallow = control app:foot\nallow = capture app:foot\ndeny = control exe:/home/me/bin/agent\n\
         allow = control app:foot\nallow = listen app:foot\nallow = control foot\nallow = control exe:rel\n\
         allow = control app:\nallow control\nsilence = control app:x\nbare\nallow = control app:a b\n",
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
    assert_eq!(
        problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        [5, 6, 7, 8, 9, 10, 11, 12, 13]
    );
    assert!(
        problems[0].message.contains("already decided"),
        "the first answer stands"
    );
    assert!(problems[1]
        .message
        .contains("expected `control` or `capture`"));
    assert!(problems[2].message.contains("app:<id>"));
    assert!(problems[3].message.contains("not an absolute path"));
    assert!(problems[4].message.contains("cannot be written"));
    assert!(
        problems[5].message.contains("expected `allow = "),
        "bare line"
    );
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
    for bad in [
        App::Desktop(String::new()),
        App::Desktop("a b".into()),
        App::Desktop("a#b".into()),
        App::Desktop("x".repeat(MAX_APP + 1)),
        App::Executable("relative".into()),
        App::Executable("/a b".into()),
    ] {
        assert!(!bad.is_rememberable(), "{bad:?}");
        assert!(c.set(Service::Control, &bad, Decision::Allow).is_err());
    }
    assert!(c.entries.is_empty());
    assert!(App::Desktop("x".repeat(MAX_APP)).is_rememberable());
}

#[test]
fn apps_and_services_have_the_words_of_their_lines() {
    assert_eq!(terminal().text(), "app:foot");
    assert_eq!(agent().text(), "exe:/home/me/bin/agent");
    assert_eq!(App::parse("app:foot"), Ok(terminal()));
    assert_eq!(App::parse("exe:/home/me/bin/agent"), Ok(agent()));
    assert!(App::parse("foot").is_err());
    assert_eq!(Service::ALL, [Service::Control, Service::Capture]);
    assert_eq!(
        (Service::Control.word(), Service::Capture.word()),
        ("control", "capture")
    );
}

#[test]
fn the_file_is_written_private_read_back_and_refused_when_others_can_write_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-control.conf");
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
fn a_missing_file_decides_nothing_and_a_store_of_a_bad_app_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-control.conf");
    assert_eq!(
        DesktopControlConf::load(&path).unwrap(),
        (DesktopControlConf::default(), vec![])
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
    assert!(!path.exists());
}

#[test]
fn an_update_reads_changes_and_writes_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-control.conf");
    let seen = DesktopControlConf::update(&path, |c| {
        c.set(Service::Control, &terminal(), Decision::Allow)
            .map_err(std::io::Error::other)?;
        Ok(c.entries.len())
    })
    .unwrap();
    assert_eq!(seen, 1);
    let n = DesktopControlConf::update(&path, |c| {
        assert_eq!(c.entries.len(), 1);
        c.forget(Service::Control, &terminal());
        Ok(c.entries.len())
    })
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}

#[test]
fn an_update_that_fails_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-control.conf");
    let err =
        DesktopControlConf::update::<()>(&path, |_| Err(std::io::Error::other("no"))).unwrap_err();
    assert_eq!(err.to_string(), "no");
    assert!(!path.exists());
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

#[test]
fn the_variable_and_file_name_are_the_documented_ones() {
    assert_eq!(DESKTOP_CONTROL_ENV, "CUPERTINO_DESKTOP_CONTROL_CONF");
    assert_eq!(DESKTOP_CONTROL_FILE, "desktop-control.conf");
    std::env::set_var(DESKTOP_CONTROL_ENV, "/x/dc.conf");
    assert_eq!(DesktopControlConf::path(), Some("/x/dc.conf".into()));
}
