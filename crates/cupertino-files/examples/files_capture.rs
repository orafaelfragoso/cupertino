//! The Files surfaces for look checks: `files/window`, a browser window (920 × 436 at 1×) on a sample home folder,
//! and `files/sidebar`, the sidebar column of that window.
//!
//! `ci/capture.sh files/sidebar --drop-place Desktop --scale 2` (the flags are `cupertino-capture`'s)
//!
//! State flags: `--view icon|list` (default icon), `--drop-place NAME` (the sidebar place) or `--drop-item NAME` (the
//! item), with the drop look of a drag over it, `--selected NAME` (the item, selected), and `--devices removable` (three
//! removable file systems under Locations: "Backup Disk" mounted, "Camera Card" busy, "KINGSTON" not mounted). The
//! home folder `avery` is made
//! under a temporary directory with the user folders and sample files; the places are those of a session with that
//! home on a computer named `cupertino`. File types come from the system's MIME database. The sidebar material is
//! the server's: here a flat fill of the window background stands in for it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use cupertino_assets::Assets;
use cupertino_capture::{App, Request, State};
use cupertino_files::browser::Browser;
use cupertino_files::geom::Size;
use cupertino_files::metrics::window as window_metrics;
use cupertino_files::places::{folder_icons, standard_places, Place};
use cupertino_files::render::{Palette, Renderer};
use cupertino_files::selection::Modifiers;
use cupertino_files::sort::SortSpec;
use cupertino_files::window::{Bars, DropOutline, ViewMode, WindowPainter, WindowState};
use cupertino_text_field::TextField;
use cupertino_theme::{Appearance, SemanticColor, Theme};
use tiny_skia::{Color, IntRect, Pixmap};

/// The sample home folder's name, the window's title.
const HOME: &str = "avery";

/// The user folders of the home folder and their `user-dirs.dirs` keys.
const USER_DIRS: [(&str, &str); 6] = [
    ("DESKTOP", "Desktop"),
    ("DOCUMENTS", "Documents"),
    ("DOWNLOAD", "Downloads"),
    ("MUSIC", "Music"),
    ("PICTURES", "Pictures"),
    ("VIDEOS", "Videos"),
];

/// The sample files: name, size in bytes and age in days.
const FILES: [(&str, u64, u64); 10] = [
    ("IMG_0042.png", 3_400_000, 5),
    ("Résumé 2026.pdf", 184_000, 12),
    ("notes.txt", 2_100, 0),
    (
        "Quarterly report with a very long name that must wrap and truncate in the middle.xlsx",
        912_000,
        30,
    ),
    ("holiday-video.mp4", 840_000_000, 90),
    ("song.mp3", 5_400_000, 20),
    ("archive.tar.gz", 48_000_000, 7),
    ("main.rs", 4_200, 0),
    ("Screenshot 2026-10-22 at 09.41.12.png", 1_200_000, 0),
    ("Website.md", 9_800, 3),
];

/// The capture's clock, 2026-01-01 15:00 UTC: the sample files' ages count back from it and the list's relative
/// dates read from it, in UTC, so a surface is the same pixels on every run.
const NOW: Duration = Duration::from_secs(1_767_279_600);

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + NOW
}

/// How long the listing of the sample folder may take.
const LISTING: Duration = Duration::from_secs(5);

fn io(e: std::io::Error) -> String {
    e.to_string()
}

/// The sample home folder under `root`: the user folders (an hour old) and the sample files (sparse, with their ages),
/// dated by the capture's clock.
fn home(root: &Path) -> Result<PathBuf, String> {
    let home = root.join(HOME);
    for (_, name) in USER_DIRS {
        let dir = home.join(name);
        std::fs::create_dir_all(&dir).map_err(io)?;
        std::fs::File::open(&dir)
            .and_then(|d| d.set_modified(now() - Duration::from_secs(3_600)))
            .map_err(io)?;
    }
    for (name, size, days) in FILES {
        let file = std::fs::File::create(home.join(name)).map_err(io)?;
        file.set_len(size).map_err(io)?;
        file.set_modified(now() - Duration::from_secs(days * 86_400 + 3_600))
            .map_err(io)?;
    }
    Ok(home)
}

