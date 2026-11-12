//! The settings files the panes read and write, through the typed modules of `cupertino-defaults` (one parser for the
//! owning process and this application; atomic writes that keep comments, pins and unknown keys, gap G2), and the
//! values a key has when its file does not set it.

use std::io;
use std::path::{Path, PathBuf};

use cupertino_defaults::accessibility::{
    AccessibilitySettings, PersonalAccessibility, SystemAccessibility, ACCESSIBILITY_ENV,
    ACCESSIBILITY_FILE, SYSTEM_ACCESSIBILITY,
};
use cupertino_defaults::appearance::AppearanceSettings;
use cupertino_defaults::compositor::{CompositorSettings, MinimizeEffectName, ScrollBarsName};
use cupertino_defaults::conf::Problem;
use cupertino_defaults::data_control::DataControlConf;
use cupertino_defaults::desktop_control::{App, Decision, DesktopControlConf, Service};
use cupertino_defaults::dock::{sanitize_large_size, sanitize_tile_size, DockSettings, Position};
use cupertino_defaults::keyboard::KeyboardSettings;
use cupertino_defaults::menubar::MenuBarSettings;
use cupertino_defaults::notifications::NotificationSettings;
use cupertino_defaults::power::PowerSettings;
use cupertino_defaults::screen_sharing::ScreenSharingConf;
use cupertino_defaults::system::read_root_file;

use crate::{clipboard, desktop_control, mapping, screen_sharing};

/// A settings file the panes write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum File {
    /// `dock.conf`, read by the dock process.
    Dock,
    /// `compositor.json`, read by the server.
    Compositor,
    /// `menubar.conf`, read by the menu bar process.
    MenuBar,
    /// `keyboard.conf`, read by the server.
    Keyboard,
    /// `power.conf`, read by the server.
    Power,
    /// `data-control.conf`, read by the server: the applications allowed to use the clipboard.
    DataControl,
    /// `screen-sharing.conf`, read by the portal: the displays apps may share again without asking.
    ScreenSharing,
    /// `notifications.conf`, read by the notification centre: what notifications show on the lock screen.
    Notifications,
    /// `desktop-control.conf`, read and written by the window server: the apps that may operate the desktop or take
    /// images of the screen.
    DesktopControl,
    /// `appearance.conf`, read by every program that draws with the toolkit and by the menu bar's schedule.
    Appearance,
}

impl File {
    pub const ALL: [File; 10] = [
        File::Dock,
        File::Compositor,
        File::MenuBar,
        File::Keyboard,
        File::Power,
        File::DataControl,
        File::ScreenSharing,
        File::Notifications,
        File::DesktopControl,
        File::Appearance,
    ];

    /// The process that applies the file.
    pub fn owner(self) -> &'static str {
        match self {
            File::Dock => "the dock",
            File::Compositor
            | File::Keyboard
            | File::Power
            | File::DataControl
            | File::DesktopControl => "the window server",
            File::MenuBar => "the menu bar",
            File::Appearance => "each app",
            File::ScreenSharing => "the portal",
            File::Notifications => "the notification centre",
        }
    }

    /// Whether the owning process applies a change of the file while it runs. All of them do (the live settings of
    /// the server's `live` module, the dock's, the menu bar's and the notification centre's watchers; the portal reads its
    /// file at each restore; the server follows `desktop-control.conf` and ends a session its app may no longer hold).
    pub fn reloads(self) -> bool {
        true
    }
}

/// Where the files are, from the environment (each file's override variable, else the settings directory); `None`
/// when neither names a directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paths {
    pub dock: Option<PathBuf>,
    pub compositor: Option<PathBuf>,
    pub menubar: Option<PathBuf>,
    pub keyboard: Option<PathBuf>,
    pub power: Option<PathBuf>,
    /// `accessibility.conf`: read only; the server is its only writer (`Session1.SetAccessibility`).
    pub accessibility: Option<PathBuf>,
    /// The machine's accessibility defaults (`SYSTEM_ACCESSIBILITY`), under every key `accessibility.conf` does not record.
    pub system_accessibility: Option<PathBuf>,
    pub data_control: Option<PathBuf>,
    pub screen_sharing: Option<PathBuf>,
    pub notifications: Option<PathBuf>,
    pub desktop_control: Option<PathBuf>,
    pub appearance: Option<PathBuf>,
    /// The installed screen reader (`installed_screen_reader`), which the session's compositor runs; `None` without one.
    /// Settings only shows whether there is one: it never starts it.
    pub screen_reader: Option<PathBuf>,
}

