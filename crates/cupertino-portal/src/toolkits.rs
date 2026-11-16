//! The session's settings for toolkits that read them elsewhere than from the portal:
//!
//! - GSettings, in the `org.gnome.desktop.interface` and `org.gnome.desktop.wm.preferences` schemas: GTK 3 reads them
//!   on Wayland, GTK 4 when no portal answers, and the GTK backend of the portal serves them too, so both backends give
//!   the same values. Set with `gsettings`.
//! - `gtk-3.0/settings.ini` and `gtk-4.0/settings.ini` in the configuration directory, the `[Settings]` group: GTK reads
//!   them when it starts under X11, where the X display publishes none of these settings. Only the keys below are
//!   written; every other line stays as it was, and a file keeps its permissions.
//!
//! These places belong to the person and are shared with other desktops of the same account, so a value is written
//! only when the desktop's own changes, or when the place still holds what was last written there ([`Written`], kept
//! in Cupertino's state folder): a value the person set there since is left as it is until the desktop's own changes.
//!
//! A GTK application on Wayland follows a change at once; one under X11 takes it when it starts.

use std::collections::BTreeMap;
use std::io::{self, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;

use crate::settings::{Appearance, BUTTON_LAYOUT};

/// What the toolkits are told; the title bar buttons always go where [`BUTTON_LAYOUT`] says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolkitSettings {
    /// Scroll bars are thin bars over the content, shown while it scrolls.
    pub overlay_scroll_bars: bool,
}

impl From<Appearance> for ToolkitSettings {
    fn from(appearance: Appearance) -> Self {
        Self {
            overlay_scroll_bars: appearance.overlay_scroll_bars(),
        }
    }
}

/// The GSettings schema of the desktop's interface settings.
pub const INTERFACE_SCHEMA: &str = "org.gnome.desktop.interface";
/// The GSettings schema of the desktop's window settings.
pub const WINDOW_SCHEMA: &str = "org.gnome.desktop.wm.preferences";
/// The GTK settings files under the configuration directory.
pub const SETTINGS_FILES: [&str; 2] = ["gtk-3.0/settings.ini", "gtk-4.0/settings.ini"];
/// The group of a GTK settings file that holds its settings.
const GROUP: &str = "[Settings]";
/// The file in Cupertino's state folder that keeps what was last written to the toolkits' places.
pub const WRITTEN_FILE: &str = "cupertino/toolkit-settings";
/// The permissions of a file made where there was none.
const NEW_FILE_MODE: u32 = 0o644;

impl ToolkitSettings {
    /// The GSettings keys these values go to, each as `(SCHEMA, KEY, VALUE)`, the value as `gsettings` prints it.
    pub fn gsettings(self) -> Vec<(&'static str, &'static str, String)> {
        vec![
            (
                INTERFACE_SCHEMA,
                "overlay-scrolling",
                self.overlay_scroll_bars.to_string(),
            ),
            (WINDOW_SCHEMA, "button-layout", format!("'{BUTTON_LAYOUT}'")),
        ]
    }

    /// The keys of a GTK settings file and their values.
    pub fn settings_ini(self) -> Vec<(&'static str, String)> {
        vec![
            (
                "gtk-overlay-scrolling",
                self.overlay_scroll_bars.to_string(),
            ),
            ("gtk-decoration-layout", BUTTON_LAYOUT.to_owned()),
        ]
    }
}

/// What was last written to each of the toolkits' places (a GSettings key, a key of a settings file), kept in a file
/// of lines `PLACE<TAB>VALUE` so that it outlasts the portal.
#[derive(Debug, Default)]
pub struct Written {
    file: Option<PathBuf>,
    values: BTreeMap<String, String>,
}

