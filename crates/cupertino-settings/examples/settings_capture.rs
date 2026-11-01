//! The Settings surfaces for look checks: every pane as `settings/<pane-key>` in the default window (715 × 625 at
//! 1×), the Wallpaper pane under its chooser sheet as `settings/wallpaper-chooser`, the Privacy pane under its
//! application chooser as `settings/privacy-chooser` and the results window of a search as `settings/search-results`.
//!
//! `ci/capture.sh settings/accessibility --login saving --scale 2` (the flags are `cupertino-capture`'s)
//!
//! State flags: `--login none|saving|refused` (Accessibility: no login screen service, a write waiting for its reply,
//! a write refused; default: the service with no file), `--mice one|two` (Mouse; default one), `--clipboard
//! empty|refused` (Privacy: no entries, a refused file; default: an allowed, a changed and a missing entry and a line
//! that allows nothing) and `--scroll PT` (the
//! pane scrolled by PT points, clamped to the form as the app does). The settings are
//! the defaults (no files are read); the live readings are example data, not a machine's. The sidebar material is the
//! server's: here a flat fill of the window background stands in for it.

use std::process::ExitCode;

use cupertino_apps::entry::{parse_desktop_entry, Locale, Visibility};
use cupertino_capture::{App, Look, Request, State};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::data_control::{DataControlConf, Entry, Unusable};
use cupertino_settings::clipboard::{self, Installed};
use cupertino_settings::form::{self, SectionSpec};
use cupertino_settings::geom::Size;
use cupertino_settings::history::History;
use cupertino_settings::live::{
    Device, Live, LoginScreen, Mouse, Network, Pictures, Reading, Volume, Wifi,
};
use cupertino_settings::paint::{Painter, Palette, PaneView, WindowState};
use cupertino_settings::pane::PANES;
use cupertino_settings::panes::{self, ControlId, Session};
use cupertino_settings::search::{popup, SearchField};
use cupertino_settings::settings::{Paths, Settings};
use cupertino_settings::sheet::{Sheet, SheetKey};
use cupertino_settings::sidebar::{self, RowSize};
use cupertino_settings::window::{self, WindowLayout};
use cupertino_settings::PaneId;
use cupertino_theme::{Appearance, Theme};
use tiny_skia::{Color, Pixmap};

/// The interface width factor of English (approximate).
const WIDTH_FACTOR: f64 = 1.0;

/// What the Login Screen row says when sessiond refuses a write from someone who is not an administrator.
const NOT_ADMINISTRATOR: &str = "Only an administrator can change the login screen.";

/// A generated picture `name` under `scratch` with its thumbnail set on the painter.
fn picture(
    painter: &mut Painter,
    scratch: &std::path::Path,
    name: &str,
    (from, to): ([u8; 3], [u8; 3]),
) -> cupertino_assets::WallpaperFile {
    let mut pm = Pixmap::new(256, 160).unwrap();
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let t = (i / 256) as f32 / 159.0;
        let mix = |a: u8, b: u8| (f32::from(a) * (1.0 - t) + f32::from(b) * t) as u8;
        *px = tiny_skia::ColorU8::from_rgba(
            mix(from[0], to[0]),
            mix(from[1], to[1]),
            mix(from[2], to[2]),
            255,
        )
        .premultiply();
    }
    let image = scratch.join(format!("{name}.jpg"));
    let thumbnail = scratch.join(format!("{name}-thumb.png"));
    pm.save_png(&thumbnail).unwrap();
    painter.set_thumbnail(&image, &thumbnail);
    cupertino_assets::WallpaperFile {
        name: name.to_string(),
        light: image,
        dark: None,
        credit: None,
    }
}

/// [`picture`] with a title and a photographer, and a dark file when `dark`.
fn credited(
    painter: &mut Painter,
    scratch: &std::path::Path,
    name: &str,
    title: &str,
    author: &str,
    colors: ([u8; 3], [u8; 3]),
    dark: bool,
) -> cupertino_assets::WallpaperFile {
    let mut file = picture(painter, scratch, name, colors);
    file.dark = dark.then(|| scratch.join(format!("{name}-dark.jpg")));
    file.credit = Some(cupertino_assets::Credit {
        title: title.to_string(),
        author: author.to_string(),
    });
    file
}

