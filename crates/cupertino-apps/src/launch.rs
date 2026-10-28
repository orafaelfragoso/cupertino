//! Starting programs from a shell process (the dock, the menu bar): what to start, the command it becomes with the
//! activation token the process fetched for it, and running that command detached. The token itself is the
//! process's to request, on its own Wayland connection.

use crate::entry::DesktopEntry;
use crate::exec::{command_for_entry, CommandSpec};
use crate::mime::{DIRECTORY, OCTET_STREAM};
use crate::mimeapps::MimeApps;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What opens a target no installed application handles: the session's default handler of its type.
pub const OPEN_COMMAND: &str = "xdg-open";

/// What a launch starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// A program (found on `PATH`) and its arguments.
    Program(Vec<String>),
    /// An application, with no files.
    App(DesktopEntry),
    /// A file opened with an application.
    Open { app: DesktopEntry, file: PathBuf },
}

impl Launch {
    /// The command that starts the launch and hands it `token`; `None` when there is no program to run (an empty
    /// argument list, an unusable `Exec`). An entry with `Terminal=true` keeps [`CommandSpec::terminal`] set.
    pub fn command(&self, token: &str) -> Option<CommandSpec> {
        let spec = match self {
            Launch::Program(argv) => CommandSpec::from_argv(argv.clone())?,
            Launch::App(app) => command_for_entry(app, &[])?,
            Launch::Open { app, file } => {
                command_for_entry(app, &[file.to_string_lossy().into_owned()])?
            }
        };
        Some(spec.with_activation_token(token))
    }
}

/// `file` opened with the default application of its type (a folder as `inode/directory`, a name no glob matches
/// as `application/octet-stream`); `None` when no installed application handles it.
pub fn open_with_default(apps: &MimeApps, file: &Path) -> Option<Launch> {
    let mime = if file.is_dir() {
        DIRECTORY
    } else {
        file.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| apps.database().type_for_name(n))
            .unwrap_or(OCTET_STREAM)
    };
    apps.default_for(mime).map(|app| Launch::Open {
        app: app.clone(),
        file: file.to_path_buf(),
    })
}

/// Run `command` detached from the caller's loop; its exit is reaped on a thread. False when it did not start.
/// Problems are reported on standard error prefixed with `process`, the caller's name.
pub fn spawn(process: &str, mut command: Command) -> bool {
    let program = command.get_program().to_string_lossy().into_owned();
    match command.spawn() {
        Ok(mut child) => {
            let process = process.to_owned();
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("{process}: {program}: {e}");
                }
            });
            true
        }
        Err(e) => {
            eprintln!("{process}: cannot run {program}: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{ACTIVATION_TOKEN_ENV, STARTUP_ID_ENV};
    use crate::{Locale, MimeDatabase, Visibility, XdgDirs};

    fn entry(id: &str, exec: &str) -> DesktopEntry {
        DesktopEntry {
            id: id.into(),
            name: "Editor".into(),
            generic_name: String::new(),
            comment: String::new(),
            keywords: vec![],
            categories: vec![],
            icon: None,
            exec: exec.into(),
            try_exec: None,
            terminal: false,
            path: None,
            startup_wm_class: None,
            actions: vec![],
            source: None,
        }
    }

    fn token_env(token: &str) -> Vec<(String, String)> {
        vec![
            (ACTIVATION_TOKEN_ENV.into(), token.into()),
            (STARTUP_ID_ENV.into(), token.into()),
        ]
    }

    #[test]
    fn an_app_starts_without_files_and_with_the_token() {
        let c = Launch::App(entry("editor.desktop", "editor --new %F %u"))
            .command("t1")
            .unwrap();
        assert_eq!(c.argv(), ["editor", "--new"]);
        assert_eq!(c.env, token_env("t1"));
        assert_eq!(c.desktop_id.as_deref(), Some("editor.desktop"));
        assert_eq!(
            Launch::App(entry("x.desktop", "%U")).command("t1"),
            None,
            "nothing to run"
        );
    }

    #[test]
    fn a_document_expands_into_the_file_codes_with_the_token() {
        let file = PathBuf::from("/home/ada/Notes & Plans.md");
        for (exec, argv) in [
            ("editor %f", vec!["editor", "/home/ada/Notes & Plans.md"]),
            (
                "editor --open %U",
                vec!["editor", "--open", "/home/ada/Notes & Plans.md"],
            ),
        ] {
            let c = Launch::Open {
                app: entry("editor.desktop", exec),
                file: file.clone(),
            }
            .command("t2")
            .unwrap();
            assert_eq!(c.argv(), argv);
            assert_eq!(c.env, token_env("t2"));
        }
    }

    #[test]
    fn a_program_runs_as_given_and_an_empty_one_is_nothing() {
        let c = Launch::Program(vec!["cupertino-settings".into(), "--about".into()])
            .command("t3")
            .unwrap();
        assert_eq!(c.argv(), ["cupertino-settings", "--about"]);
        assert_eq!(c.env, token_env("t3"));
        assert_eq!(Launch::Program(vec![]).command("t3"), None);
    }

    #[test]
    fn a_file_opens_with_the_default_application_of_its_type() {
        let d = tempfile::tempdir().unwrap();
        let write = |rel: &str, text: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write("usr/mime/globs2", "50:text/plain:*.txt\n");
        write(
            "usr/applications/editor.desktop",
            "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor %f\nMimeType=text/plain;\n",
        );
        let dirs = XdgDirs {
            config_home: d.path().join("config"),
            config_dirs: vec![],
            data_home: d.path().join("local"),
            data_dirs: vec![d.path().join("usr")],
            current_desktops: vec![],
        };
        let db = MimeDatabase::load(&dirs.all_data_dirs()).unwrap();
        let apps = MimeApps::load(
            &dirs,
            db,
            &Locale::default(),
            &Visibility {
                skip_try_exec: true,
                ..Visibility::default()
            },
        )
        .unwrap();
        let notes = Path::new("/home/ada/notes.txt");
        match open_with_default(&apps, notes) {
            Some(Launch::Open { app, file }) => {
                assert_eq!(app.id, "editor.desktop");
                assert_eq!(file, notes);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            open_with_default(&apps, Path::new("/home/ada/photo.png")),
            None,
            "no application for the type"
        );
    }

    #[test]
    fn spawn_reports_whether_the_program_started() {
        assert!(spawn("test", Command::new("true")));
        assert!(!spawn(
            "test",
            Command::new("/nonexistent/cupertino-program")
        ));
    }
}