impl Paths {
    pub fn from_env() -> Self {
        Paths {
            dock: DockSettings::path(),
            compositor: CompositorSettings::path(),
            menubar: MenuBarSettings::path(),
            keyboard: KeyboardSettings::path(),
            power: PowerSettings::path(),
            accessibility: cupertino_defaults::config_file(ACCESSIBILITY_ENV, ACCESSIBILITY_FILE),
            system_accessibility: Some(PathBuf::from(SYSTEM_ACCESSIBILITY)),
            data_control: DataControlConf::path(),
            screen_sharing: ScreenSharingConf::path(),
            notifications: NotificationSettings::path(),
            desktop_control: DesktopControlConf::path(),
            appearance: AppearanceSettings::path(),
            screen_reader: cupertino_defaults::accessibility::installed_screen_reader(),
        }
    }

    pub fn of(&self, file: File) -> Option<&PathBuf> {
        match file {
            File::Dock => self.dock.as_ref(),
            File::Compositor => self.compositor.as_ref(),
            File::MenuBar => self.menubar.as_ref(),
            File::Keyboard => self.keyboard.as_ref(),
            File::Power => self.power.as_ref(),
            File::DataControl => self.data_control.as_ref(),
            File::ScreenSharing => self.screen_sharing.as_ref(),
            File::Notifications => self.notifications.as_ref(),
            File::DesktopControl => self.desktop_control.as_ref(),
            File::Appearance => self.appearance.as_ref(),
        }
    }
}

/// What the files hold.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub dock: DockSettings,
    pub compositor: CompositorSettings,
    pub menubar: MenuBarSettings,
    /// `appearance.conf`: the accent, the highlight, the schedule and how windows behave.
    pub appearance: AppearanceSettings,
    pub keyboard: KeyboardSettings,
    pub power: PowerSettings,
    pub accessibility: AccessibilitySettings,
    /// `data-control.conf`, empty without a path.
    pub clipboard: clipboard::List,
    /// `screen-sharing.conf`, empty without a path.
    pub screen_sharing: screen_sharing::Grants,
    /// `notifications.conf`: the lock screen's policy, by default and by application.
    pub notifications: NotificationSettings,
    /// `desktop-control.conf`, empty without a path. A [`Settings::reload`] keeps it as the pane shows it, so that a
    /// committed row names the app it showed; [`Settings::reload_desktop_control`] reads it again.
    pub desktop_control: desktop_control::Answers,
    /// The height of the output the dock sits on: its default tile size depends on it (`default_tile_size`).
    pub display_height: f64,
}

/// What a write did not manage.
#[derive(Debug)]
pub enum StoreError {
    /// No settings directory (neither the file's variable, `XDG_CONFIG_HOME` nor `HOME` is set).
    NoPath(File),
    Io(File, io::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NoPath(_) => {
                write!(f, "There is no settings folder to save the change in.")
            }
            StoreError::Io(..) => write!(f, "The change could not be saved."),
        }
    }
}

impl std::error::Error for StoreError {}

impl Settings {
    /// Read every file at `paths`; a missing file sets nothing. A line the file's parser rejects is reported on stderr
    /// and keeps its key unset.
    pub fn load(paths: &Paths, display_height: f64) -> io::Result<Self> {
        Self::read(paths, None, display_height)
    }

    /// Read every file again, as it is now: another writer (the dock's menu, the server, an editor) may have changed it
    /// since. A commit reloads before it changes its key, so a store writes the user's key over the file's current
    /// values and never puts back a value read earlier. A line the parser rejects keeps the value of `self`.
    pub fn reload(&mut self, paths: &Paths) -> io::Result<()> {
        *self = Self::read(paths, Some(self), self.display_height)?;
        Ok(())
    }

