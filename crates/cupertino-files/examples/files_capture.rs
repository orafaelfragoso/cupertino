//! The Files surfaces for look checks: `files/window`, a browser window (920 × 436 at 1×) on a sample home folder,
//! and `files/sidebar`, the sidebar column of that window. The window is the Kit window ([`cupertino_files::kit`])
//! drawn on the headless window system with the CPU renderer.
//!
//! `ci/capture.sh files/window --view list --scale 2` (the flags are `cupertino-capture`'s)
//!
//! State flags: `--view icon|list` (default icon), `--selected NAME` (the item, selected) and `--devices removable`
//! (three removable file systems under Locations: "Backup Disk" mounted, "Camera Card" busy, "KINGSTON" not
//! mounted). The home folder `avery` is made under a temporary directory with the user folders and sample files; the
//! places are those of a session with that home on a computer named `cupertino`. File types come from the system's
//! MIME database, the items' icons from the asset packs and the icon theme, and the sample pictures (drawn here) show
//! their thumbnails, made in a thumbnail cache under the temporary directory before the window is drawn. The list's
//! dates read relative to a fixed time ([`sample_now`]), which the sample files are dated back from, so every capture
//! draws the same dates whenever it runs.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cupertino_capture::{App, Look, Request, State};
use cupertino_files::kit::{prepare, Files, Start};
use cupertino_files::notice::Notices;
use cupertino_files::places::{folder_icons, standard_places, Medium, Place};
use cupertino_files::thumbnail::Thumbnailer;
use cupertino_files::trash::Trash;
use cupertino_files::window::ViewMode;
use cupertino_kit::core::Role;
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, SystemAppearance, Timestamp,
    WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{PaintOp, PixelFormat};
use cupertino_kit::theme::ThemeTokens;
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};
use tiny_skia::{IntRect, IntSize, Pixmap};

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

/// The sample files: name, size in bytes and age in days. A `.png` is a picture drawn by [`sample_picture`], so it
/// has a thumbnail, and its size is the picture's.
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

/// The sample pictures: how many there are, and their size in pixels. They are wider than tall, so a thumbnail is the
/// one image of the window that is not square.
const PICTURES: usize = 2;
const PICTURE_SIZE: (u32, u32) = (256, 176);
/// How long the thumbnails of the sample pictures may take.
const THUMBNAILS: Duration = Duration::from_secs(20);

/// The time the capture is drawn at, in seconds since the epoch: noon (UTC) of an autumn weekday.
const SAMPLE_TIME: u64 = 1_792_670_400;

/// The time the window words its dates against and the sample files are dated back from.
fn sample_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(SAMPLE_TIME)
}

/// The window's size at 1×, in points.
const WINDOW: (f64, f64) = (920.0, 436.0);
/// The sidebar column's width, in points.
const SIDEBAR: f64 = cupertino_files::metrics::sidebar::DEFAULT_WIDTH;
/// How long the listing of the sample folder may take.
const LISTING: Duration = Duration::from_secs(5);
/// The work the runtime may do between two looks at the window.
const TURNS: u32 = 10_000;
/// The frames a window may ask for before it rests.
const FRAMES: usize = 8;

fn io(e: std::io::Error) -> String {
    e.to_string()
}

/// The sample home folder under `root`: the user folders (an hour old) and the sample files (sparse, with their ages).
fn home(root: &Path) -> Result<PathBuf, String> {
    let home = root.join(HOME);
    let now = sample_now();
    for (_, name) in USER_DIRS {
        let dir = home.join(name);
        std::fs::create_dir_all(&dir).map_err(io)?;
        std::fs::File::open(&dir)
            .and_then(|d| d.set_modified(now - Duration::from_secs(3_600)))
            .map_err(io)?;
    }
    for (n, (name, size, days)) in FILES.into_iter().enumerate() {
        let path = home.join(name);
        if name.ends_with(".png") {
            sample_picture(n)
                .save_png(&path)
                .map_err(|e| e.to_string())?;
        } else if let Some(text) = sample_text(name, size) {
            std::fs::write(&path, text).map_err(io)?;
        } else {
            std::fs::File::create(&path)
                .and_then(|f| f.set_len(size))
                .map_err(io)?;
        }
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(now - Duration::from_secs(days * 86_400 + 3_600)))
            .map_err(io)?;
    }
    // a folder of documents, for the column view's chain
    let documents = home.join("Documents");
    for (name, size) in DOCUMENTS {
        let path = documents.join(name);
        match sample_text(name, size) {
            Some(text) => std::fs::write(&path, text).map_err(io)?,
            None => std::fs::File::create(&path)
                .and_then(|f| f.set_len(size))
                .map_err(io)?,
        }
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(now - Duration::from_secs(2 * 86_400)))
            .map_err(io)?;
    }
    std::fs::create_dir_all(documents.join("Projects")).map_err(io)?;
    Ok(home)
}

