//! `screen-sharing.conf`: the displays the person let an app share again without asking.

use std::os::unix::fs::PermissionsExt;

use cupertino_defaults::lock::FileLock;
use cupertino_defaults::screen_sharing::*;

fn grant(n: u32, app: &str, display: &str) -> Grant {
    Grant {
        id: format!("{n:032x}"),
        app_id: app.into(),
        display: display.into(),
    }
}

#[test]
fn a_line_holds_one_grant_and_anything_else_is_a_problem() {
    let id = format!("{:032x}", 0xabc);
    let (c, problems) = ScreenSharingConf::parse(&format!(
        "# c\ngrant = {id} org.example.Meeting display DP-1\ngrant = {id} other display DP-2\n\
         grant = XYZ app display DP-1\ngrant = {id}2 app display DP-1\ngrant = {a:032x} app window DP-1\n\
         grant = {b:032x} app display\nshare = 1\nbare\n",
        a = 1,
        b = 2,
    ));
    assert_eq!(
        c.grants,
        [Grant {
            id: id.clone(),
            app_id: "org.example.Meeting".into(),
            display: "DP-1".into()
        }]
    );
    assert_eq!(
        problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        [3, 4, 5, 6, 7, 8, 9]
    );
    for (index, expected) in [
        (0, "already given"),
        (1, "32 lowercase hexadecimal digits"),
        (2, "32 lowercase hexadecimal digits"),
        (3, "display <connector>"),
    ] {
        assert!(
            problems[index].message.contains(expected),
            "problem {index}: {}",
            problems[index].message
        );
    }
}

#[test]
fn a_grant_holds_only_what_a_line_can_hold() {
    for (id, valid) in [
        ("0123456789abcdef".repeat(2), true),
        ("A".repeat(32), false),
        ("a".repeat(31), false),
        ("g".repeat(32), false),
    ] {
        assert_eq!(is_id(&id), valid, "{id}");
    }
    let longest = "x".repeat(MAX_NAME);
    let long = "x".repeat(MAX_NAME + 1);
    for (app, display, valid) in [
        ("a.App", "DP-1", true),
        ("", "DP-1", false),
        ("a b", "DP-1", false),
        ("a.App", "DP\t1", false),
        (long.as_str(), "DP-1", false),
        (longest.as_str(), "DP-1", true),
    ] {
        assert_eq!(
            grant(1, app, display).check().is_ok(),
            valid,
            "{app:?} {display:?}"
        );
    }
    assert_eq!(
        grant(1, "a.App", "DP-1").text(),
        format!("{:032x} a.App display DP-1", 1)
    );
}

#[test]
fn a_grant_restores_only_for_its_own_app_and_is_gone_once_revoked() {
    let mut c = ScreenSharingConf {
        grants: vec![grant(1, "a.App", "DP-1"), grant(2, "b.App", "DP-2")],
    };
    let id = format!("{:032x}", 1);
    assert_eq!(c.find(&id, "a.App").unwrap().display, "DP-1");
    assert!(c.find(&id, "b.App").is_none(), "another app's grant");
    assert!(c.revoke(&id));
    assert!(!c.revoke(&id));
    assert!(c.find(&id, "a.App").is_none());
    assert_eq!(c.grants.len(), 1);
}

#[test]
fn the_file_is_private_and_refused_when_others_can_write_it_or_a_grant_is_bad() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("screen-sharing.conf");
    assert_eq!(
        ScreenSharingConf::load(&path).unwrap(),
        (ScreenSharingConf::default(), vec![]),
        "a missing file holds no grant"
    );
    let bad = ScreenSharingConf {
        grants: vec![grant(1, "a b", "DP-1")],
    };
    assert_eq!(
        bad.store(&path).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert!(!path.exists(), "a bad grant is not stored");
    let c = ScreenSharingConf {
        grants: vec![grant(1, "a.App", "DP-1"), grant(2, "b.App", "HDMI-A-1")],
    };
    c.store(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(ScreenSharingConf::load(&path).unwrap(), (c, vec![]));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(
        ScreenSharingConf::load(&path).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[test]
fn updates_under_the_lock_keep_every_grant_added_and_forget_a_revoked_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("screen-sharing.conf");
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                ScreenSharingConf::update(&path, |c| {
                    c.grants.push(grant(i, "a.App", "DP-1"));
                    Ok(())
                })
                .unwrap();
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(ScreenSharingConf::load(&path).unwrap().0.grants.len(), 8);
    let removed =
        ScreenSharingConf::update(&path, |c| Ok(c.revoke(&format!("{:032x}", 1)))).unwrap();
    assert!(removed);
    assert_eq!(ScreenSharingConf::load(&path).unwrap().0.grants.len(), 7);
}

#[test]
fn an_update_while_another_writer_keeps_the_lock_fails_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("screen-sharing.conf");
    let _held = FileLock::acquire(&path).unwrap();
    let err = ScreenSharingConf::update(&path, |c| {
        c.grants.push(grant(1, "a.App", "DP-1"));
        Ok(())
    })
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(!path.exists());
}