fn mice(list: &[(&str, &str, u32, bool)]) -> Reading<Vec<Mouse>> {
    Reading::Ready(
        list.iter()
            .map(|(key, name, dpi, stored)| Mouse {
                key: (*key).into(),
                name: (*name).into(),
                dpi: *dpi,
                stored: *stored,
            })
            .collect(),
    )
}

/// Example readings: a Wi-Fi device with four networks, the mixer at 60 %, no battery, one mouse, the login screen
/// service with no file, and generated pictures under `scratch` (bundled ones and a folder).
fn live(painter: &mut Painter, scratch: &std::path::Path) -> Live {
    let network = |ssid: &str, signal, security: &str, saved, connected| Network {
        ssid: ssid.into(),
        name: ssid.into(),
        signal,
        security: security.into(),
        saved,
        connected,
    };
    let wifi = Wifi {
        enabled: true,
        devices: vec![Device {
            index: 2,
            name: "wlan0".into(),
            kind: "wifi".into(),
            state: "connected".into(),
            ssid: "Home".into(),
            failure: String::new(),
            address: "10.0.0.2".into(),
        }],
        state: "connected".into(),
        networks: vec![
            network("Home", -42, "wpa2", true, true),
            network("Office", -61, "wpa2", true, false),
            network("Cafe", -70, "open", false, false),
            network("Neighbour", -80, "wpa2", false, false),
        ],
        joining: None,
        failure: None,
    };
    std::fs::create_dir_all(scratch.join("Trips")).unwrap();
    let bundled = vec![
        credited(
            painter,
            scratch,
            "dusk",
            "Dusk",
            "A. Tester",
            ([250, 170, 120], [60, 50, 120]),
            false,
        ),
        credited(
            painter,
            scratch,
            "ridge",
            "Ridge",
            "B. Tester",
            ([235, 235, 240], [120, 130, 150]),
            true,
        ),
        credited(
            painter,
            scratch,
            "ocean",
            "Ocean",
            "C. Tester",
            ([120, 190, 230], [10, 60, 120]),
            false,
        ),
    ];
    let folder = vec![
        picture(painter, scratch, "meadow", ([170, 220, 170], [40, 110, 90])),
        picture(painter, scratch, "plum", ([200, 140, 200], [70, 30, 90])),
    ];
    let current = bundled[1].light.clone();
    Live {
        wifi: Reading::Ready(wifi),
        volume: Reading::Ready(Volume {
            level: 0.6,
            muted: false,
        }),
        battery: Reading::Ready(None),
        mice: mice(&[(
            "046d:c077:USB Optical Mouse",
            "USB Optical Mouse",
            800,
            false,
        )]),
        login_screen: Some(LoginScreen::default()),
        pictures: Pictures {
            bundled,
            folders: vec![(scratch.join("Trips"), folder)],
            default: Some(current),
            ..Default::default()
        },
        ..Live::default()
    }
}

