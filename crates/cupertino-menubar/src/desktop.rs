//! Application name for the bold menu of the bar, from the desktop entry named by the server.

use std::io::ErrorKind;
use std::path::PathBuf;

/// What the bar shows for an application id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppName {
    pub name: String,
    /// A desktop entry exists for the id: the app menu offers About.
    pub has_entry: bool,
}

/// The `Name` of `<app_id>.desktop` in the `applications` directory of the first data directory
/// that has it, else the last segment of the id with its first letter capitalised.
pub fn app_name(app_id: &str, data_dirs: &[PathBuf]) -> AppName {
    // an id with a separator names no entry directly under `applications`
    let entry = (!app_id.contains('/'))
        .then(|| entry_name(app_id, data_dirs))
        .flatten();
    match entry {
        Some(name) => AppName {
            name,
            has_entry: true,
        },
        None => AppName {
            name: capitalised_tail(app_id),
            has_entry: false,
        },
    }
}

/// `Some(name)` for a found entry; an entry without a `Name` shows its capitalised id.
fn entry_name(app_id: &str, data_dirs: &[PathBuf]) -> Option<String> {
    for dir in data_dirs {
        let path = dir.join("applications").join(format!("{app_id}.desktop"));
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                return Some(desktop_entry_name(&text).unwrap_or_else(|| capitalised_tail(app_id)))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => eprintln!("cupertino-menubar: cannot read {}: {e}", path.display()),
        }
    }
    None
}

/// `Name=` of the `[Desktop Entry]` group.
fn desktop_entry_name(text: &str) -> Option<String> {
    let mut in_group = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
        } else if let Some(name) = line.strip_prefix("Name=").filter(|_| in_group) {
            return Some(name.trim().to_string()).filter(|n| !n.is_empty());
        }
    }
    None
}

fn capitalised_tail(app_id: &str) -> String {
    let last = app_id
        .rsplit(['.', '/'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(app_id);
    let mut chars = last.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("applications")).unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join("applications").join(name), text).unwrap();
        }
        dir
    }

    #[test]
    fn the_entry_names_the_app_and_enables_about() {
        let dir = data_dir(&[(
            "org.example.editor.desktop",
            "[Desktop Entry]\nType=Application\nName=Text Editor\nName[de]=Textbearbeitung\n",
        )]);
        let name = app_name("org.example.editor", &[dir.path().to_path_buf()]);
        assert_eq!(
            name,
            AppName {
                name: "Text Editor".into(),
                has_entry: true
            }
        );
    }

    #[test]
    fn the_first_data_directory_with_the_entry_wins() {
        let first = data_dir(&[]);
        let second = data_dir(&[("viewer.desktop", "[Desktop Entry]\nName=Viewer\n")]);
        let third = data_dir(&[("viewer.desktop", "[Desktop Entry]\nName=Other\n")]);
        let dirs = [
            first.path().to_path_buf(),
            second.path().to_path_buf(),
            third.path().to_path_buf(),
        ];
        assert_eq!(app_name("viewer", &dirs).name, "Viewer");
    }

    #[test]
    fn without_an_entry_the_id_is_capitalised() {
        let dir = data_dir(&[]);
        let name = app_name("org.example.editor", &[dir.path().to_path_buf()]);
        assert_eq!(
            name,
            AppName {
                name: "Editor".into(),
                has_entry: false
            }
        );
    }

    #[test]
    fn an_id_with_a_separator_names_no_entry() {
        let dir = data_dir(&[("secret.desktop", "[Desktop Entry]\nName=Secret\n")]);
        let name = app_name("../applications/secret", &[dir.path().to_path_buf()]);
        assert_eq!(
            name,
            AppName {
                name: "Secret".into(),
                has_entry: false
            }
        );
    }

    #[test]
    fn a_name_outside_the_entry_group_is_ignored() {
        let dir = data_dir(&[(
            "tool.desktop",
            "[Desktop Action new]\nName=New Window\n[Desktop Entry]\nExec=tool\n",
        )]);
        assert_eq!(
            app_name("tool", &[dir.path().to_path_buf()]),
            AppName {
                name: "Tool".into(),
                has_entry: true
            }
        );
    }
}
