//! What a shell process starts: the commands of the three kinds of launch, a file's default application and spawning.

mod common;

use common::{desktop, entry, vis, write};
use cupertino_apps::entry::Locale;
use cupertino_apps::exec::{ACTIVATION_TOKEN_ENV, STARTUP_ID_ENV};
use cupertino_apps::launch::{open_with_default, spawn, Launch, OPEN_COMMAND};
use cupertino_apps::mime::MimeDatabase;
use cupertino_apps::mimeapps::{MimeApps, XdgDirs};
use std::path::Path;

fn token_of(spec: &cupertino_apps::CommandSpec, name: &str) -> Vec<String> {
    spec.env
        .iter()
        .filter(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
        .collect()
}

#[test]
fn a_program_launch_is_its_argument_list_with_the_token() {
    let spec = Launch::Program(vec!["tool".into(), "--x".into()])
        .command("tok")
        .unwrap();
    assert_eq!(spec.argv(), ["tool", "--x"]);
    assert_eq!(token_of(&spec, ACTIVATION_TOKEN_ENV), ["tok"]);
    assert_eq!(token_of(&spec, STARTUP_ID_ENV), ["tok"]);
}

#[test]
fn an_empty_program_launch_has_no_command() {
    assert!(Launch::Program(Vec::new()).command("tok").is_none());
}

#[test]
fn an_application_launch_runs_its_exec_without_files() {
    let app = entry(
        "a.desktop",
        &desktop("A", "tool-for-tests-only %F --new", ""),
    );
    let spec = Launch::App(app).command("tok").unwrap();
    assert_eq!(spec.argv(), ["tool-for-tests-only", "--new"]);
    assert_eq!(spec.desktop_id.as_deref(), Some("a.desktop"));
    assert_eq!(token_of(&spec, ACTIVATION_TOKEN_ENV), ["tok"]);
}

#[test]
fn an_open_launch_passes_the_file_through_the_field_codes() {
    let app = entry("a.desktop", &desktop("A", "tool-for-tests-only %f", ""));
    let launch = Launch::Open {
        app,
        file: "/data/a b.txt".into(),
    };
    let spec = launch.command("tok").unwrap();
    assert_eq!(spec.argv(), ["tool-for-tests-only", "/data/a b.txt"]);
}

#[test]
fn an_application_with_an_unusable_exec_has_no_launch_command() {
    let app = entry("a", &desktop("A", "\"broken", ""));
    assert!(Launch::App(app).command("tok").is_none());
}

#[test]
fn the_open_command_is_xdg_open() {
    assert_eq!(OPEN_COMMAND, "xdg-open");
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
    write(
        dir.path(),
        "home/applications/editor.desktop",
        &desktop("Editor", "tool-for-tests-only %f", "MimeType=text/plain;"),
    );
    let apps = apps_in(dir.path());
    let file = dir.path().join("notes.txt");
    match open_with_default(&apps, &file).unwrap() {
        Launch::Open { app, file: opened } => {
            assert_eq!(app.id, "editor.desktop");
            assert_eq!(opened, file);
        }
        other => panic!("{other:?}"),
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
fn a_folder_opens_as_inode_directory() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "home/applications/files.desktop",
        &desktop(
            "Files",
            "tool-for-tests-only %U",
            "MimeType=inode/directory;",
        ),
    );
    let apps = apps_in(dir.path());
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    match open_with_default(&apps, &folder).unwrap() {
        Launch::Open { app, .. } => assert_eq!(app.id, "files.desktop"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unnamed_type_opens_with_an_octet_stream_handler() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "home/applications/any.desktop",
        &desktop(
            "Any",
            "tool-for-tests-only %f",
            "MimeType=application/octet-stream;",
        ),
    );
    let apps = apps_in(dir.path());
    assert!(open_with_default(&apps, &dir.path().join("blob.zzz")).is_some());
}

#[test]
fn spawning_a_program_that_does_not_exist_fails() {
    let command = std::process::Command::new("/nonexistent/cupertino-test-program");
    assert!(!spawn("test", command));
}

#[test]
fn spawning_a_program_that_exists_succeeds_without_waiting_for_it() {
    assert!(spawn("test", std::process::Command::new("true")));
}
