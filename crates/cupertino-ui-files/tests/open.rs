//! Opening an item against a fixture of installed applications: the default application of its type and the Open
//! With list under the user's choices, a terminal application, and an application's own item, which starts it.

#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;

use std::path::{Path, PathBuf};

use cupertino_apps::entry::{Locale, Visibility};
use cupertino_apps::mime::MimeDatabase;
use cupertino_apps::mimeapps::{MimeApps, XdgDirs};
use cupertino_ui_files::open::{
    application_at, command_with, default_app_id, open_command, open_with, OpenError,
};
use scratch::Scratch;

const TERMINAL: [&str; 2] = ["foot", "-e"];

fn desktop(name: &str, exec: &str, extra: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n{extra}\n")
}

/// Applications installed under `data/applications`, the types of `data/mime/globs2`, and the user's
/// `config/mimeapps.list` holding `choices`.
fn session(test: &str, choices: &str) -> (Scratch, MimeApps) {
    let scratch = Scratch::new(test);
    let apps = [
        (
            "editor.desktop",
            desktop("Editor", "editor %f", "MimeType=text/plain;"),
        ),
        (
            "viewer.desktop",
            desktop("Viewer", "viewer %F", "MimeType=text/plain;image/png;"),
        ),
        (
            "notes-tui.desktop",
            desktop(
                "Notes TUI",
                "vim %f",
                "Terminal=true\nMimeType=text/markdown;",
            ),
        ),
        (
            "files.desktop",
            desktop("Files", "filer %U", "MimeType=inode/directory;"),
        ),
        ("org/tool.desktop", desktop("Tool", "tool --run", "")),
    ];
    for (id, text) in apps {
        scratch.file(&format!("data/applications/{id}"), text.as_bytes());
    }
    scratch.file(
        "data/mime/globs2",
        b"50:text/plain:*.txt\n50:image/png:*.png\n50:text/markdown:*.md\n",
    );
    scratch.file("config/mimeapps.list", choices.as_bytes());
    let root = scratch.path();
    let dirs = XdgDirs {
        config_home: root.join("config"),
        config_dirs: Vec::new(),
        data_home: root.join("home"),
        data_dirs: vec![root.join("data")],
        current_desktops: vec!["Cupertino".into()],
    };
    let visibility = Visibility {
        current_desktops: vec!["Cupertino".into()],
        path_dirs: Vec::new(),
        skip_try_exec: true,
    };
    let database = MimeDatabase::load(&[root.join("data")]).unwrap();
    let apps = MimeApps::load(&dirs, database, &Locale::default(), &visibility).unwrap();
    (scratch, apps)
}