    fn read(paths: &Paths, last_good: Option<&Settings>, display_height: f64) -> io::Result<Self> {
        let lines = |file: &str, problems: Vec<Problem>| {
            problems.iter().for_each(|p| {
                eprintln!("cupertino-settings: {file} line {}: {}", p.line, p.message)
            })
        };
        let dock = match &paths.dock {
            Some(p) => DockSettings::load(p, last_good.map(|l| &l.dock)).map(|(s, problems)| {
                lines("dock.conf", problems);
                s
            })?,
            None => DockSettings::default(),
        };
        let compositor = match &paths.compositor {
            Some(p) => CompositorSettings::load(p, last_good.map(|l| &l.compositor)).map(
                |(s, problems)| {
                    problems.iter().for_each(|p| {
                        eprintln!(
                            "cupertino-settings: compositor.json {}: {}",
                            p.key, p.message
                        )
                    });
                    s
                },
            )?,
            None => CompositorSettings::default(),
        };
        let menubar = match &paths.menubar {
            Some(p) => {
                MenuBarSettings::load(p, last_good.map(|l| &l.menubar)).map(|(s, problems)| {
                    lines("menubar.conf", problems);
                    s
                })?
            }
            None => MenuBarSettings::UNSET,
        };
        let appearance = match &paths.appearance {
            Some(p) => AppearanceSettings::load(p, last_good.map(|l| &l.appearance)).map(
                |(s, problems)| {
                    lines("appearance.conf", problems);
                    s
                },
            )?,
            None => AppearanceSettings::UNSET,
        };
        let keyboard = match &paths.keyboard {
            Some(p) => {
                KeyboardSettings::load(p, last_good.map(|l| &l.keyboard)).map(|(s, problems)| {
                    lines("keyboard.conf", problems);
                    s
                })?
            }
            None => KeyboardSettings::default(),
        };
        let power = match &paths.power {
            Some(p) => {
                PowerSettings::load(p, last_good.map(|l| &l.power)).map(|(s, problems)| {
                    lines("power.conf", problems);
                    s
                })?
            }
            None => PowerSettings::default(),
        };
        let system = paths
            .system_accessibility
            .as_deref()
            .and_then(system_accessibility)
            .unwrap_or_default();
        let accessibility = match &paths.accessibility {
            Some(p) => PersonalAccessibility::load(p).map(|(personal, problems)| {
                problems.iter().for_each(|(line, message)| {
                    eprintln!("cupertino-settings: accessibility.conf line {line}: {message}")
                });
                personal.over(&system)
            })?,
            None => system,
        };
        let clipboard = paths
            .data_control
            .as_deref()
            .map(clipboard::List::load)
            .unwrap_or_default();
        let screen_sharing = paths
            .screen_sharing
            .as_deref()
            .map(screen_sharing::Grants::load)
            .unwrap_or_default();
        let notifications = match &paths.notifications {
            Some(p) => NotificationSettings::load(p, last_good.map(|l| &l.notifications)).map(
                |(s, problems)| {
                    lines("notifications.conf", problems);
                    s
                },
            )?,
            None => NotificationSettings::default(),
        };
        // a row commits by its place in the list shown: the list read again could hold another app there
        let desktop_control = match last_good {
            Some(last) => last.desktop_control.clone(),
            None => load_desktop_control(paths),
        };
        Ok(Settings {
            dock,
            compositor,
            menubar,
            appearance,
            keyboard,
            power,
            accessibility,
            clipboard,
            screen_sharing,
            notifications,
            desktop_control,
            display_height,
        })
    }

    /// Take the accessibility settings from `held`, the server's `Accessibility` property (every key of the file with
    /// its value as the file writes it). A key or value this build does not understand is skipped and named in the
    /// returned messages.
    pub fn apply_session_accessibility(
        &mut self,
        held: &std::collections::HashMap<String, String>,
    ) -> Vec<String> {
        held.iter()
            .filter_map(|(key, value)| {
                self.accessibility
                    .set(key, value)
                    .err()
                    .map(|e| format!("the session's accessibility setting {key}: {e}"))
            })
            .collect()
    }