impl Written {
    /// What `file` keeps; nothing when it is missing or unreadable. Without a file nothing outlasts the process.
    pub fn load(file: Option<PathBuf>) -> Self {
        let text = file
            .as_deref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default();
        let values = text
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(place, value)| (place.to_owned(), value.to_owned()))
            .collect();
        Self { file, values }
    }

    /// What was last written to `place`.
    pub fn get(&self, place: &str) -> Option<&str> {
        self.values.get(place).map(String::as_str)
    }

    /// Whether `ours` goes to `place`, which holds `current`: when nothing was written there yet, when ours is not
    /// what was last written, or when the place still holds that. Any other value is the person's own.
    pub fn takes(&self, place: &str, ours: &str, current: Option<&str>) -> bool {
        match self.get(place) {
            None => true,
            Some(last) => ours != last || current == Some(last),
        }
    }

    fn set(&mut self, place: String, value: &str) -> bool {
        self.values.insert(place, value.to_owned()).as_deref() != Some(value)
    }

    /// Keeps the values in the file.
    pub fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let text: String = self
            .values
            .iter()
            .map(|(place, value)| format!("{place}\t{value}\n"))
            .collect();
        replace_file(file, &text)
    }
}

/// The file [`Written`] keeps its values in: `$XDG_STATE_HOME/`[`WRITTEN_FILE`], else under `~/.local/state`.
pub fn written_file() -> Option<PathBuf> {
    let set = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let state = set("XDG_STATE_HOME").or_else(|| set("HOME").map(|h| h.join(".local/state")))?;
    Some(state.join(WRITTEN_FILE))
}

/// Whether a line of a GTK settings file starts a group.
fn is_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

/// The value of `key` in a line `KEY=VALUE`.
fn entry_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_once('=')
        .filter(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
}

/// The lines of `lines` inside a `[Settings]` group (the group may come more than once).
fn in_group(lines: &[String]) -> Vec<bool> {
    let mut inside = false;
    lines
        .iter()
        .map(|l| {
            if is_header(l) {
                inside = l.trim() == GROUP;
                false
            } else {
                inside
            }
        })
        .collect()
}

/// The value GTK takes for `key` from `text`, a GTK settings file: the last one in its `[Settings]` group.
pub fn settings_ini_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let inside = in_group(&lines);
    text.lines()
        .zip(inside)
        .filter(|(_, inside)| *inside)
        .filter_map(|(l, _)| entry_value(l, key))
        .last()
}

