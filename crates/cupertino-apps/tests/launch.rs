//! What a shell process starts: the commands of the three kinds of launch, a file's default application and spawning.

mod common;

use common::{desktop, entry, vis, write};
use cupertino_apps::entry::Locale;
use cupertino_apps::exec::{ACTIVATION_TOKEN_ENV, STARTUP_ID_ENV};
use cupertino_apps::launch::{open_with_default, spawn, Launch};
use cupertino_apps::mime::MimeDatabase;
use cupertino_apps::mimeapps::{MimeApps, XdgDirs};
use std::path::Path;

#[test]
fn each_kind_of_launch_becomes_its_command_with_the_token() {
    let app = |exec: &str| entry("a.desktop", &desktop("A", exec, ""));
    for (launch, expected) in [
        (
            Launch::Program(vec!["tool".into(), "--x".into()]),
            Some(&["tool", "--x"][..]),
        ),
        (Launch::Program(Vec::new()), None),
        (
            Launch::App(app("tool-for-tests-only %F --new")),
            Some(&["tool-for-tests-only", "--new"]),
        ),
        (
            Launch::Open {
                app: app("tool-for-tests-only %f"),
                file: "/data/a b.txt".into(),
            },
            Some(&["tool-for-tests-only", "/data/a b.txt"]),
        ),
        (Launch::App(app("\"broken")), None),
    ] {
        let label = format!("{launch:?}");
        let spec = launch.command("tok");
        assert_eq!(
            spec.as_ref().map(|s| s.argv()),
            expected.map(<[&str]>::to_vec),
            "{label}"
        );
        if let Some(spec) = spec {
            for name in [ACTIVATION_TOKEN_ENV, STARTUP_ID_ENV] {
                assert!(
                    spec.env.iter().any(|(k, v)| k == name && v == "tok"),
                    "{label}: {name}"
                );
            }
        }
    }
    let spec = Launch::App(app("tool-for-tests-only"))
        .command("tok")
        .unwrap();
    assert_eq!(spec.desktop_id.as_deref(), Some("a.desktop"));
}

fn apps_in(root: &Path) -> MimeApps {
    let dirs = XdgDirs {
        config_home: root.join("config"),
        config_dirs: Vec::new(),
        data_home: root.join("home"),
        data_dirs: Vec::new(),
        current_desktops: Vec::new(),
    };
    let db = MimeDatabase::load(&[root.join("home")]).unwrap();
    MimeApps::load(&dirs, db, &Locale::default(), &vis()).unwrap()
}

#[test]
fn a_file_opens_with_the_default_application_of_its_type() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "home/mime/globs2", "50:text/plain:*.txt\n");
    for (id, mime) in [
        ("editor", "text/plain"),
        ("files", "inode/directory"),
        ("any", "application/octet-stream"),
    ] {
        write(
            dir.path(),
            &format!("home/applications/{id}.desktop"),
            &desktop(id, "tool-for-tests-only %f", &format!("MimeType={mime};")),
        );
    }
    let apps = apps_in(dir.path());
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    for (file, expected) in [
        (dir.path().join("notes.txt"), "editor.desktop"),
        (folder, "files.desktop"),
        (dir.path().join("blob.zzz"), "any.desktop"),
    ] {
        match open_with_default(&apps, &file) {
            Some(Launch::Open { app, file: opened }) => {
                assert_eq!(app.id, expected, "{}", file.display());
                assert_eq!(opened, file);
            }
            other => panic!("{}: {other:?}", file.display()),
        }
    }
}

#[test]
fn a_file_of_a_type_without_a_handler_has_no_default_launch() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "home/mime/globs2", "50:text/plain:*.txt\n");
    let apps = apps_in(dir.path());
    assert!(open_with_default(&apps, &dir.path().join("a.png")).is_none());
}

#[test]
fn spawning_reports_whether_the_program_started() {
    assert!(!spawn(
        "test",
        std::process::Command::new("/nonexistent/cupertino-test-program")
    ));
    assert!(spawn("test", std::process::Command::new("true")));
}
