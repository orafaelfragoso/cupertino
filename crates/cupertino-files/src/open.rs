//! Opening files with applications: the type of a file from its name (shared MIME database globs), its
//! default application and Open With list from `mimeapps.list` (`cupertino-apps`), and the command that
//! opens it.
//!
//! Types come from names only, without content sniffing: a regular file whose name matches no glob is
//! `application/octet-stream`, as the shared-mime-info specification has it when nothing matches.

use cupertino_apps::exec::{command_for_entry, CommandSpec};
use cupertino_apps::mime::{MimeDatabase, DIRECTORY, OCTET_STREAM};
use cupertino_apps::{DesktopEntry, MimeApps, OpenWithItem};
use std::fmt;
use std::path::Path;

/// The type a file is opened as: `inode/directory` for a folder (`is_dir` follows symbolic links), else
/// the type of its name, else `application/octet-stream`.
pub fn file_type<'a>(db: &'a MimeDatabase, path: &Path, is_dir: bool) -> &'a str {
    if is_dir {
        return DIRECTORY;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| db.type_for_name(n))
        .unwrap_or(OCTET_STREAM)
}

/// Why a file cannot be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// No application handles the type.
    NoApplication { mime: String },
    /// The application's `Exec` line gives no command.
    BadExec { id: String },
    /// The application asks for a terminal (`Terminal=true`), and the file manager has none to run it in.
    NeedsTerminal { id: String },
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NoApplication { mime } => write!(f, "no application opens {mime}"),
            OpenError::BadExec { id } => write!(f, "{id} has no usable Exec line"),
            OpenError::NeedsTerminal { id } => {
                write!(f, "{id} runs in a terminal, and none is configured")
            }
        }
    }
}

impl std::error::Error for OpenError {}

/// The command that opens `path` with `entry`; an entry with `Terminal=true` runs under `terminal`, the
/// argv prefix of a terminal emulator (for example `["foot", "-e"]`).
pub fn command_with(
    entry: &DesktopEntry,
    path: &Path,
    terminal: Option<&[String]>,
) -> Result<CommandSpec, OpenError> {
    let spec =
        command_for_entry(entry, &[path.to_string_lossy().into_owned()]).ok_or_else(|| {
            OpenError::BadExec {
                id: entry.id.clone(),
            }
        })?;
    match (spec.terminal, terminal) {
        (false, _) => Ok(spec),
        (true, Some(terminal)) => Ok(spec.in_terminal(terminal)),
        (true, None) => Err(OpenError::NeedsTerminal {
            id: entry.id.clone(),
        }),
    }
}

/// The command that opens `path` with the default application of its type; `terminal` as for
/// [`command_with`].
pub fn open_command(
    apps: &MimeApps,
    path: &Path,
    is_dir: bool,
    terminal: Option<&[String]>,
) -> Result<CommandSpec, OpenError> {
    let mime = file_type(apps.database(), path, is_dir);
    let entry = apps
        .default_for(mime)
        .ok_or_else(|| OpenError::NoApplication {
            mime: mime.to_string(),
        })?;
    command_with(entry, path, terminal)
}

/// The desktop entry id of the default application of `path`, if its type has one.
pub fn default_app_id(apps: &MimeApps, path: &Path, is_dir: bool) -> Option<String> {
    apps.default_for(file_type(apps.database(), path, is_dir))
        .map(|entry| entry.id.clone())
}

/// The Open With list of `path`: the default application first, then the others by name.
pub fn open_with<'a>(apps: &'a MimeApps, path: &Path, is_dir: bool) -> Vec<OpenWithItem<'a>> {
    apps.open_with(file_type(apps.database(), path, is_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_apps::{Locale, Visibility, XdgDirs};

    fn apps(root: &Path) -> MimeApps {
        let write = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write(
            "usr/mime/globs2",
            "50:text/markdown:*.md\n50:text/plain:*.txt\n50:image/png:*.png\n",
        );
        write("usr/mime/subclasses", "text/markdown text/plain\n");
        write("usr/applications/editor.desktop", "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor --open %F\nMimeType=text/plain;\n");
        write(
            "usr/applications/alpha.desktop",
            "[Desktop Entry]\nType=Application\nName=Alpha\nExec=alpha %f\nMimeType=text/plain;\n",
        );
        write("usr/applications/notes.desktop", "[Desktop Entry]\nType=Application\nName=Notes\nExec=notes %u\nMimeType=text/markdown;\n");
        write("usr/applications/files.desktop", "[Desktop Entry]\nType=Application\nName=Files\nExec=files %f\nMimeType=inode/directory;\n");
        write("usr/applications/vi.desktop", "[Desktop Entry]\nType=Application\nName=Vi\nExec=vi %f\nTerminal=true\nMimeType=image/png;\n");
        write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/plain=editor.desktop\n",
        );
        let dirs = XdgDirs {
            config_home: root.join("config"),
            config_dirs: vec![],
            data_home: root.join("local"),
            data_dirs: vec![root.join("usr")],
            current_desktops: vec![],
        };
        let db = MimeDatabase::load(&dirs.all_data_dirs()).unwrap();
        MimeApps::load(
            &dirs,
            db,
            &Locale::default(),
            &Visibility {
                current_desktops: vec![],
                path_dirs: vec![],
                skip_try_exec: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn files_open_with_the_default_of_their_type() {
        let dir = tempfile::tempdir().unwrap();
        let apps = apps(dir.path());
        let c = open_command(&apps, Path::new("/home/u/read me.txt"), false, None).unwrap();
        assert_eq!(c.argv(), ["editor", "--open", "/home/u/read me.txt"]);
        assert_eq!(c.desktop_id.as_deref(), Some("editor.desktop"));
        let c = open_command(&apps, Path::new("/home/u/notes.md"), false, None).unwrap();
        assert_eq!(
            c.argv(),
            ["notes", "/home/u/notes.md"],
            "a handler of the type beats the default of its parent"
        );
        assert_eq!(
            open_command(&apps, Path::new("/home/u/Music"), true, None)
                .unwrap()
                .argv(),
            ["files", "/home/u/Music"]
        );
    }

    #[test]
    fn the_default_application_id_is_the_entry_the_launch_token_names() {
        let dir = tempfile::tempdir().unwrap();
        let apps = apps(dir.path());
        assert_eq!(
            default_app_id(&apps, Path::new("read me.txt"), false).as_deref(),
            Some("editor.desktop")
        );
        assert_eq!(default_app_id(&apps, Path::new("blob"), false), None);
    }

    #[test]
    fn unknown_types_are_errors_and_terminal_handlers_need_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let apps = apps(dir.path());
        assert_eq!(
            open_command(&apps, Path::new("/x/blob"), false, None),
            Err(OpenError::NoApplication {
                mime: OCTET_STREAM.into()
            })
        );
        assert_eq!(
            open_command(&apps, Path::new("/x/a.png"), false, None),
            Err(OpenError::NeedsTerminal {
                id: "vi.desktop".into()
            })
        );
        let foot = ["foot".to_string(), "-e".to_string()];
        assert_eq!(
            open_command(&apps, Path::new("/x/a.png"), false, Some(&foot))
                .unwrap()
                .argv(),
            ["foot", "-e", "vi", "/x/a.png"]
        );
    }

    #[test]
    fn the_open_with_list_starts_with_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let apps = apps(dir.path());
        let list: Vec<(&str, bool)> = open_with(&apps, Path::new("a.md"), false)
            .iter()
            .map(|i| (i.entry.name.as_str(), i.is_default))
            .collect();
        assert_eq!(list, [("Notes", true), ("Alpha", false), ("Editor", false)]);
    }
}