struct Capture {
    painter: WindowPainter,
    theme: Theme,
    browser: Browser,
    places: Vec<Place>,
    /// The sample home folder.
    _root: tempfile::TempDir,
}

impl Capture {
    fn new(assets: Assets) -> Result<Self, String> {
        let theme = Theme::load().map_err(|e| e.to_string())?;
        let root = tempfile::tempdir().map_err(io)?;
        let home = home(root.path())?;
        let user_dirs: Vec<(String, PathBuf)> = USER_DIRS
            .iter()
            .map(|(key, name)| (key.to_string(), home.join(name)))
            .collect();
        let places = standard_places(
            &home,
            &user_dirs,
            &root.path().join("no-trash"),
            None,
            "cupertino",
        );
        let folders: HashMap<PathBuf, &'static str> = folder_icons(&home, &user_dirs, &places)
            .into_iter()
            .collect();
        let dirs = cupertino_apps::XdgDirs::from_env().map_err(|e| e.to_string())?;
        let types =
            cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
        let icons = cupertino_icons::document::FileIcons::new(&assets, types.clone(), folders);
        let painter = WindowPainter::new(Renderer::new(icons, &assets).map_err(|e| e.to_string())?);
        let mut browser = Browser::open(
            home.clone(),
            SortSpec::default(),
            root.path().join("recently-used.xbel"),
            Some(Arc::new(types)),
            Arc::new(|| {}),
        )
        .map_err(|e| format!("listing {}: {e}", home.display()))?;
        let end = Instant::now() + LISTING;
        while browser.entries().len() < USER_DIRS.len() + FILES.len() {
            if Instant::now() > end {
                return Err(format!("listing {} took over {LISTING:?}", home.display()));
            }
            browser.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(Capture {
            painter,
            theme,
            browser,
            places,
            _root: root,
        })
    }
}

fn state(flag: &str, value: &str) -> State {
    vec![(flag.to_string(), value.to_string())]
}

impl App for Capture {
    const NAME: &'static str = "files";
    const STATE_FLAGS: &'static [&'static str] =
        &["view", "drop-place", "drop-item", "selected", "devices"];

    fn surfaces(&self) -> Vec<String> {
        vec!["window".into(), "sidebar".into()]
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        match surface {
            "window" => vec![
                State::new(),
                state("view", "list"),
                state("selected", "notes.txt"),
                state("drop-item", "Documents"),
            ],
            _ => vec![
                State::new(),
                state("drop-place", "Desktop"),
                state("devices", "removable"),
            ],
        }
    }