fn argv(
    result: Result<cupertino_apps::exec::CommandSpec, OpenError>,
) -> Result<Vec<String>, OpenError> {
    result.map(|spec| spec.argv().into_iter().map(String::from).collect())
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_file_opens_with_the_default_application_of_its_type_the_user_chose_or_the_first_installed() {
    let rows = [
        (
            "no choice made",
            "",
            "notes.txt",
            false,
            Ok(vec!["editor", "/w/notes.txt"]),
        ),
        (
            "a choice of the user",
            "[Default Applications]\ntext/plain=viewer.desktop\n",
            "notes.txt",
            false,
            Ok(vec!["viewer", "/w/notes.txt"]),
        ),
        (
            "a type with one handler",
            "",
            "photo.png",
            false,
            Ok(vec!["viewer", "/w/photo.png"]),
        ),
        ("a folder", "", "stuff", true, Ok(vec!["filer", "/w/stuff"])),
        (
            "a type no application handles",
            "",
            "data.zzzz",
            false,
            Err(OpenError::NoApplication {
                mime: "application/octet-stream".into(),
            }),
        ),
    ];
    for (what, choices, name, is_dir, expected) in rows {
        let (_scratch, apps) = session("open-default", choices);
        let path = Path::new("/w").join(name);
        let opened = argv(open_command(&apps, &path, is_dir, None));
        let expected = expected.map(|a: Vec<&str>| strings(&a));
        assert_eq!(opened, expected, "{what}");
    }
}

#[test]
fn an_application_that_runs_in_a_terminal_opens_only_when_a_terminal_is_given() {
    let (_scratch, apps) = session("open-terminal", "");
    let path = PathBuf::from("/w/todo.md");
    let terminal = strings(&TERMINAL);
    assert_eq!(
        argv(open_command(&apps, &path, false, None)),
        Err(OpenError::NeedsTerminal {
            id: "notes-tui.desktop".into()
        })
    );
    assert_eq!(
        argv(open_command(&apps, &path, false, Some(&terminal))),
        Ok(strings(&["foot", "-e", "vim", "/w/todo.md"]))
    );
    let entry = apps.entry("notes-tui.desktop").unwrap();
    assert_eq!(
        argv(command_with(entry, &path, None)),
        Err(OpenError::NeedsTerminal {
            id: "notes-tui.desktop".into()
        })
    );
    let editor = apps.entry("editor.desktop").unwrap();
    assert_eq!(
        argv(command_with(editor, &path, Some(&terminal))),
        Ok(strings(&["editor", "/w/todo.md"]))
    );
}

#[test]
fn the_open_with_list_has_the_default_first_and_the_rest_by_name() {
    let rows = [
        (
            "no choice made",
            "",
            "editor.desktop",
            vec![("Editor", true), ("Viewer", false)],
        ),
        (
            "a choice of the user",
            "[Default Applications]\ntext/plain=viewer.desktop\n",
            "viewer.desktop",
            vec![("Viewer", true), ("Editor", false)],
        ),
    ];
    for (what, choices, default_id, expected) in rows {
        let (_scratch, apps) = session("open-with", choices);
        let path = Path::new("/w/notes.txt");
        let listed: Vec<(String, bool)> = open_with(&apps, path, false)
            .into_iter()
            .map(|item| (item.entry.name.clone(), item.is_default))
            .collect();
        let expected: Vec<(String, bool)> = expected
            .into_iter()
            .map(|(name, default)| (name.to_string(), default))
            .collect();
        assert_eq!(listed, expected, "{what}");
        let default = default_app_id(&apps, path, false);
        assert_eq!(default.as_deref(), Some(default_id), "{what}");
    }
    let (_scratch, apps) = session("open-with-none", "");
    assert!(open_with(&apps, Path::new("/w/data.zzzz"), false).is_empty());
    assert_eq!(
        default_app_id(&apps, Path::new("/w/data.zzzz"), false),
        None
    );
}

#[test]
fn the_item_of_an_installed_application_starts_it_and_a_copy_of_its_file_elsewhere_does_not() {
    let (scratch, apps) = session("open-application", "");
    let editor = scratch.path().join("data/applications/editor.desktop");
    let nested = scratch.path().join("data/applications/org/tool.desktop");
    let copy = scratch.file(
        "elsewhere/applications/editor.desktop",
        b"[Desktop Entry]\n",
    );
    let plain = scratch.path().join("data/applications/readme.txt");

    assert_eq!(
        application_at(&apps, &editor).map(|e| e.id.as_str()),
        Some("editor.desktop")
    );
    assert_eq!(
        application_at(&apps, &nested).map(|e| e.id.as_str()),
        Some("org-tool.desktop")
    );
    assert!(application_at(&apps, &copy).is_none());
    assert!(application_at(&apps, &plain).is_none());

    assert_eq!(
        argv(open_command(&apps, &editor, false, None)),
        Ok(strings(&["editor"]))
    );
    assert_eq!(
        argv(open_command(&apps, &nested, false, None)),
        Ok(strings(&["tool", "--run"]))
    );
    assert_eq!(
        default_app_id(&apps, &nested, false).as_deref(),
        Some("org-tool.desktop")
    );
}
