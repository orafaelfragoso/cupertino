//! The Files surfaces for look checks: `files/window`, a browser window (920 × 436 at 1×) on a sample home folder,
//! and `files/sidebar`, the sidebar column of that window; `files/preview`, the preview window Space opens on the
//! selection; `files/popover`, the tags popover of the selection; `files/tabs`, the window with a second tab gone
//! into Documents; and `files/desktop`, the desktop's items (640 × 400) over a flat fill. The windows are the Kit
//! windows ([`cupertino_files::kit`], [`cupertino_files::kit::desktop`]) drawn on the headless window system with the
//! CPU renderer.
//!
//! `cargo run -p cupertino-files --example files_capture -- files/window --view list --scale 2` (the flags are `cupertino_kit::test::capture`'s)
//!
//! State flags: `--view icon|list|columns|gallery` (default icon), `--selected PATH` (the item, selected: a path under
//! the home folder, its folders shown on the way in the column view), `--state rename|ring|band` (the view's item, or
//! the one selected, with its name being edited from its context menu's Rename; with the ring of its context menu
//! open, a secondary press on it; or a band held from the view's lower trailing corner over it) and
//! `--devices removable` (three removable file systems under Locations: "Backup Disk" mounted, "Camera Card" busy,
//! "KINGSTON" not mounted). The view's item is `notes.txt`, in the gallery `IMG_0042.png`, whose picture the strip
//! shows in view. The home folder `avery` is made in [`ROOT`] under the temporary directory with the user folders and
//! sample files; the places are those of a session with that home on a computer named `cupertino`. File types come
//! from the system's MIME database, the items' icons from the asset packs and the icon theme, and the sample pictures
//! (drawn here) show their thumbnails, made in a thumbnail cache under [`ROOT`] before the window is drawn. The list's
//! dates read relative to a fixed time ([`sample_now`]), which the sample files are dated back from, so every capture
//! draws the same dates whenever it runs. A file's creation date, which a preview shows, is the time it was first
//! written and cannot be set: [`ROOT`] is kept between runs, its files written again in place, so the dates stay too.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cupertino_files::kit::desktop::{self, Desktop, Screen};
use cupertino_files::kit::{prepare, Files, Start};
use cupertino_files::notice::Notices;
use cupertino_files::places::{folder_icons, standard_places, Medium, Place};
use cupertino_files::trash::Trash;
use cupertino_files::window::ViewMode;
use cupertino_icons::thumbnail::Thumbnailer;
use cupertino_kit::core::{Application, Role, RuntimeOptions};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::platform::{
    KeyEvent, Keysym, Mods, MouseButton, PointerEvent, SystemAppearance, Timestamp, WindowId,
};
use cupertino_kit::render::{PaintOp, PixelFormat};
use cupertino_kit::test::capture::{App, Look, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowEvent, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use tiny_skia::{IntRect, IntSize, Pixmap};

/// The folder under the temporary directory that holds the sample home folder, the trash and the thumbnail cache,
/// kept between runs.
const ROOT: &str = "cupertino-files-capture";

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
/// The desktop's output, in points: the size of its one window.
const SCREEN: (f64, f64) = (640.0, 400.0);
/// What the desktop's items are drawn over where the wallpaper would be: a mid grey.
const DESKTOP_FILL: [u8; 3] = [96, 112, 128];
/// The item a view's states act on, and the gallery's, whose picture its strip shows in view.
const ITEM: &str = "notes.txt";
const GALLERY_ITEM: &str = "IMG_0042.png";
/// How far in from the window's lower trailing corner a band's press is, in points: past the last row of the icon
/// grid and the list.
const BAND_CORNER: (f64, f64) = (30.0, 16.0);
/// How far into the column view's first column a band's press is, in points.
const BAND_COLUMN: f64 = 120.0;
/// The sidebar column's width, in points.
const SIDEBAR: f64 = cupertino_files::metrics::sidebar::DEFAULT_WIDTH;
/// How long the listing of the sample folder may take.
const LISTING: Duration = Duration::from_secs(5);
/// The work the runtime may do between two looks at the window.
const TURNS: u32 = 10_000;
/// A frame of a motion, a sixtieth of a second, in nanoseconds.
const FRAME_NS: u64 = 16_666_667;
/// The frames a window may ask for without resting or showing a steady state before the capture gives up: a guard
/// against a hang only.
const HANG_FRAMES: usize = 10_000;

fn io(e: std::io::Error) -> String {
    e.to_string()
}

/// The sample home folder under `root`: the user folders (an hour old) and the sample files (sparse, with their ages).
/// Every item is dated back from [`sample_now`], a folder once what it holds is written, since writing into a folder
/// dates it by the wall clock.
fn home(root: &Path) -> Result<PathBuf, String> {
    let home = root.join(HOME);
    let now = sample_now();
    for (_, name) in USER_DIRS {
        std::fs::create_dir_all(home.join(name)).map_err(io)?;
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
    let projects = documents.join("Projects");
    std::fs::create_dir_all(&projects).map_err(io)?;
    // the desktop's items: copies of sample files, and a folder
    let desktop = home.join("Desktop");
    for name in DESKTOP {
        std::fs::copy(home.join(name), desktop.join(name)).map_err(io)?;
    }
    let drafts = desktop.join(DESKTOP_FOLDER);
    std::fs::create_dir_all(&drafts).map_err(io)?;
    let folders = USER_DIRS.iter().map(|(_, name)| home.join(name));
    for dir in folders.chain([projects, drafts]) {
        std::fs::File::open(&dir)
            .and_then(|d| d.set_modified(now - Duration::from_secs(3_600)))
            .map_err(io)?;
    }
    Ok(home)
}

/// The sample files copied onto the desktop, and the folder made there.
const DESKTOP: [&str; 3] = ["IMG_0042.png", "notes.txt", "Résumé 2026.pdf"];
const DESKTOP_FOLDER: &str = "Drafts";

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

/// The contents of sample text file `name`, `size` bytes of plain lines, the lines over again under a numbered heading
/// so no stretch of the text reads as the start repeated; `None` for a file that is not text.
fn sample_text(name: &str, size: u64) -> Option<Vec<u8>> {
    if ![".txt", ".md", ".rs"].iter().any(|ext| name.ends_with(ext)) {
        return None;
    }
    let mut text = Vec::new();
    let mut n = 0;
    while (text.len() as u64) < size {
        let round = n / SAMPLE_LINES.len();
        match n % SAMPLE_LINES.len() {
            0 if round > 0 => text.extend_from_slice(format!("Week {}", round + 1).as_bytes()),
            line => text.extend_from_slice(SAMPLE_LINES[line].as_bytes()),
        }
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
    root: PathBuf,
}

impl Capture {
    fn new() -> Result<Self, String> {
        let root = std::env::temp_dir().join(ROOT);
        let home = home(&root)?;
        let user_dirs: Vec<(String, PathBuf)> = USER_DIRS
            .iter()
            .map(|(key, name)| (key.to_string(), home.join(name)))
            .collect();
        // an applications folder and a trash that holds something, so the sidebar lists both
        let applications = root.join("share/applications");
        let trash = root.join("Trash/files");
        for dir in [&applications, &trash] {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let places = standard_places(&home, &user_dirs, &trash, Some(&applications), "cupertino");
        Ok(Capture { home, places, root })
    }

    /// The session's file types and the items' icons with the user directories' folder icons of `places`.
    fn icons(&self, places: &[Place]) -> Result<(cupertino_apps::MimeDatabase, FileIcons), String> {
        let dirs = cupertino_apps::XdgDirs::from_env().map_err(|e| e.to_string())?;
        let types =
            cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
        let user_dirs: Vec<(String, PathBuf)> = USER_DIRS
            .iter()
            .map(|(key, name)| (key.to_string(), self.home.join(name)))
            .collect();
        let folders = folder_icons(&self.home, &user_dirs, places)
            .into_iter()
            .collect();
        let icons = FileIcons::discover(types.clone(), folders);
        Ok((types, icons))
    }

    /// The desktop's items on the sample home folder's Desktop, drawn in `look` at `scale` over [`DESKTOP_FILL`].
    fn desktop(&self, look: Look, scale: f32) -> Result<Pixmap, String> {
        let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
        let (types, icons) = self.icons(&self.places)?;
        let screen = Screen {
            key: 1,
            width: SCREEN.0,
            height: SCREEN.1,
        };
        desktop::prepare(
            self.home.join("Desktop"),
            vec![screen],
            Arc::new(types),
            icons,
            Thumbnailer::new(self.root.join("cache"), "cupertino-files"),
        );
        let turns = NonZeroU32::new(TURNS).expect("a turn budget");
        let (mut driver, window) =
            started::<Desktop>(look, scale, Size::new(SCREEN.0, SCREEN.1), turns)?;
        wait_until(&mut driver, window, turns, LISTING, |driver| {
            DESKTOP
                .iter()
                .chain([&DESKTOP_FOLDER])
                .all(|name| named_center(driver, window, name).is_some())
        })
        .map_err(|e| e.unwrap_or_else(|| "the desktop's listing never ended".into()))?;
        wait_until(&mut driver, window, turns, THUMBNAILS, |driver| {
            thumbnails_drawn(driver, window) >= 1
        })
        .map_err(|e| e.unwrap_or_else(|| "the desktop's thumbnail was never drawn".into()))?;
        if !settle(&mut driver, window, turns).map_err(|e| fail(&e))? {
            return Err("the desktop keeps asking for frames".into());
        }
        let mut pm = presented(&mut driver, window)?;
        let [r, g, b] = DESKTOP_FILL;
        let mut fill = Pixmap::new(pm.width(), pm.height()).ok_or("the desktop is empty")?;
        fill.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        fill.draw_pixmap(
            0,
            0,
            pm.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
        std::mem::swap(&mut pm, &mut fill);
        Ok(pm)
    }

    /// The window on the sample home folder in `view` with `places`, `selected` selected, drawn in `look` at `scale`;
    /// after `then` is done, the window it opened last (the preview window, a popover) when `opened`, else the browser
    /// window.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        view: ViewMode,
        places: Vec<Place>,
        selected: Option<&str>,
        then: &[Step],
        opened: bool,
        look: Look,
        scale: f32,
        size: Size,
    ) -> Result<Pixmap, String> {
        let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
        let (types, icons) = self.icons(&places)?;
        let cache = self.root.join("cache");
        let start = Start::open(
            self.home.clone(),
            places,
            Some(Arc::new(types)),
            self.root.join("recently-used.xbel"),
            Notices::start(None),
            Box::new(|| Err(std::io::Error::other("no applications in a capture"))),
            None,
            None,
        )
        .map_err(|e| format!("listing {}: {e}", self.home.display()))?
        .with_trash(Trash::with_home(self.root.join("Trash")))
        .with_home(self.home.clone())
        .with_view(view)
        .with_file_icons(icons)
        .with_thumbnails(Thumbnailer::new(&cache, "cupertino-files"))
        .with_clock(sample_now);
        prepare(start);
        let turns = NonZeroU32::new(TURNS).expect("a turn budget");
        let (mut driver, window) = started::<Files>(look, scale, size, turns)?;
        // the folder is listed on a thread of its own
        wait_until(&mut driver, window, turns, LISTING, |driver| {
            named_center(driver, window, ITEM).is_some()
        })
        .map_err(|e| e.unwrap_or_else(|| format!("listing {} never ended", self.home.display())))?;
        // the pictures' thumbnails are made on worker threads: wait until the window draws each it shows, of which
        // the gallery's strip shows the first
        let in_view = match view {
            ViewMode::Gallery => 1,
            _ => PICTURES,
        };
        wait_until(&mut driver, window, turns, THUMBNAILS, |driver| {
            thumbnails_drawn(driver, window) >= in_view
        })
        .map_err(|e| e.unwrap_or_else(|| "the thumbnails were never drawn".into()))?;
        // a path of names is clicked one after the other, each once its column shows it
        // the gallery's strip names its items only for assistive technology
        let item_center = |driver: &TestDriver<Files>, name: &str| {
            text_center(driver, window, name).or_else(|| named_center(driver, window, name))
        };
        for name in selected.into_iter().flat_map(|path| path.split('/')) {
            wait_until(&mut driver, window, turns, LISTING, |driver| {
                item_center(driver, name).is_some()
            })
            .map_err(|e| e.unwrap_or_else(|| format!("{name}: no item of the sample folder")))?;
            let at = item_center(&driver, name).expect("shown");
            click(&mut driver, window, at, turns).map_err(|e| fail(&e))?;
            settle(&mut driver, window, turns).map_err(|e| fail(&e))?;
        }
        for step in then {
            match step {
                &Step::Key(keysym, mods) => {
                    key(&mut driver, window, keysym, mods, turns).map_err(|e| fail(&e))?
                }
                Step::Click(name) => {
                    let at = item_center(&driver, name)
                        .ok_or_else(|| format!("{name}: nothing of that name"))?;
                    click(&mut driver, window, at, turns).map_err(|e| fail(&e))?;
                }
                Step::Secondary(name) => {
                    let at = item_center(&driver, name)
                        .ok_or_else(|| format!("{name}: nothing of that name"))?;
                    let pointer = [
                        PointerEvent::Motion { position: at },
                        button(at, MouseButton::Right, true),
                        button(at, MouseButton::Right, false),
                    ];
                    send(&mut driver, window, pointer, turns).map_err(|e| fail(&e))?;
                }
                Step::Choose(name) => {
                    let menu = match driver.runtime().windows().last() {
                        Some(&(_, menu)) if menu != window => menu,
                        _ => return Err(format!("{name}: no menu is open")),
                    };
                    open_popup(&mut driver, menu, scale, turns)?;
                    let at = named_center(&driver, menu, name)
                        .ok_or_else(|| format!("{name}: no row of the menu"))?;
                    click(&mut driver, menu, at, turns).map_err(|e| fail(&e))?;
                    // the row flashes on the clock before its choice is sent and the menu closes
                    let mut at = 0;
                    while driver.runtime().windows().iter().any(|&(_, w)| w == menu) {
                        if at >= HANG_FRAMES as u64 * FRAME_NS {
                            return Err(format!("{name}: the menu never closes"));
                        }
                        at += FRAME_NS;
                        driver.advance_to(Timestamp(at)).map_err(|e| fail(&e))?;
                        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
                    }
                }
                &Step::Band { from, ref over } => {
                    let to = item_center(&driver, over)
                        .ok_or_else(|| format!("{over}: nothing of that name"))?;
                    let pointer = [
                        PointerEvent::Motion { position: from },
                        button(from, MouseButton::Left, true),
                        PointerEvent::Motion { position: to },
                    ];
                    send(&mut driver, window, pointer, turns).map_err(|e| fail(&e))?;
                }
            }
            settle(&mut driver, window, turns).map_err(|e| fail(&e))?;
        }
        // the start of a text file previewed is read on a thread of its own: wait until the preview shows it
        let previewed = selected
            .and_then(|path| path.rsplit('/').next())
            .filter(|name| sample_text(name, 1).is_some());
        if let Some(name) = previewed {
            let group = format!("Preview of {name}");
            wait_until(&mut driver, window, turns, LISTING, |driver| {
                !any_named(driver, |n| n == group)
                    || any_named(driver, |n| n.starts_with(SAMPLE_LINES[0]))
            })
            .map_err(|e| e.unwrap_or_else(|| format!("{name}: its text never showed")))?;
        }
        let shown = driver.runtime().windows();
        let window = match shown.last() {
            Some(&(_, newest)) if newest != window && opened => {
                open_popup(&mut driver, newest, scale, turns)?;
                newest
            }
            _ => window,
        };
        if !settle(&mut driver, window, turns).map_err(|e| fail(&e))? {
            return Err("the window keeps asking for frames".into());
        }
        presented(&mut driver, window)
    }
}

/// The options of a driver drawing in `look`: the Cupertino theme in that appearance.
fn options(look: Look) -> Result<RuntimeOptions, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    let preferences = cupertino_kit::test::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
        steady_caret: false,
    });
    let theme = cupertino_assets::theme::load().map_err(|e| e.to_string())?;
    options.theme =
        Arc::new(ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?);
    Ok(options)
}

/// A driver of application `A`, drawn in `look`, and its first window at `scale`, `size` points large and active.
fn started<A: Application>(
    look: Look,
    scale: f32,
    size: Size,
    turns: NonZeroU32,
) -> Result<(TestDriver<A>, WindowId), String> {
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<A>::new(
        Capabilities::default(),
        options(look)?,
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
    Ok((driver, window))
}

/// Show the window `popup` opened, at `scale`, the size it asked to be placed at as a compositor makes it, and run its
/// motion on the frames' clock, frames a sixtieth of a second apart, until it rests.
fn open_popup<A: Application>(
    driver: &mut TestDriver<A>,
    popup: WindowId,
    scale: f32,
    turns: NonZeroU32,
) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    let system = driver.window_system_mut();
    system
        .enqueue_window(popup, WindowEvent::Scale(f64::from(scale)))
        .map_err(|e| fail(&e))?;
    let placed = system
        .placement(popup)
        .map_err(|e| fail(&e))?
        .map(|placement| placement.size);
    system
        .configure(popup, placed, active)
        .map_err(|e| fail(&e))?;
    let mut at = 0;
    for _ in 0..HANG_FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if !system.frame_requested(popup).map_err(|e| fail(&e))? {
            return Ok(());
        }
        at += FRAME_NS;
        system
            .frame_ready(popup, Timestamp(at))
            .map_err(|e| fail(&e))?;
    }
    Err("the opened window's motion never rests".into())
}