/// Clipboard Access with an allowed system program, a program of the user changed since it was allowed, one that is
/// gone and a line that allows nothing, and the installed applications the chooser lists.
fn clipboard(scratch: &std::path::Path) -> Result<(clipboard::List, Vec<Installed>), String> {
    use std::os::unix::fs::PermissionsExt;
    let bin = scratch.join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let program = |name: &str, text: &str| -> Result<std::path::PathBuf, String> {
        let path = bin.join(name);
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        Ok(path)
    };
    let clipd = program("clipd", "x")?;
    let pinned = Entry::for_program(&clipd)?;
    program("clipd", "changed since")?;
    for name in ["python3", "flatpak", "notes-sync", "screen-recorder"] {
        program(name, "x")?;
    }
    let history = std::fs::canonicalize("/usr/bin/true").map_err(|e| e.to_string())?;
    let (_, problems) = DataControlConf::parse("allow = python3");
    let list = clipboard::List {
        conf: DataControlConf {
            allowed: vec![
                Entry {
                    program: history.clone(),
                    pin: None,
                },
                pinned,
                Entry {
                    program: "/opt/gone/clipman".into(),
                    pin: None,
                },
            ],
        },
        unusable: problems
            .into_iter()
            .map(|p| Unusable {
                text: "allow = python3".into(),
                message: p.message,
            })
            .collect(),
        unreadable: None,
    };
    let vis = Visibility {
        skip_try_exec: true,
        ..Visibility::default()
    };
    let entry = |id: &str, name: &str, icon: &str, exec: &str| {
        let text =
            format!("[Desktop Entry]\nType=Application\nName={name}\nIcon={icon}\nExec={exec}\n");
        parse_desktop_entry(id, &text, &Locale::parse("C"), &vis)
            .map_err(|e| format!("{id}: {e:?}"))
    };
    let entries = vec![
        entry(
            "history.desktop",
            "Clipboard History",
            "edit-paste",
            &history.display().to_string(),
        )?,
        entry("python.desktop", "Python", "python", "python3")?,
        entry(
            "org.example.Notes.desktop",
            "Notes (Flatpak)",
            "accessories-text-editor",
            "flatpak run org.example.Notes",
        )?,
        entry(
            "notes-sync.desktop",
            "Notes Sync",
            "folder-sync",
            "notes-sync %U",
        )?,
        entry(
            "screen-recorder.desktop",
            "Screen Recorder",
            "media-record",
            "screen-recorder",
        )?,
    ];
    Ok((list, clipboard::installed(entries, &[bin])))
}

fn fill(pm: &mut Pixmap, rgba: [f32; 4]) {
    pm.fill(
        Color::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).expect("palette colours are in range"),
    );
}

fn pixmap(w: f64, h: f64, scale: f32) -> Result<Pixmap, String> {
    Pixmap::new(
        (w * f64::from(scale)).ceil() as u32,
        (h * f64::from(scale)).ceil() as u32,
    )
    .ok_or_else(|| format!("{w} × {h} pt at {scale}× is too large"))
}

/// The palette of the appearance and Increase Contrast.
fn palette(theme: &Theme, look: Look) -> Result<Palette, String> {
    let appearance = match (look.dark, look.increase_contrast) {
        (false, false) => Appearance::Light,
        (true, false) => Appearance::Dark,
        (false, true) => Appearance::LightHighContrast,
        (true, true) => Appearance::DarkHighContrast,
    };
    Palette::from_theme(theme, appearance).map_err(|e| e.to_string())
}

fn state(flag: &str, value: &str) -> State {
    vec![(flag.to_string(), value.to_string())]
}

struct Capture {
    painter: Painter,
    theme: Theme,
    settings: Settings,
    live: Live,
    /// The pictures' files.
    scratch: tempfile::TempDir,
}

impl Capture {
    fn new() -> Result<Self, String> {
        let assets = cupertino_capture::assets()?;
        let theme = Theme::load().map_err(|e| e.to_string())?;
        let mut painter = Painter::new(&assets).map_err(|e| e.to_string())?;
        let paths = Paths {
            dock: None,
            compositor: None,
            menubar: None,
            keyboard: None,
            power: None,
            accessibility: None,
            system_accessibility: None,
            data_control: None,
        };
        let mut settings = Settings::load(&paths, 1080.0).map_err(|e| e.to_string())?;
        let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
        let mut live = live(&mut painter, scratch.path());
        (settings.clipboard, live.apps) = clipboard(scratch.path())?;
        Ok(Capture {
            painter,
            theme,
            settings,
            live,
            scratch,
        })
    }