/// The files of the sample Documents folder: name and size in bytes.
const DOCUMENTS: [(&str, u64); 4] = [
    ("Meeting notes.txt", 1_400),
    ("Budget 2027.xlsx", 36_000),
    ("Reading list.md", 900),
    ("Contract.pdf", 220_000),
];

/// The lines a sample text file repeats.
const SAMPLE_LINES: [&str; 6] = [
    "Plan for the week",
    "",
    "- Review the layout of the new folder views",
    "- Write down what the preview shows for each kind of file",
    "- Check the colours in both appearances",
    "- Keep the sizes on the point grid",
];

/// The contents of sample text file `name`, `size` bytes of plain lines; `None` for a file that is not text.
fn sample_text(name: &str, size: u64) -> Option<Vec<u8>> {
    if ![".txt", ".md", ".rs"].iter().any(|ext| name.ends_with(ext)) {
        return None;
    }
    let mut text = Vec::new();
    let mut n = 0;
    while (text.len() as u64) < size {
        text.extend_from_slice(SAMPLE_LINES[n % SAMPLE_LINES.len()].as_bytes());
        text.push(b'\n');
        n += 1;
    }
    text.truncate(size as usize);
    Some(text)
}

/// Sample picture `n`: a sky over hills, in colours of its own.
fn sample_picture(n: usize) -> Pixmap {
    let (w, h) = PICTURE_SIZE;
    let mut pm = Pixmap::new(w, h).expect("a picture");
    let hue = n as f32 * 0.37;
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let hill = 0.62 + 0.12 * (fx * 6.0 + hue * 5.0).sin();
            let [r, g, b] = if fy > hill {
                [0.25 + 0.2 * hue, 0.55 - 0.2 * fy, 0.25]
            } else {
                [0.45 + 0.4 * fy, 0.6 + 0.3 * fy - 0.2 * hue, 0.95]
            };
            let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let i = ((y * w + x) * 4) as usize;
            pm.data_mut()[i..i + 4].copy_from_slice(&[c(r), c(g), c(b), 255]);
        }
    }
    pm
}

struct Capture {
    home: PathBuf,
    places: Vec<Place>,
    root: tempfile::TempDir,
}

