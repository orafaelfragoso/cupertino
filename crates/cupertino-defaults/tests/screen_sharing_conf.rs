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
    assert_eq!(c.grants.len(), 1);
    assert_eq!(
        c.grants[0],
        Grant {
            id: id.clone(),
            app_id: "org.example.Meeting".into(),
            display: "DP-1".into()
        }
    );
    assert_eq!(
        problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        [3, 4, 5, 6, 7, 8, 9]
    );
    assert!(problems[0].message.contains("already given"));
    assert!(problems[1]
        .message
        .contains("32 lowercase hexadecimal digits"));
    assert!(problems[2]
        .message
        .contains("32 lowercase hexadecimal digits"));
    assert!(problems[3].message.contains("display <connector>"));
}

#[test]
fn a_grant_id_is_32_lowercase_hexadecimal_digits() {
    assert!(is_id(&"0123456789abcdef".repeat(2)));
    assert!(!is_id(&"A".repeat(32)));
    assert!(!is_id(&"a".repeat(31)));
    assert!(!is_id(&"g".repeat(32)));
    assert_eq!(GRANT_ID_DIGITS, 32);
}

#[test]
fn a_grant_restores_only_for_its_own_app_and_is_gone_once_revoked() {
    let mut c = ScreenSharingConf {
        grants: vec![grant(1, "a.App", "DP-1"), grant(2, "b.App", "DP-2")],
    };
    let id = format!("{:032x}", 1);
    assert_eq!(c.find(&id, "a.App").unwrap().display, "DP-1");
    assert!(c.find(&id, "b.App").is_none());
    assert!(c.revoke(&id));
    assert!(!c.revoke(&id));
    assert!(c.find(&id, "a.App").is_none());
    assert_eq!(c.grants.len(), 1);
}

#[test]
fn a_grant_checks_what_a_line_can_hold() {
    assert!(grant(1, "a.App", "DP-1").check().is_ok());
    assert!(grant(1, "", "DP-1").check().is_err());
    assert!(grant(1, "a b", "DP-1").check().is_err());
    assert!(grant(1, "a.App", "DP\t1").check().is_err());
    assert!(grant(1, &"x".repeat(MAX_NAME + 1), "DP-1").check().is_err());
    assert!(grant(1, &"x".repeat(MAX_NAME), "DP-1").check().is_ok());
    assert_eq!(
        grant(1, "a.App", "DP-1").text(),
        format!("{:032x} a.App display DP-1", 1)
    );
}

#[test]
fn the_file_is_written_private_read_back_and_refused_when_others_can_write_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("screen-sharing.conf");
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
fn a_missing_file_holds_no_grant_and_a_bad_grant_is_not_stored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("screen-sharing.conf");
    assert_eq!(
        ScreenSharingConf::load(&path).unwrap(),
        (ScreenSharingConf::default(), vec![])
    );
    let bad = ScreenSharingConf {
        grants: vec![grant(1, "a b", "DP-1")],
    };
    assert_eq!(
        bad.store(&path).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert!(!path.exists());
}

#[test]
fn an_update_adds_and_forgets_a_grant_under_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("screen-sharing.conf");
    ScreenSharingConf::update(&path, |c| {
        c.grants.push(grant(1, "a.App", "DP-1"));
        Ok(())
    })
    .unwrap();
    let removed =
        ScreenSharingConf::update(&path, |c| Ok(c.revoke(&format!("{:032x}", 1)))).unwrap();
    assert!(removed);
    assert!(ScreenSharingConf::load(&path).unwrap().0.grants.is_empty());
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

#[test]
fn concurrent_updates_keep_every_grant_added() {
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
}

#[test]
fn the_variable_and_file_name_are_the_documented_ones() {
    assert_eq!(SCREEN_SHARING_ENV, "CUPERTINO_SCREEN_SHARING_CONF");
    assert_eq!(SCREEN_SHARING_FILE, "screen-sharing.conf");
    std::env::set_var(SCREEN_SHARING_ENV, "/x/ss.conf");
    assert_eq!(ScreenSharingConf::path(), Some("/x/ss.conf".into()));
}