    /// Store the resolution `dpi` of the mouse `key` in `compositor.json` over the file as it is now, so a key the
    /// server wrote since the last read (the appearance) is not put back; on a failure the settings stay as they were.
    pub fn set_mouse_dpi(&mut self, paths: &Paths, key: &str, dpi: u32) -> Result<(), String> {
        self.rewrite_compositor(paths, |c| c.set_mouse_dpi(key, Some(dpi)))
    }

    /// Change `compositor.json` over the file as it is now, so a key another writer set since the last read is not put
    /// back; on a failure the settings stay as they were.
    pub fn rewrite_compositor(
        &mut self,
        paths: &Paths,
        change: impl FnOnce(
            &mut cupertino_defaults::compositor::CompositorSettings,
        ) -> Result<(), String>,
    ) -> Result<(), String> {
        let before = self.clone();
        let outcome = self
            .reload(paths)
            .map_err(|e| {
                eprintln!("cupertino-settings: the settings could not be read: {e}");
                "The settings could not be read.".to_string()
            })
            .and_then(|()| change(&mut self.compositor))
            .and_then(|()| {
                self.store(paths, File::Compositor)
                    .map_err(|e| e.to_string())
            });
        if outcome.is_err() {
            *self = before;
        }
        outcome
    }

    /// List the folder `folder` in the Wallpaper pane.
    pub fn add_wallpaper_folder(
        &mut self,
        paths: &Paths,
        folder: &std::path::Path,
    ) -> Result<(), String> {
        if !folder.is_absolute() {
            return Err("That folder is not an absolute path.".to_string());
        }
        let folder = folder.to_string_lossy().into_owned();
        self.rewrite_compositor(paths, |c| {
            let folders = c.wallpaper_folders.get_or_insert_with(Vec::new);
            if !folders.contains(&folder) {
                folders.push(folder);
            }
            Ok(())
        })
    }

    /// Read `data-control.conf` again (when the Privacy pane shows and after each write).
    pub fn reload_clipboard(&mut self, paths: &Paths) {
        self.clipboard = paths
            .data_control
            .as_deref()
            .map(clipboard::List::load)
            .unwrap_or_default();
    }

    /// Read `screen-sharing.conf` again (when the Privacy pane shows and after each write): the portal adds to it.
    pub fn reload_screen_sharing(&mut self, paths: &Paths) {
        self.screen_sharing = paths
            .screen_sharing
            .as_deref()
            .map(screen_sharing::Grants::load)
            .unwrap_or_default();
    }

    /// Take the screen-sharing grant `id` out of `screen-sharing.conf` and read the file again. The file is read and
    /// written under its lock ([`ScreenSharingConf::update`]) and only that grant goes: one the portal added since the
    /// pane read the file stays. `Err` is the sentence the pane shows; nothing was written. Either way the pane then
    /// shows the file as it is.
    pub fn forget_grant(&mut self, paths: &Paths, id: &str) -> Result<(), String> {
        let path = paths
            .screen_sharing
            .as_deref()
            .ok_or_else(|| StoreError::NoPath(File::ScreenSharing).to_string())?;
        let forgotten = ScreenSharingConf::update(path, |conf| {
            conf.revoke(id);
            Ok(())
        })
        .map_err(|e| {
            eprintln!("cupertino-settings: the screen-sharing grant could not be forgotten: {e}");
            StoreError::Io(File::ScreenSharing, e).to_string()
        });
        // the pane took the row out at once; it shows the file as it is, so a grant not forgotten shows again
        self.reload_screen_sharing(paths);
        forgotten
    }

    /// Read `desktop-control.conf` again: when the Privacy pane shows, when the file changes and after each change the
    /// pane makes. The compositor adds an app to it the first time the app asks.
    pub fn reload_desktop_control(&mut self, paths: &Paths) {
        self.desktop_control = load_desktop_control(paths);
    }