impl Capture {
    fn new() -> Result<Self, String> {
        let root = tempfile::tempdir().map_err(io)?;
        let home = home(root.path())?;
        let user_dirs: Vec<(String, PathBuf)> = USER_DIRS
            .iter()
            .map(|(key, name)| (key.to_string(), home.join(name)))
            .collect();
        // an applications folder and a trash that holds something, so the sidebar lists both
        let applications = root.path().join("share/applications");
        let trash = root.path().join("Trash/files");
        for dir in [&applications, &trash] {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let places = standard_places(&home, &user_dirs, &trash, Some(&applications), "cupertino");
        Ok(Capture { home, places, root })
    }

    /// The window on the sample home folder in `view` with `places`, `selected` selected, drawn in `look` at `scale`;
    /// after `then` is done, the window it opened last (the preview window, a popover), or the browser window.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        view: ViewMode,
        places: Vec<Place>,
        selected: Option<&str>,
        then: &[Step],
        look: Look,
        scale: f32,
        size: Size,
    ) -> Result<Pixmap, String> {
        let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
        let dirs = cupertino_apps::XdgDirs::from_env().map_err(|e| e.to_string())?;
        let types =
            cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
        let user_dirs: Vec<(String, PathBuf)> = USER_DIRS
            .iter()
            .map(|(key, name)| (key.to_string(), self.home.join(name)))
            .collect();
        let folders = folder_icons(&self.home, &user_dirs, &places)
            .into_iter()
            .collect();
        let icons = FileIcons::discover(types.clone(), folders);
        let cache = self.root.path().join("cache");
        let start = Start::open(
            self.home.clone(),
            places,
            Some(Arc::new(types)),
            self.root.path().join("recently-used.xbel"),
            Notices::start(None),
            Box::new(|| Err(std::io::Error::other("no applications in a capture"))),
            None,
            None,
        )
        .map_err(|e| format!("listing {}: {e}", self.home.display()))?
        .with_trash(Trash::with_home(self.root.path().join("Trash")))
        .with_home(self.home.clone())
        .with_view(view)
        .with_file_icons(icons)
        .with_thumbnails(Thumbnailer::new(&cache, "cupertino-files"))
        .with_clock(sample_now);
        prepare(start);
        let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
        let preferences = cupertino_kit::app::preferences(&SystemAppearance {
            dark: look.dark,
            increase_contrast: look.increase_contrast,
            reduce_transparency: look.reduce_transparency,
            reduce_motion: false,
        });
        let theme = Theme::load().map_err(|e| e.to_string())?;
        options.theme =
            Arc::new(ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?);
        let turns = NonZeroU32::new(TURNS).expect("a turn budget");
        let mut driver = TestDriver::<Files>::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let window = driver.runtime().windows()[0].1;
        let system = driver.window_system_mut();
        system
            .enqueue_window(window, WindowEvent::Scale(f64::from(scale)))
            .map_err(|e| fail(&e))?;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        system
            .configure(window, Some(size), active)
            .map_err(|e| fail(&e))?;
        // the folder is listed on a thread of its own
        let names = |driver: &TestDriver<Files>| {
            driver
                .runtime()
                .snapshot(window)
                .map(|s| {
                    s.accessibility
                        .iter()
                        .filter(|(_, n)| n.role == Role::StaticText)
                        .filter_map(|(_, n)| n.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let end = Instant::now() + LISTING;
        while !names(&driver).iter().any(|n| n == "notes.txt") {
            if Instant::now() > end {
                return Err(format!(
                    "listing {} took over {LISTING:?}",
                    self.home.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
            settle(&mut driver, window, turns).map_err(|e| fail(&e))?;
        }
        // the pictures' thumbnails are made on worker threads: wait until the window draws each
        let end = Instant::now() + THUMBNAILS;
        while thumbnails_drawn(&driver, window) < PICTURES {
            if Instant::now() > end {
                return Err(format!("the thumbnails took over {THUMBNAILS:?}"));
            }
            std::thread::sleep(Duration::from_millis(5));
            settle(&mut driver, window, turns).map_err(|e| fail(&e))?;
        }
        // a path of names is clicked one after the other, each once its column shows it
        for name in selected.into_iter().flat_map(|path| path.split('/')) {
            let end = Instant::now() + LISTING;
            let at = loop {
                if let Some(at) = text_center(&driver, window, name) {
                    break at;
                }
                if Instant::now() > end {
                    return Err(format!("{name}: no item of the sample folder"));
                }
                std::thread::sleep(Duration::from_millis(5));
                settle(&mut driver, window, turns).map_err(|e| fail(&e))?;
            };
            click(&mut driver, window, at, turns).map_err(|e| fail(&e))?;
            settle(&mut driver, window, turns).map_err(|e| fail(&e))?;
        }
        for step in then {
            match *step {
                Step::Key(keysym, mods) => {
                    key(&mut driver, window, keysym, mods, turns).map_err(|e| fail(&e))?
                }
                Step::Click(name) => {
                    let at = text_center(&driver, window, name)
                        .or_else(|| named_center(&driver, window, name))
                        .ok_or_else(|| format!("{name}: nothing of that name"))?;
                    click(&mut driver, window, at, turns).map_err(|e| fail(&e))?;
                }
            }
            settle(&mut driver, window, turns).map_err(|e| fail(&e))?;
        }
        let shown = driver.runtime().windows();
        let window = match shown.last() {
            Some(&(_, newest)) if newest != window => {
                let system = driver.window_system_mut();
                system
                    .enqueue_window(newest, WindowEvent::Scale(f64::from(scale)))
                    .map_err(|e| fail(&e))?;
                system
                    .configure(newest, None, active)
                    .map_err(|e| fail(&e))?;
                // its motion runs on the frames' clock: present frames a sixtieth of a second apart until it rests
                let mut at = 0;
                for _ in 0..MOTION_FRAMES {
                    driver.run_until_idle(turns).map_err(|e| fail(&e))?;
                    let system = driver.window_system_mut();
                    if !system.frame_requested(newest).map_err(|e| fail(&e))? {
                        break;
                    }
                    at += 16_666_667;
                    system
                        .frame_ready(newest, Timestamp(at))
                        .map_err(|e| fail(&e))?;
                }
                newest
            }
            _ => window,
        };
        if !settle(&mut driver, window, turns).map_err(|e| fail(&e))? {
            return Err("the window keeps asking for frames".into());
        }
        let frame = driver
            .window_system_mut()
            .frames(window)
            .map_err(|e| fail(&e))?
            .last()
            .ok_or("the window presented no frame")?;
        let mut pixels = frame.pixels.clone();
        if frame.format == PixelFormat::Rgba8 {
            for px in pixels.as_chunks_mut::<4>().0 {
                let a = u16::from(px[3]);
                for c in &mut px[..3] {
                    *c = ((u16::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        let size = IntSize::from_wh(frame.width, frame.height).ok_or("the frame is empty")?;
        Pixmap::from_vec(pixels, size)
            .ok_or_else(|| "the frame's pixels do not fill it".to_string())
    }
}

/// Run the window and present the frames it asks for until it rests, or draws the same pixels frame after frame;
/// `false` when it does neither.
fn settle(
    driver: &mut TestDriver<Files>,
    window: WindowId,
    turns: NonZeroU32,
) -> Result<bool, Box<dyn std::fmt::Debug>> {
    for _ in 0..FRAMES {
        driver.run_until_idle(turns).map_err(|e| Box::new(e) as _)?;
        let system = driver.window_system_mut();
        if !system
            .frame_requested(window)
            .map_err(|e| Box::new(e) as _)?
        {
            return Ok(true);
        }
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| Box::new(e) as _)?;
    }
    // a window that goes on asking for frames but presents the same pixels each time (a busy row's activity indicator
    // at the capture's fixed clock) shows its last state
    let frames = driver
        .window_system_mut()
        .frames(window)
        .map_err(|e| Box::new(e) as _)?;
    Ok(matches!(frames, [.., a, b] if a.pixels == b.pixels))
}

/// How many thumbnails the window draws: the images that are not square.
fn thumbnails_drawn(driver: &TestDriver<Files>, window: WindowId) -> usize {
    let Some(snapshot) = driver.runtime().snapshot(window) else {
        return 0;
    };
    snapshot
        .display_list
        .nodes
        .iter()
        .flat_map(|n| &n.ops)
        .filter(|op| match op {
            PaintOp::Image { dst, .. } => (dst.size.width - dst.size.height).abs() > 0.5,
            _ => false,
        })
        .count()
}

/// What a capture does once the items are selected.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// A key pressed and let go, with the modifiers held.
    Key(u32, Mods),
    /// A click on the text or the control of this name.
    Click(&'static str),
}

/// The frames a window opened by a step may take to finish its motion.
const MOTION_FRAMES: usize = 120;

/// The middle of the node named `name` in the window.
fn named_center(driver: &TestDriver<Files>, window: WindowId, name: &str) -> Option<Point> {
    let snapshot = driver.runtime().snapshot(window)?;
    snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.name.as_deref() == Some(name))
        .map(|(_, n)| n.bounds.center())
}

/// The key of `keysym` pressed and let go with `mods` held.
fn key(
    driver: &mut TestDriver<Files>,
    window: WindowId,
    keysym: u32,
    mods: Mods,
    turns: NonZeroU32,
) -> Result<(), Box<dyn std::fmt::Debug>> {
    for pressed in [true, false] {
        let event = WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: char::from_u32(keysym)
                    .filter(|c| pressed && !c.is_control() && mods.is_empty()),
                pressed,
                repeat: false,
                mods,
            },
            Timestamp(0),
        );
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .map_err(|e| Box::new(e) as _)?;
        driver.run_until_idle(turns).map_err(|e| Box::new(e) as _)?;
    }
    Ok(())
}

/// The middle of the text `name` in the window: the one furthest along, so an item is found before the sidebar's place
/// of the same name, and the newest column's before the others.
fn text_center(driver: &TestDriver<Files>, window: WindowId, name: &str) -> Option<Point> {
    let snapshot = driver.runtime().snapshot(window)?;
    snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::StaticText && n.name.as_deref() == Some(name))
        .map(|(_, n)| n.bounds.center())
        .max_by(|a, b| a.x.total_cmp(&b.x))
}

/// A click at `at`.
fn click(
    driver: &mut TestDriver<Files>,
    window: WindowId,
    at: Point,
    turns: NonZeroU32,
) -> Result<(), Box<dyn std::fmt::Debug>> {
    let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
    let mut events = vec![pointer(PointerEvent::Motion { position: at })];
    for pressed in [true, false] {
        events.push(pointer(PointerEvent::Button {
            position: at,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        }));
    }
    for event in events {
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .map_err(|e| Box::new(e) as _)?;
        driver.run_until_idle(turns).map_err(|e| Box::new(e) as _)?;
    }
    Ok(())
}

fn state(flag: &str, value: &str) -> State {
    vec![(flag.to_string(), value.to_string())]
}

impl App for Capture {
    const NAME: &'static str = "files";
    const STATE_FLAGS: &'static [&'static str] = &["view", "selected", "devices"];

    fn surfaces(&self) -> Vec<String> {
        vec![
            "window".into(),
            "sidebar".into(),
            "preview".into(),
            "popover".into(),
        ]
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        match surface {
            "window" => vec![
                State::new(),
                state("view", "list"),
                state("selected", "notes.txt"),
            ],
            _ => vec![State::new(), state("devices", "removable")],
        }
    }

    fn capture(&mut self, r: &Request<'_>) -> Result<Pixmap, String> {
        // the preview window opens with Space on the selection; the tags popover from the toolbar's Tags button
        let then: &[Step] = match r.surface {
            "window" | "sidebar" => &[],
            "preview" => &[Step::Key(0x20, Mods::NONE)],
            "popover" => &[Step::Click("Tags")],
            _ => return Err(format!("{}: no such surface", r.surface)),
        };
        let mut view = ViewMode::Icons;
        let mut selected = None;
        let mut places = self.places.clone();
        for (flag, value) in r.state {
            match (flag.as_str(), value.as_str()) {
                ("view", "icon") => view = ViewMode::Icons,
                ("view", "list") => view = ViewMode::List,
                ("view", "columns") => view = ViewMode::Columns,
                ("view", _) => return Err(format!("--view {value}: icon, list or columns")),
                ("devices", "removable") => places.extend([
                    Place::removable(
                        "Backup Disk",
                        Some(Path::new("/run/media/avery/Backup")),
                        false,
                        Medium::Disk,
                    ),
                    Place::removable(
                        "Camera Card",
                        Some(Path::new("/run/media/avery/CAM")),
                        true,
                        Medium::Card,
                    ),
                    Place::removable("KINGSTON", None, false, Medium::Stick),
                ]),
                ("devices", _) => return Err(format!("--devices {value}: removable")),
                (_, name) => selected = Some(name.to_string()),
            }
        }
        let (w, h) = r.size.unwrap_or(WINDOW);
        let pm = self.draw(
            view,
            places,
            selected.as_deref(),
            then,
            r.look,
            r.scale,
            Size::new(w, h),
        )?;
        if r.surface != "sidebar" {
            return Ok(pm);
        }
        let scale = f64::from(r.scale);
        let rect = IntRect::from_xywh(0, 0, (SIDEBAR * scale).round() as u32, pm.height())
            .ok_or("the sidebar is empty")?;
        pm.clone_rect(rect)
            .ok_or_else(|| "the sidebar is outside the window".to_string())
    }
}

fn main() -> ExitCode {
    cupertino_capture::run(Capture::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(surface: &'a str, state: &'a State) -> Request<'a> {
        Request {
            surface,
            state,
            look: Look {
                dark: false,
                increase_contrast: false,
                reduce_transparency: false,
            },
            scale: 1.0,
            size: None,
        }
    }

    #[test]
    fn the_list_window_is_the_same_pixels_on_every_run() {
        let state = vec![("view".to_string(), "list".to_string())];
        let drawn = Capture::new()
            .unwrap()
            .capture(&request("window", &state))
            .unwrap();
        assert_eq!((drawn.width(), drawn.height()), (920, 436));
        let again = Capture::new()
            .unwrap()
            .capture(&request("window", &state))
            .unwrap();
        assert!(
            drawn.data() == again.data(),
            "the second run drew other pixels"
        );
    }

    #[test]
    fn the_sidebar_is_the_window_cut_to_its_column() {
        let state = State::new();
        let sidebar = Capture::new()
            .unwrap()
            .capture(&request("sidebar", &state))
            .unwrap();
        assert_eq!((sidebar.width(), sidebar.height()), (223, 436));
    }

    #[test]
    fn an_unknown_item_is_refused_by_name() {
        let state = vec![("selected".to_string(), "nothing.txt".to_string())];
        let refused = Capture::new()
            .unwrap()
            .capture(&request("window", &state))
            .unwrap_err();
        assert!(refused.contains("nothing.txt"), "{refused}");
    }
}
