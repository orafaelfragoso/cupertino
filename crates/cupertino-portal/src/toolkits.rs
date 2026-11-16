//! The session's settings for toolkits that read them elsewhere than from the portal:
//!
//! - GSettings, in the `org.gnome.desktop.interface` schema: GTK 3 reads it on Wayland, GTK 4 when no portal answers,
//!   and the GTK backend of the portal serves it too, so both backends give the same value. Set with `gsettings`.
//! - `gtk-3.0/settings.ini` and `gtk-4.0/settings.ini` in the configuration directory, the `[Settings]` group: GTK reads
//!   them when it starts under X11, where the X display publishes none of these settings. Only the keys below are
//!   written; every other line stays as it was.
//!
//! A GTK application on Wayland follows a change at once; one under X11 takes it when it starts.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;

use crate::settings::Appearance;

/// What the toolkits are told.
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
/// The GTK settings files under the configuration directory.
pub const SETTINGS_FILES: [&str; 2] = ["gtk-3.0/settings.ini", "gtk-4.0/settings.ini"];
/// The group of a GTK settings file that holds its settings.
const GROUP: &str = "[Settings]";

impl ToolkitSettings {
    /// The `gsettings` argument lists that set these values (each as `set SCHEMA KEY VALUE`).
    pub fn gsettings(self) -> Vec<[String; 4]> {
        vec![[
            "set".into(),
            INTERFACE_SCHEMA.into(),
            "overlay-scrolling".into(),
            self.overlay_scroll_bars.to_string(),
        ]]
    }

    /// The keys of a GTK settings file and their values.
    pub fn settings_ini(self) -> Vec<(&'static str, String)> {
        vec![(
            "gtk-overlay-scrolling",
            self.overlay_scroll_bars.to_string(),
        )]
    }
}

/// `text`, a GTK settings file, with `keys` set in its `[Settings]` group: a key already there gets the new value in
/// its place, a missing one is added at the end of the group, and the group is added when there is none.
pub fn edit_settings_ini(text: &str, keys: &[(&'static str, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let header = |l: &str| l.trim_start().starts_with('[');
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
        .position(|l| header(l))
        .map_or(lines.len(), |i| start + 1 + i);
    for (key, value) in keys {
        let entry = format!("{key}={value}");
        let found = (start + 1..end).find(|&i| {
            lines[i]
                .split_once('=')
                .is_some_and(|(k, _)| k.trim() == *key)
        });
        match found {
            Some(i) => lines[i] = entry,
            None => {
                // after the group's last entry, before the blank lines that part it from the next group
                let mut at = end;
                while at > start + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                lines.insert(at, entry);
                end += 1;
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Writes `settings` into the GTK settings files under `config_home`, creating them when missing. A file left as it was
/// is not written; a link is followed, so the file it names is changed and the link stays.
pub fn write_settings_ini(config_home: &Path, settings: ToolkitSettings) -> io::Result<()> {
    let keys = settings.settings_ini();
    for name in SETTINGS_FILES {
        let path = config_home.join(name);
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let old = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let new = edit_settings_ini(&old, &keys);
        if new == old {
            continue;
        }
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let temporary = dir.join(".settings.ini.cupertino");
        std::fs::write(&temporary, new)?;
        std::fs::rename(&temporary, &path)?;
    }
    Ok(())
}

/// Runs each `gsettings` argument list of `settings` with `gsettings` (a fresh command for each).
pub fn write_gsettings(
    settings: ToolkitSettings,
    gsettings: &dyn Fn() -> Command,
) -> io::Result<()> {
    for args in settings.gsettings() {
        let status = gsettings().args(&args).status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "gsettings {} exited with {status}",
                args.join(" ")
            )));
        }
    }
    Ok(())
}

/// Hands each new [`ToolkitSettings`] to the toolkits on a thread of its own, in order, so the portal's loop never
/// waits on a file or on `gsettings`. A failure is reported on standard error and the next change tries again.
pub struct Writer {
    sender: mpsc::Sender<ToolkitSettings>,
}

impl Writer {
    /// Starts the thread. `config_home` holds the GTK settings files (none are written without it); `gsettings`
    /// makes the command that runs `gsettings`.
    pub fn start(
        config_home: Option<PathBuf>,
        gsettings: impl Fn() -> Command + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, received) = mpsc::channel::<ToolkitSettings>();
        std::thread::Builder::new()
            .name("toolkits".to_owned())
            .spawn(move || {
                while let Ok(mut settings) = received.recv() {
                    // only the latest of the changes queued meanwhile matters
                    while let Ok(newer) = received.try_recv() {
                        settings = newer;
                    }
                    if let Err(e) = write_gsettings(settings, &gsettings) {
                        eprintln!("cupertino-portal: GTK applications on Wayland keep their scroll bars: {e}");
                    }
                    if let Some(home) = &config_home {
                        if let Err(e) = write_settings_ini(home, settings) {
                            eprintln!(
                                "cupertino-portal: GTK applications under X11 keep their scroll bars: {e}"
                            );
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
            "[Settings]\ngtk-overlay-scrolling=false\n"
        );
    }

    #[test]
    fn a_key_already_set_changes_in_place_and_every_other_line_stays() {
        let text = "# mine\n[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling = true\ngtk-font-name=Inter 10\n";
        assert_eq!(
            edit_settings_ini(text, &keys(false)),
            "# mine\n[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling=false\ngtk-font-name=Inter 10\n"
        );
    }

    #[test]
    fn a_missing_key_ends_its_group_before_the_next_one() {
        let text = "[Settings]\ngtk-theme-name=Mine\n\n[Other]\ngtk-overlay-scrolling=true\n";
        assert_eq!(
            edit_settings_ini(text, &keys(true)),
            "[Settings]\ngtk-theme-name=Mine\ngtk-overlay-scrolling=true\n\n[Other]\ngtk-overlay-scrolling=true\n"
        );
    }

    #[test]
    fn a_file_without_the_group_keeps_its_lines_and_gets_the_group_after_them() {
        assert_eq!(
            edit_settings_ini("[Other]\na=b", &keys(true)),
            "[Other]\na=b\n\n[Settings]\ngtk-overlay-scrolling=true\n"
        );
    }

    #[test]
    fn the_same_settings_again_leave_the_text_as_it_was() {
        let once = edit_settings_ini("[Settings]\ngtk-theme-name=Mine\n", &keys(false));
        assert_eq!(edit_settings_ini(&once, &keys(false)), once);
    }

    #[test]
    fn gsettings_is_told_whether_scroll_bars_overlay_the_content() {
        let args = ToolkitSettings {
            overlay_scroll_bars: false,
        }
        .gsettings();
        assert_eq!(
            args,
            [[
                "set".to_owned(),
                "org.gnome.desktop.interface".to_owned(),
                "overlay-scrolling".to_owned(),
                "false".to_owned()
            ]]
        );
    }
}