    fn capture(&mut self, r: &Request<'_>) -> Result<Pixmap, String> {
        let mut mode = ViewMode::Icons;
        let mut drop_target = None;
        let mut selected = None;
        let mut places = self.places.clone();
        let item = |name: &str| {
            self.browser
                .entries()
                .iter()
                .position(|e| e.name == name)
                .ok_or_else(|| format!("{name}: no item of the sample folder"))
        };
        for (flag, value) in r.state {
            match (flag.as_str(), value.as_str()) {
                ("view", "icon") => mode = ViewMode::Icons,
                ("view", "list") => mode = ViewMode::List,
                ("view", _) => return Err(format!("--view {value}: icon or list")),
                ("devices", "removable") => places.extend([
                    Place::removable(
                        "Backup Disk",
                        Some(Path::new("/run/media/avery/Backup")),
                        false,
                    ),
                    Place::removable("Camera Card", Some(Path::new("/run/media/avery/CAM")), true),
                    Place::removable("KINGSTON", None, false),
                ]),
                ("devices", _) => return Err(format!("--devices {value}: removable")),
                ("drop-place", name) => {
                    let place = self.places.iter().position(|p| p.name == name);
                    let place = place.ok_or_else(|| format!("{name}: no sidebar place"))?;
                    drop_target = Some(DropOutline::Place(place));
                }
                ("drop-item", _) if drop_target.is_some() => {
                    return Err("--drop-place and --drop-item: one drop target at a time".into());
                }
                ("drop-item", name) => drop_target = Some(DropOutline::Item(item(name)?)),
                (_, name) => selected = Some(item(name)?),
            }
        }
        let path = self.browser.dir().to_path_buf();
        self.browser.press_empty(Modifiers::default());
        if let Some(i) = selected {
            let item = self.browser.entries()[i].path.clone();
            self.browser.select_when_appears(item);
        }
        let (w, h) = r.size.unwrap_or(window_metrics::DEFAULT_SIZE);
        let frame = self.painter.frame(
            Size { w, h },
            &places,
            Bars::HIDDEN,
            window_metrics::TOOLBAR_SEARCH_FIELD.0,
            &path,
            mode,
            self.browser.outline(),
            0.0,
            r.scale,
        );
        let pal = Palette::from_theme(
            &self.theme,
            if r.look.dark {
                Appearance::Dark
            } else {
                Appearance::Light
            },
        );
        let state = WindowState {
            browser: &self.browser,
            places: &places,
            mode,
            active: true,
            search: &TextField::new(),
            search_focused: false,
            search_caret: false,
            title: HOME,
            now: now(),
            edit: None,
            edit_caret: false,
            drop_target,
            increase_contrast: r.look.increase_contrast,
            marquee: None,
            status: "",
            sidebar_focus: None,
        };
        let scale = f64::from(r.scale);
        let mut pm = Pixmap::new((w * scale).ceil() as u32, (h * scale).ceil() as u32)
            .ok_or_else(|| format!("{w} × {h} pt at {scale}× is too large"))?;
        let appearance = if r.look.dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        let [red, green, blue, alpha] = self
            .theme
            .tokens
            .semantic(appearance, SemanticColor::WindowBackground)
            .ok_or("the theme has no window background")?
            .to_array();
        pm.fill(
            Color::from_rgba(red, green, blue, alpha)
                .ok_or("the window background is out of range")?,
        );
        self.painter.paint(&mut pm, r.scale, &frame, &state, &pal);
        if r.surface == "window" {
            return Ok(pm);
        }
        let s = frame.window.sidebar;
        let rect = IntRect::from_xywh(
            (s.x * scale).round() as i32,
            (s.y * scale).round() as i32,
            (s.w * scale).round() as u32,
            (s.h * scale).round() as u32,
        )
        .ok_or("the sidebar is empty")?;
        pm.clone_rect(rect)
            .ok_or_else(|| "the sidebar is outside the window".to_string())
    }
}

fn main() -> ExitCode {
    std::env::set_var("TZ", "UTC");
    cupertino_capture::run(|| Capture::new(cupertino_capture::assets()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_capture::Look;
    use cupertino_files::render::relative_date;

    #[test]
    fn the_list_window_is_the_same_pixels_on_every_run() {
        std::env::set_var("TZ", "UTC");
        let state = vec![("view".to_string(), "list".to_string())];
        let request = Request {
            surface: "window",
            state: &state,
            look: Look {
                dark: false,
                increase_contrast: false,
                reduce_transparency: false,
            },
            scale: 1.0,
            size: None,
        };
        let mut first = Capture::new(Assets::builtin()).unwrap();
        let notes = first
            .browser
            .entries()
            .iter()
            .find(|e| e.name == "notes.txt");
        let modified = notes.and_then(|e| e.modified).unwrap();
        assert_eq!(relative_date(modified, now()), "Today at 14:00");
        let documents = first
            .browser
            .entries()
            .iter()
            .find(|e| e.name == "Documents");
        let modified = documents.and_then(|e| e.modified).unwrap();
        assert_eq!(relative_date(modified, now()), "Today at 14:00");
        let drawn = first.capture(&request).unwrap();
        let again = Capture::new(Assets::builtin())
            .unwrap()
            .capture(&request)
            .unwrap();
        assert!(
            drawn.data() == again.data(),
            "the second run drew other pixels"
        );
    }
}
