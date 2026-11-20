//! Application name for the bold menu of the bar, from the desktop entry named by the server.

use std::io::ErrorKind;
use std::path::PathBuf;

/// What the bar shows for an application id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppName {
    pub name: String,
    /// A desktop entry exists for the id: the About panel can show what it names.
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

/// What the bar shows for `app_id` without its desktop entry: the last segment of the id with its first letter
/// capitalised.
pub fn provisional_name(app_id: &str) -> AppName {
    AppName {
        name: capitalised_tail(app_id),
        has_entry: false,
    }
}

/// `Some(name)` for a found entry; an entry without a `Name` shows its capitalised id. For the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
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