    /// The window on `pane` drawn with `live`, the Login Screen row refused when `refused`, the pane scrolled by
    /// `scroll` points (clamped to the form, as the app does).
    #[allow(clippy::too_many_arguments)]
    fn window(
        &mut self,
        pane: PaneId,
        live: &Live,
        refused: bool,
        scroll: f64,
        size: Size,
        scale: f32,
        session: Session,
        pal: &Palette,
    ) -> Result<(Pixmap, WindowLayout), String> {
        let layout = WindowLayout::new(
            size,
            WIDTH_FACTOR,
            RowSize::default(),
            &sidebar::groups(&[]),
            false,
        );
        let mut sections = panes::sections(pane, &self.settings, session, live);
        if refused {
            for row in sections.iter_mut().flat_map(|s| &mut s.rows) {
                if row.id == ControlId::LoginScreen {
                    row.error = Some(NOT_ADMINISTRATOR.into());
                }
            }
        }
        let specs: Vec<SectionSpec<'_>> = sections
            .iter()
            .map(|s| SectionSpec {
                header: s.header.is_some(),
                rows: s.rows.iter().map(|r| r.spec()).collect(),
                footer: s.footer.is_some(),
            })
            .collect();
        let (form, form_height) =
            form::layout(&specs, layout.content.x, layout.content.w, layout.content.y);
        let scroll = scroll.clamp(0.0, (form_height - layout.content.h).max(0.0));
        let history = History::new(pane);
        let search = SearchField::default();
        let state = WindowState {
            selected: pane,
            has_battery: live.has_battery(),
            active: true,
            history: &history,
            search: &search,
            search_focused: false,
            search_caret: false,
        };
        let mut pm = pixmap(size.w, size.h, scale)?;
        fill(&mut pm, pal.window_background);
        self.painter.paint_window(
            &mut pm,
            scale,
            &layout,
            &state,
            &PaneView {
                sections: &sections,
                layout: &form,
                scroll,
                dragging: None,
            },
            pal,
        );
        Ok((pm, layout))
    }
}