/// The last frame `window` presented, its pixels premultiplied.
fn presented<A: Application>(
    driver: &mut TestDriver<A>,
    window: WindowId,
) -> Result<Pixmap, String> {
    let frame = driver
        .window_system_mut()
        .frames(window)
        .map_err(|e| format!("the headless window system: {e:?}"))?
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
    Pixmap::from_vec(pixels, size).ok_or_else(|| "the frame's pixels do not fill it".to_string())
}

/// Run the window and present the frames it asks for until it rests, or presents the same pixels twice in a row (a
/// busy row's activity indicator at the capture's fixed clock shows its last state); `false` when it does neither
/// within [`HANG_FRAMES`], which only guards against a hang.
fn settle<A: Application>(
    driver: &mut TestDriver<A>,
    window: WindowId,
    turns: NonZeroU32,
) -> Result<bool, Box<dyn std::fmt::Debug>> {
    let presented = |driver: &mut TestDriver<A>| -> Result<usize, Box<dyn std::fmt::Debug>> {
        Ok(driver
            .window_system_mut()
            .frames(window)
            .map_err(|e| Box::new(e) as _)?
            .len())
    };
    let before = presented(driver)?;
    // the frames presented when the window was last given a frame, if it was
    let mut given = None;
    for _ in 0..HANG_FRAMES {
        driver.run_until_idle(turns).map_err(|e| Box::new(e) as _)?;
        let now = presented(driver)?;
        let system = driver.window_system_mut();
        if !system
            .frame_requested(window)
            .map_err(|e| Box::new(e) as _)?
        {
            return Ok(true);
        }
        // it asks again: a steady state when the frame it was given drew nothing, or two frames of this settling
        // drew the same pixels
        let frames = system.frames(window).map_err(|e| Box::new(e) as _)?;
        if given == Some(now)
            || (now >= before + 2 && matches!(frames, [.., a, b] if a.pixels == b.pixels))
        {
            return Ok(true);
        }
        given = Some(now);
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| Box::new(e) as _)?;
    }
    Ok(false)
}