/// `text`, a GTK settings file, with `keys` set in its `[Settings]` group: every line of a key already there gets the
/// new value in its place, a missing one is added at the end of the group, and the group is added when there is none.
pub fn edit_settings_ini(text: &str, keys: &[(&'static str, String)]) -> String {
    if keys.is_empty() {
        return text.to_owned();
    }
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let start = match lines.iter().position(|l| l.trim() == GROUP) {
        Some(i) => i,
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(GROUP.to_owned());
            lines.len() - 1
        }
    };
    let mut end = lines[start + 1..]
        .iter()
        .position(|l| is_header(l))
        .map_or(lines.len(), |i| start + 1 + i);
    for (key, value) in keys {
        let entry = format!("{key}={value}");
        let inside = in_group(&lines);
        let found: Vec<usize> = (0..lines.len())
            .filter(|&i| inside[i] && entry_value(&lines[i], key).is_some())
            .collect();
        if found.is_empty() {
            // after the group's last entry, before the blank lines that part it from the next group
            let mut at = end;
            while at > start + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            lines.insert(at, entry);
            end += 1;
        } else {
            for i in found {
                lines[i].clone_from(&entry);
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Puts `text` in `path` as a whole: written to a file beside it, flushed to the disk and renamed over it, with the
/// permissions the old file had.
fn replace_file(path: &Path, text: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mode = std::fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o7777);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = dir.join(format!(".{name}.cupertino"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode.unwrap_or(NEW_FILE_MODE))
        .open(&temporary)?;
    if let Some(mode) = mode {
        // the mode given at creation is narrowed by the process's mask
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    std::fs::File::open(dir)?.sync_all()
}

/// Writes `settings` into the GTK settings files under `config_home`, creating them when missing, where `written`
/// lets them go ([`Written::takes`]). A file left as it was is not written; a link is followed, so the file it names
/// is changed and the link stays.
pub fn write_settings_ini(
    config_home: &Path,
    settings: ToolkitSettings,
    written: &mut Written,
) -> io::Result<()> {
    for name in SETTINGS_FILES {
        let path = config_home.join(name);
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let old = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let keys: Vec<(&'static str, String)> = settings
            .settings_ini()
            .into_iter()
            .filter(|(key, value)| {
                written.takes(
                    &format!("{name} {key}"),
                    value,
                    settings_ini_value(&old, key),
                )
            })
            .collect();
        let new = edit_settings_ini(&old, &keys);
        if new != old {
            replace_file(&path, &new)?;
        }
        for (key, value) in &keys {
            written.set(format!("{name} {key}"), value);
        }
    }
    Ok(())
}

/// Sets each GSettings key of `settings` with `gsettings` (a fresh command for each run) where `written` lets it go
/// ([`Written::takes`]); a key that already holds the value is not set again.
pub fn write_gsettings(
    settings: ToolkitSettings,
    gsettings: &dyn Fn() -> Command,
    written: &mut Written,
) -> io::Result<()> {
    let run = |args: &[&str]| -> io::Result<String> {
        let output = gsettings().args(args).output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "gsettings {} exited with {}",
                args.join(" "),
                output.status
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    for (schema, key, value) in settings.gsettings() {
        let current = run(&["get", schema, key])?;
        let place = format!("gsettings {schema} {key}");
        if !written.takes(&place, &value, Some(&current)) {
            continue;
        }
        if current != value {
            run(&["set", schema, key, &value])?;
        }
        written.set(place, &value);
    }
    Ok(())
}

/// Hands each new [`ToolkitSettings`] to the toolkits on a thread of its own, in order, so the portal's loop never
/// waits on a file or on `gsettings`. A failure is reported on standard error and the next change tries again.
pub struct Writer {
    sender: mpsc::Sender<ToolkitSettings>,
}

impl Writer {
    /// Starts the thread. `config_home` holds the GTK settings files (none are written without it); `written` keeps
    /// what was last written ([`Written`]); `gsettings` makes the command that runs `gsettings`.
    pub fn start(
        config_home: Option<PathBuf>,
        written: Option<PathBuf>,
        gsettings: impl Fn() -> Command + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, received) = mpsc::channel::<ToolkitSettings>();
        std::thread::Builder::new()
            .name("toolkits".to_owned())
            .spawn(move || {
                let mut written = Written::load(written);
                while let Ok(mut settings) = received.recv() {
                    // only the latest of the changes queued meanwhile matters
                    while let Ok(newer) = received.try_recv() {
                        settings = newer;
                    }
                    let before = written.values.clone();
                    if let Err(e) = write_gsettings(settings, &gsettings, &mut written) {
                        eprintln!("cupertino-portal: GTK applications on Wayland keep their own scroll bars and title bar buttons: {e}");
                    }
                    if let Some(home) = &config_home {
                        if let Err(e) = write_settings_ini(home, settings, &mut written) {
                            eprintln!(
                                "cupertino-portal: GTK applications under X11 keep their own scroll bars and title bar buttons: {e}"
                            );
                        }
                    }
                    if written.values != before {
                        if let Err(e) = written.save() {
                            eprintln!("cupertino-portal: settings the person changes in GTK's places may be replaced at the next start: {e}");
                        }
                    }
                }
            })?;
        Ok(Self { sender })
    }

    /// Hands `settings` to the toolkits.
    pub fn apply(&self, settings: ToolkitSettings) {
        let _ = self.sender.send(settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(overlay: bool) -> Vec<(&'static str, String)> {
        ToolkitSettings {
            overlay_scroll_bars: overlay,
        }
        .settings_ini()
    }

    #[test]
    fn an_empty_file_gets_the_group_and_the_keys() {
        assert_eq!(
            edit_settings_ini("", &keys(false)),
            "[Settings]\ngtk-overlay-scrolling=false\ngtk-decoration-layout=close,minimize,maximize:\n"
        );
    }

    #[test]
    fn a_key_already_set_changes_in_place_and_every_other_line_stays() {
        let text = "# mine\n[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling = true\ngtk-decoration-layout=menu:close\ngtk-font-name=Inter 10\n";
        assert_eq!(
            edit_settings_ini(text, &keys(false)),
            "# mine\n[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling=false\ngtk-decoration-layout=close,minimize,maximize:\ngtk-font-name=Inter 10\n"
        );
    }

    #[test]
    fn a_key_set_twice_changes_in_both_places_as_gtk_takes_the_last() {
        let text = "[Settings]\ngtk-overlay-scrolling=true\n[Other]\ngtk-overlay-scrolling=keep\n[Settings]\ngtk-overlay-scrolling=true\n";
        assert_eq!(
            settings_ini_value(text, "gtk-overlay-scrolling"),
            Some("true")
        );
        let edited = edit_settings_ini(text, &keys(false)[..1]);
        assert_eq!(
            edited,
            "[Settings]\ngtk-overlay-scrolling=false\n[Other]\ngtk-overlay-scrolling=keep\n[Settings]\ngtk-overlay-scrolling=false\n"
        );
        assert_eq!(
            settings_ini_value("[Settings]\na=1\na = 2\n[Other]\na=3\n", "a"),
            Some("2")
        );
    }

    #[test]
    fn a_missing_key_ends_its_group_before_the_next_one() {
        let text = "[Settings]\ngtk-theme-name=Mine\n\n[Other]\ngtk-overlay-scrolling=true\n";
        assert_eq!(
            edit_settings_ini(text, &keys(true)),
            "[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling=true\ngtk-decoration-layout=close,minimize,maximize:\n\n[Other]\ngtk-overlay-scrolling=true\n"
        );
    }

    #[test]
    fn a_file_without_the_group_keeps_its_lines_and_gets_the_group_after_them() {
        assert_eq!(
            edit_settings_ini("[Other]\na=b", &keys(true)),
            "[Other]\na=b\n\n[Settings]\ngtk-overlay-scrolling=true\ngtk-decoration-layout=close,minimize,maximize:\n"
        );
    }

    #[test]
    fn the_same_settings_again_leave_the_text_as_it_was() {
        let once = edit_settings_ini("[Settings]\ngtk-theme-name=Mine\n", &keys(false));
        assert_eq!(edit_settings_ini(&once, &keys(false)), once);
    }

    #[test]
    fn a_place_takes_ours_first_and_after_a_change_of_ours_and_keeps_the_person_s_own_otherwise() {
        let mut written = Written::default();
        assert!(written.takes("p", "a", Some("mine")), "nothing written yet");
        written.set("p".into(), "a");
        assert!(written.takes("p", "a", Some("a")));
        assert!(!written.takes("p", "a", Some("mine")), "the person's own");
        assert!(!written.takes("p", "a", None), "the person took it out");
        assert!(written.takes("p", "b", Some("mine")), "ours changed");
    }

    #[test]
    fn what_was_written_outlasts_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state/cupertino/toolkit-settings");
        let mut written = Written::load(Some(file.clone()));
        written.set("gsettings s k".into(), "'a:b'");
        written.set("gtk-3.0/settings.ini k".into(), "x");
        written.save().unwrap();
        let again = Written::load(Some(file));
        assert_eq!(again.get("gsettings s k"), Some("'a:b'"));
        assert_eq!(again.get("gtk-3.0/settings.ini k"), Some("x"));
        assert_eq!(again.get("other"), None);
    }

    #[test]
    fn gsettings_is_told_whether_scroll_bars_overlay_the_content_and_where_the_buttons_go() {
        let keys = ToolkitSettings {
            overlay_scroll_bars: false,
        }
        .gsettings();
        assert_eq!(
            keys,
            [
                (
                    "org.gnome.desktop.interface",
                    "overlay-scrolling",
                    "false".to_owned()
                ),
                (
                    "org.gnome.desktop.wm.preferences",
                    "button-layout",
                    "'close,minimize,maximize:'".to_owned()
                )
            ]
        );
    }
}