impl App for Capture {
    const NAME: &'static str = "settings";
    const STATE_FLAGS: &'static [&'static str] = &["login", "mice", "clipboard", "scroll"];

    fn surfaces(&self) -> Vec<String> {
        PANES
            .iter()
            .map(|p| p.id.key().to_string())
            .chain([
                "wallpaper-chooser".into(),
                "privacy-chooser".into(),
                "search-results".into(),
            ])
            .collect()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        match surface {
            "accessibility" => vec![
                State::new(),
                state("login", "saving"),
                state("login", "refused"),
                state("login", "none"),
            ],
            "mouse" => vec![State::new(), state("mice", "two")],
            "privacy" => vec![
                State::new(),
                state("clipboard", "empty"),
                state("clipboard", "refused"),
            ],
            _ => vec![State::new()],
        }
    }

    fn capture(&mut self, r: &Request<'_>) -> Result<Pixmap, String> {
        let mut live = self.live.clone();
        let shown = self.settings.clipboard.clone();
        let mut refused = false;
        let mut scroll = 0.0;
        for (flag, value) in r.state {
            match (flag.as_str(), r.surface, value.as_str()) {
                ("scroll", "search-results", _) => {
                    return Err("settings/search-results does not scroll".into())
                }
                ("scroll", ..) => {
                    scroll = value
                        .parse()
                        .ok()
                        .filter(|s: &f64| s.is_finite() && *s >= 0.0)
                        .ok_or_else(|| {
                            format!("--scroll {value}: points from the top, 0 or more")
                        })?
                }
                ("login", "accessibility", "none") => live.login_screen = None,
                ("login", "accessibility", "saving") => {
                    live.login_screen = Some(LoginScreen {
                        current: None,
                        saving: true,
                    })
                }
                ("login", "accessibility", "refused") => refused = true,
                ("mice", "mouse", "one") => {}
                ("mice", "mouse", "two") => {
                    live.mice = mice(&[
                        (
                            "046d:c077:USB Optical Mouse",
                            "USB Optical Mouse",
                            1600,
                            true,
                        ),
                        ("1532:0043:Gaming Mouse", "Gaming Mouse", 2400, true),
                    ])
                }
                ("clipboard", "privacy", "empty") => {
                    self.settings.clipboard = clipboard::List::default()
                }
                ("clipboard", "privacy", "refused") => {
                    self.settings.clipboard = clipboard::List {
                        unreadable: Some("a group or other users can write the file".into()),
                        ..clipboard::List::default()
                    }
                }
                ("clipboard", "privacy", _) => {
                    return Err(format!("--clipboard {value}: empty or refused"))
                }
                ("clipboard", ..) => {
                    return Err("--clipboard is a state of settings/privacy".into())
                }
                ("login", "accessibility", _) => {
                    return Err(format!("--login {value}: none, saving or refused"))
                }
                ("mice", "mouse", _) => return Err(format!("--mice {value}: one or two")),
                ("login", ..) => return Err("--login is a state of settings/accessibility".into()),
                _ => return Err("--mice is a state of settings/mouse".into()),
            }
        }
        let pal = palette(&self.theme, r.look)?;
        let session = Session {
            dark: r.look.dark,
            display: DisplayOptions {
                reduce_motion: false,
                reduce_transparency: r.look.reduce_transparency,
                increase_contrast: r.look.increase_contrast,
                differentiate_without_color: false,
            },
        };
        let size = r
            .size
            .map(|(w, h)| Size { w, h })
            .unwrap_or_else(|| window::default_size(WIDTH_FACTOR));
        match r.surface {
            "search-results" => {
                if r.size.is_some() {
                    return Err("settings/search-results has the size of its results".into());
                }
                let mut search = SearchField::default();
                search.edit(|f| f.insert("scroll"));
                let count = search.results().len();
                let (w, h) = popup::size(WIDTH_FACTOR, count, RowSize::default());
                let rows = popup::rows(count, w, RowSize::default());
                let mut pm = pixmap(w, h, r.scale)?;
                fill(&mut pm, pal.window_background);
                self.painter
                    .paint_popup(&mut pm, r.scale, &search, &rows, Some(0), &pal);
                Ok(pm)
            }
            "wallpaper-chooser" => {
                let (mut pm, layout) = self.window(
                    PaneId::Wallpaper,
                    &live,
                    false,
                    scroll,
                    size,
                    r.scale,
                    session,
                    &pal,
                )?;
                let picks = self.scratch.path().join("Pictures");
                for dir in ["Summer", "Screenshots"] {
                    std::fs::create_dir_all(picks.join(dir)).map_err(|e| e.to_string())?;
                }
                for f in ["beach.jpg", "city.png", "mountain.jpg", "notes.txt"] {
                    std::fs::write(picks.join(f), b"x").map_err(|e| e.to_string())?;
                }
                let mut sheet = Sheet::picker(&picks, cupertino_filepicker::Mode::Photo);
                for _ in 0..3 {
                    sheet.key(SheetKey::Down);
                }
                let sheet_layout = sheet.layout(&layout.content);
                self.painter.paint_sheet(
                    &mut pm,
                    r.scale,
                    &sheet,
                    &sheet_layout,
                    &layout.content,
                    &pal,
                );
                Ok(pm)
            }
            "privacy-chooser" => {
                let (mut pm, layout) = self.window(
                    PaneId::Privacy,
                    &live,
                    false,
                    scroll,
                    size,
                    r.scale,
                    session,
                    &pal,
                )?;
                let apps = clipboard::apps(
                    &live.apps,
                    &self.settings.clipboard,
                    sidebar::standard_compare,
                );
                let mut sheet = Sheet::apps(apps);
                sheet.key(SheetKey::Down);
                let sheet_layout = sheet.layout(&layout.content);
                self.painter.paint_sheet(
                    &mut pm,
                    r.scale,
                    &sheet,
                    &sheet_layout,
                    &layout.content,
                    &pal,
                );
                Ok(pm)
            }
            key => {
                let pane = PaneId::from_key(key).ok_or_else(|| format!("{key}: no pane"))?;
                let window =
                    self.window(pane, &live, refused, scroll, size, r.scale, session, &pal);
                self.settings.clipboard = shown;
                Ok(window?.0)
            }
        }
    }
}

fn main() -> ExitCode {
    cupertino_capture::run(Capture::new)
}