/// Settle the window until `done` holds of it, waiting for a task on another thread (a listing, a thumbnail, a file
/// read) to wake the runtime between two looks. `guard` only guards against a hang: the error is `None` when nothing
/// woke the runtime within it, and the reason when the window could not be run.
fn wait_until<A: Application>(
    driver: &mut TestDriver<A>,
    window: WindowId,
    turns: NonZeroU32,
    guard: Duration,
    done: impl Fn(&TestDriver<A>) -> bool,
) -> Result<(), Option<String>> {
    loop {
        settle(driver, window, turns)
            .map_err(|e| Some(format!("the headless window system: {e:?}")))?;
        if done(driver) {
            return Ok(());
        }
        if !driver.window_system_mut().wait_for_wake(guard) {
            return Err(None);
        }
    }
}

/// How many thumbnails the window draws: the images that are not square.
fn thumbnails_drawn<A: Application>(driver: &TestDriver<A>, window: WindowId) -> usize {
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
#[derive(Clone, Debug)]
enum Step {
    /// A key pressed and let go, with the modifiers held.
    Key(u32, Mods),
    /// A click on the text or the control of this name.
    Click(&'static str),
    /// A secondary press and its release on the item of this name.
    Secondary(String),
    /// A click on the row of this name of the menu open last.
    Choose(&'static str),
    /// A press at `from` in the window, held, and a move onto the item named `over`.
    Band { from: Point, over: String },
}

/// The middle of the node named `name` in the window.
fn named_center<A: Application>(
    driver: &TestDriver<A>,
    window: WindowId,
    name: &str,
) -> Option<Point> {
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

/// Whether a node of any window of the application has a name that `name` accepts.
fn any_named(driver: &TestDriver<Files>, name: impl Fn(&str) -> bool) -> bool {
    let runtime = driver.runtime();
    runtime.windows().into_iter().any(|(_, window)| {
        runtime.snapshot(window).is_some_and(|snapshot| {
            snapshot
                .accessibility
                .iter()
                .any(|(_, n)| n.name.as_deref().is_some_and(&name))
        })
    })
}

/// A button of the pointer at `at`, pressed or let go.
fn button(at: Point, button: MouseButton, pressed: bool) -> PointerEvent {
    PointerEvent::Button {
        position: at,
        button,
        pressed,
        serial: 1,
        mods: Mods::NONE,
    }
}

/// The pointer's `events` in the window, each run to idle.
fn send<A: Application>(
    driver: &mut TestDriver<A>,
    window: WindowId,
    events: impl IntoIterator<Item = PointerEvent>,
    turns: NonZeroU32,
) -> Result<(), Box<dyn std::fmt::Debug>> {
    for event in events {
        driver
            .window_system_mut()
            .enqueue_window(window, WindowEvent::Pointer(event, Timestamp(0)))
            .map_err(|e| Box::new(e) as _)?;
        driver.run_until_idle(turns).map_err(|e| Box::new(e) as _)?;
    }
    Ok(())
}

/// A click at `at`.
fn click<A: Application>(
    driver: &mut TestDriver<A>,
    window: WindowId,
    at: Point,
    turns: NonZeroU32,
) -> Result<(), Box<dyn std::fmt::Debug>> {
    let events = [
        PointerEvent::Motion { position: at },
        button(at, MouseButton::Left, true),
        button(at, MouseButton::Left, false),
    ];
    send(driver, window, events, turns)
}

fn state(flag: &str, value: &str) -> State {
    vec![(flag.to_string(), value.to_string())]
}

/// The views a sheet of `files/window` shows, each by its `--view` value, the icon view by none.
const VIEWS: [Option<&str>; 4] = [None, Some("list"), Some("columns"), Some("gallery")];

impl App for Capture {
    const NAME: &'static str = "files";
    const STATE_FLAGS: &'static [&'static str] = &["view", "selected", "state", "devices"];

    fn surfaces(&self) -> Vec<String> {
        vec![
            "window".into(),
            "sidebar".into(),
            "preview".into(),
            "popover".into(),
            "tabs".into(),
            "desktop".into(),
        ]
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        match surface {
            // each view at rest, its item selected, renamed, ringed and under a band
            "window" => VIEWS
                .into_iter()
                .flat_map(|view| {
                    let base: State = view.map(|v| state("view", v)).unwrap_or_default();
                    let item = if view == Some("gallery") {
                        GALLERY_ITEM
                    } else {
                        ITEM
                    };
                    let with = |flag: &str, value: &str| {
                        let mut state = base.clone();
                        state.push((flag.to_string(), value.to_string()));
                        state
                    };
                    [
                        base.clone(),
                        with("selected", item),
                        with("state", "rename"),
                        with("state", "ring"),
                        with("state", "band"),
                    ]
                })
                .collect(),
            "desktop" => vec![State::new()],
            _ => vec![State::new(), state("devices", "removable")],
        }
    }

    fn capture(&mut self, r: &Request<'_>) -> Result<Pixmap, String> {
        if r.surface == "desktop" {
            return match r.state.first() {
                None => self.desktop(r.look, r.scale),
                Some((flag, _)) => Err(format!("files/desktop has no state --{flag}")),
            };
        }
        // the preview window opens with Space on the selection; the tags popover from the toolbar's Tags button
        let mut then: Vec<Step> = match r.surface {
            "window" | "sidebar" => Vec::new(),
            "preview" => vec![Step::Key(0x20, Mods::NONE)],
            "popover" => vec![Step::Click("Tags")],
            // a second tab, gone into Documents
            "tabs" => vec![
                Step::Key(0x74, Mods::PRIMARY),
                Step::Click("Documents"),
                Step::Key(0xff54, Mods::PRIMARY),
            ],
            _ => return Err(format!("{}: no such surface", r.surface)),
        };
        let mut opened = r.surface != "window" && r.surface != "sidebar";
        let mut view = ViewMode::Icons;
        let mut selected = None;
        let mut acted = None;
        let mut places = self.places.clone();
        for (flag, value) in r.state {
            match (flag.as_str(), value.as_str()) {
                ("view", "icon") => view = ViewMode::Icons,
                ("view", "list") => view = ViewMode::List,
                ("view", "columns") => view = ViewMode::Columns,
                ("view", "gallery") => view = ViewMode::Gallery,
                ("view", _) => {
                    return Err(format!("--view {value}: icon, list, columns or gallery"))
                }
                ("state", "rename" | "ring" | "band") => acted = Some(value.to_string()),
                ("state", _) => return Err(format!("--state {value}: rename, ring or band")),
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
        let item = match (&selected, view) {
            (Some(path), _) => path.rsplit('/').next().unwrap_or(path).to_string(),
            (None, ViewMode::Gallery) => GALLERY_ITEM.to_string(),
            (None, _) => ITEM.to_string(),
        };
        match acted.as_deref() {
            Some("rename") => {
                then.extend([Step::Secondary(item), Step::Choose("Rename")]);
                opened = false;
            }
            Some("ring") => then.push(Step::Secondary(item)),
            Some(_) => {
                let from = match view {
                    ViewMode::Columns => Point::new(SIDEBAR + BAND_COLUMN, h - BAND_CORNER.1),
                    _ => Point::new(w - BAND_CORNER.0, h - BAND_CORNER.1),
                };
                then.push(Step::Band { from, over: item });
            }
            None => {}
        }
        let pm = self.draw(
            view,
            places,
            selected.as_deref(),
            &then,
            opened,
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
    cupertino_kit::test::capture::run(Capture::new)
}