    /// Record `decision` for `app` and `service` in `desktop-control.conf` and read the file again. The file is read
    /// and written under its lock ([`DesktopControlConf::update`]) and only that app's answer changes, while the file
    /// still holds it: an app removed meanwhile stays removed and asks again. `Err` is the sentence the pane shows;
    /// nothing was written. Either way the pane then shows the file as it is.
    pub fn decide_app(
        &mut self,
        paths: &Paths,
        service: Service,
        app: &App,
        decision: Decision,
    ) -> Result<(), String> {
        self.change_desktop_control(paths, |conf| {
            if conf.decision(service, app).is_some() {
                conf.set(service, app, decision).map_err(io::Error::other)?;
            }
            Ok(())
        })
    }

    /// Take the answer of `app` for `service` out of `desktop-control.conf`, so the app asks again at its next
    /// request, and read the file again; as [`Settings::decide_app`], only that answer goes.
    pub fn forget_app(&mut self, paths: &Paths, service: Service, app: &App) -> Result<(), String> {
        self.change_desktop_control(paths, |conf| {
            conf.forget(service, app);
            Ok(())
        })
    }

    fn change_desktop_control(
        &mut self,
        paths: &Paths,
        change: impl FnOnce(&mut DesktopControlConf) -> io::Result<()>,
    ) -> Result<(), String> {
        let path = paths
            .desktop_control
            .as_deref()
            .ok_or_else(|| StoreError::NoPath(File::DesktopControl).to_string())?;
        let changed = DesktopControlConf::update(path, change).map_err(|e| {
            eprintln!("cupertino-settings: desktop-control.conf could not be changed: {e}");
            StoreError::Io(File::DesktopControl, e).to_string()
        });
        // the pane showed the change at once; it shows the file as it is, so a change not made shows undone
        self.reload_desktop_control(paths);
        changed
    }

    /// Let `program` use the clipboard: its entry ([`clipboard::entry_for`], so a link becomes its file) is added and
    /// the file written. `Err` is the sentence the chooser shows; nothing changed.
    pub fn allow_clipboard(
        &mut self,
        paths: &Paths,
        program: &std::path::Path,
    ) -> Result<(), String> {
        let entry = clipboard::entry_for(program)?;
        let mut list = self.clipboard.clone();
        list.conf.allowed.retain(|e| e.program != entry.program);
        list.conf.allowed.push(entry);
        self.store_clipboard(paths, list)
    }

    /// Take `program` off Clipboard Access and write the file.
    pub fn unallow_clipboard(
        &mut self,
        paths: &Paths,
        program: &std::path::Path,
    ) -> Result<(), String> {
        let mut list = self.clipboard.clone();
        list.conf.allowed.retain(|e| e.program != program);
        self.store_clipboard(paths, list)
    }

    fn store_clipboard(&mut self, paths: &Paths, list: clipboard::List) -> Result<(), String> {
        let path = paths
            .data_control
            .as_deref()
            .ok_or_else(|| StoreError::NoPath(File::DataControl).to_string())?;
        list.store(path).map_err(|e| e.to_string())?;
        self.reload_clipboard(paths);
        Ok(())
    }

    /// Show the picture `picture` on the desktop in both appearances.
    pub fn set_wallpaper(
        &mut self,
        paths: &Paths,
        picture: &std::path::Path,
    ) -> Result<(), String> {
        if !picture.is_absolute() {
            return Err("That picture is not an absolute path.".to_string());
        }
        let picture = picture.to_string_lossy().into_owned();
        self.rewrite_compositor(paths, |c| {
            c.wallpaper = Some(picture);
            c.wallpaper_dark = None;
            Ok(())
        })
    }

    /// Write `file` to its path atomically.
    pub fn store(&self, paths: &Paths, file: File) -> Result<(), StoreError> {
        let path = paths.of(file).ok_or(StoreError::NoPath(file))?;
        let stored = match file {
            File::Dock => self.dock.store(path).map(drop),
            File::Compositor => self.compositor.store(path).map(drop),
            File::MenuBar => self.menubar.store(path).map(drop),
            File::Appearance => self.appearance.store(path).map(drop),
            File::Keyboard => self.keyboard.store(path).map(drop),
            File::Power => self.power.store(path).map(drop),
            File::DataControl => self.clipboard.store(path),
            File::Notifications => self.notifications.store(path).map(drop),
            // the portal adds grants while Settings runs, so the list read earlier is never written back whole
            File::ScreenSharing => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "screen-sharing.conf is changed one grant at a time",
            )),
            // the compositor adds answers while Settings runs, so the list read earlier is never written back whole
            File::DesktopControl => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "desktop-control.conf is changed one answer at a time",
            )),
        };
        stored.map_err(|e| {
            eprintln!("cupertino-settings: the {file:?} settings could not be saved: {e}");
            StoreError::Io(file, e)
        })
    }

    // ---- the values in effect: the file's, else the key's default ------------------------------------------------

    /// The dock's tile size in points (the dock's own sanitizing rule and display-height default).
    pub fn tile_size(&self) -> f64 {
        sanitize_tile_size(self.dock.tile_size, self.display_height)
    }

    pub fn large_size(&self) -> f64 {
        sanitize_large_size(self.dock.large_size)
    }

    /// Defaults: magnification off, auto-hide off, recents shown, at the bottom.
    pub fn magnification(&self) -> bool {
        self.dock.magnification.unwrap_or(false)
    }

    pub fn autohide(&self) -> bool {
        self.dock.autohide.unwrap_or(false)
    }

    pub fn recents(&self) -> bool {
        self.dock.recents.unwrap_or(true)
    }

    pub fn position(&self) -> Position {
        self.dock.position.unwrap_or(Position::Bottom)
    }

    /// Default: warp (the server's default effect).
    pub fn minimize_effect(&self) -> MinimizeEffectName {
        self.compositor
            .minimize_effect
            .unwrap_or(MinimizeEffectName::Warp)
    }

    /// Defaults: 0.6875 for both (settled), acceleration on, natural scrolling on for the trackpad and off for the mouse.
    pub fn mouse_speed(&self) -> f64 {
        self.compositor
            .mouse_speed
            .unwrap_or(mapping::TRACKING_DEFAULT)
    }

    pub fn trackpad_speed(&self) -> f64 {
        self.compositor
            .trackpad_speed
            .unwrap_or(mapping::TRACKING_DEFAULT)
    }

    pub fn mouse_acceleration(&self) -> bool {
        self.compositor.mouse_acceleration.unwrap_or(true)
    }

    /// Automatic unless the file says otherwise.
    pub fn show_scroll_bars(&self) -> ScrollBarsName {
        self.compositor.show_scroll_bars.unwrap_or_default()
    }

    pub fn trackpad_acceleration(&self) -> bool {
        self.compositor.trackpad_acceleration.unwrap_or(true)
    }

    pub fn natural_scrolling(&self) -> bool {
        self.compositor.natural_scrolling.unwrap_or(true)
    }

    pub fn mouse_natural_scrolling(&self) -> bool {
        self.compositor.mouse_natural_scrolling.unwrap_or(false)
    }

    /// Defaults: 500 ms delay, 83 ms interval (settled).
    pub fn repeat_delay_ms(&self) -> u32 {
        self.keyboard.repeat_delay_ms.unwrap_or(500)
    }

    pub fn repeat_interval_ms(&self) -> u32 {
        self.keyboard.repeat_interval_ms.unwrap_or(83)
    }
}

/// `desktop-control.conf` as the pane shows it; no answer without a path.
fn load_desktop_control(paths: &Paths) -> desktop_control::Answers {
    paths
        .desktop_control
        .as_deref()
        .map(desktop_control::Answers::load)
        .unwrap_or_default()
}

/// The machine's accessibility defaults in `path` (`SYSTEM_ACCESSIBILITY`) as the login screen reads them: `None`
/// without a file or with one the readers refuse (not root's, writable by others, a link, unreadable).
pub fn system_accessibility(path: &Path) -> Option<AccessibilitySettings> {
    let text = read_root_file(path).ok()??;
    Some(SystemAccessibility::from_text(&text).0.settings)
}
