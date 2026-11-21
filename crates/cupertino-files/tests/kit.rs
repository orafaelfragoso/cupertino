//! Files on Kit's tasks, driven headless through its windows: a file operation runs in slices on the I/O pool, the
//! sidebar's places follow the folders they come from and the trash, and a launch that finds no session bus opens its
//! window on its own.

#![expect(
    clippy::disallowed_methods,
    reason = "the test makes and removes its own files on its own thread; it has no UI thread"
)]

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cupertino_assets::BUILTIN_FONT;
use cupertino_files::instance::Request;
use cupertino_files::kit::{prepare, Buses, Files, Start};
use cupertino_files::trash::Trash;
use cupertino_kit::core::{Role, RuntimeOptions};
use cupertino_kit::files::places::{standard_places, Place};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, Timestamp, WindowId};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowEvent, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};

/// The runtime turns between two looks at the window.
const TURNS: u32 = 1000;
/// How long a test waits for a task on another thread before it calls the run hung.
const GUARD: Duration = Duration::from_secs(20);
/// The window's size, wide enough for the sidebar and the listing.
const WINDOW: Size = Size::new(900.0, 600.0);
/// The name of Kit's I/O pool's threads, before their number.
const IO_THREAD: &str = "kit-io";

/// Options that draw on the CPU with the built-in font, in the neutral light theme.
fn options() -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance: Appearance::Light,
                reduce_transparency: false,
                reduce_motion: true,
            })
            .unwrap(),
        ),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 20_000,
        renderers: Box::new(move || {
            Box::new(CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 22,
                    glyph_cache_bytes: 1 << 20,
                    image_cache_bytes: 1 << 20,
                    font_smoothing: true,
                },
                GlyphRaster::new(Arc::clone(&glyphs)),
            ))
        }),
        accessibility: None,
        clock: Arc::new(ManualClock::new(Timestamp(0))),
        on_window_error: None,
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(TURNS).unwrap()
}

/// A folder of its own for a test: the home (`home`, with `files`), the trash's folder beside it (`Trash`, not
/// made) and the Recents file.
struct Sample {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Sample {
    fn new(files: &[&str]) -> Sample {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir(root.join("home")).unwrap();
        for name in files {
            std::fs::write(root.join("home").join(name), name.as_bytes()).unwrap();
        }
        Sample { _dir: dir, root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn trash_files(&self) -> PathBuf {
        self.root.join("Trash/files")
    }

    /// The places of the home and the trash, named as the sidebar shows them.
    fn places(&self) -> Vec<Place> {
        standard_places(&self.home(), &[], &self.trash_files(), None, "Computer")
    }

    /// Files on the home, with this sample's trash.
    fn start(&self) -> Start {
        self.start_with(self.places())
    }

    /// Files on the home with the sidebar's `places`, with this sample's trash.
    fn start_with(&self, places: Vec<Place>) -> Start {
        Start::open(
            self.home(),
            places,
            None,
            self.root.join("recently-used.xbel"),
            Box::new(|| Err(std::io::Error::other("no applications in a test"))),
            None,
        )
        .with_trash(Trash::with_home(self.root.join("Trash")))
        .with_home(self.home())
    }
}

/// A driver of Files on `start` and its window, shown and active.
fn started(start: Start) -> (TestDriver<Files>, WindowId) {
    prepare(start);
    let mut driver = TestDriver::<Files>::new(
        Capabilities::default(),
        options(),
        ManualClock::new(Timestamp(0)),
    );
    let window = wait_for_window(&mut driver);
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    driver
        .window_system_mut()
        .configure(window, Some(WINDOW), active)
        .unwrap();
    driver.run_until_idle(turns()).unwrap();
    (driver, window)
}

/// The first window, once Files opened it: the launch may wait on a task before it does.
fn wait_for_window(driver: &mut TestDriver<Files>) -> WindowId {
    loop {
        driver.run_until_idle(turns()).unwrap();
        if let Some((_, window)) = driver.runtime().windows().first() {
            return *window;
        }
        assert!(
            driver.window_system_mut().wait_for_wake(GUARD),
            "Files opened no window"
        );
    }
}

/// Run the window until `done` holds, waiting for the tasks on other threads to wake it between two looks.
fn wait_until(
    driver: &mut TestDriver<Files>,
    what: &str,
    done: impl Fn(&TestDriver<Files>) -> bool,
) {
    loop {
        driver.run_until_idle(turns()).unwrap();
        if done(driver) {
            return;
        }
        assert!(
            driver.window_system_mut().wait_for_wake(GUARD),
            "nothing woke Files while waiting until {what}; it shows {:?}",
            texts(driver)
        );
    }
}

/// The texts every window shows.
fn texts(driver: &TestDriver<Files>) -> Vec<String> {
    driver
        .runtime()
        .windows()
        .into_iter()
        .filter_map(|(_, window)| driver.runtime().snapshot(window))
        .flat_map(|snapshot| {
            snapshot
                .accessibility
                .iter()
                .filter_map(|(_, n)| n.name.as_ref().map(|name| format!("{:?} {name}", n.role)))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whether the window shows the text `name`.
fn shows(driver: &TestDriver<Files>, window: WindowId, name: &str) -> bool {
    has(driver, window, Role::StaticText, name)
}

/// Whether the window's sidebar or listing has the row `name`.
fn row(driver: &TestDriver<Files>, window: WindowId, name: &str) -> bool {
    has(driver, window, Role::ListItem, name)
}

fn has(driver: &TestDriver<Files>, window: WindowId, role: Role, name: &str) -> bool {
    driver.runtime().snapshot(window).is_some_and(|snapshot| {
        snapshot
            .accessibility
            .iter()
            .any(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
    })
}

/// The key of `keysym` pressed and let go with `mods` held.
fn key(driver: &mut TestDriver<Files>, window: WindowId, keysym: u32, mods: Mods) {
    for pressed in [true, false] {
        let event = WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: None,
                pressed,
                repeat: false,
                mods,
            },
            Timestamp(0),
        );
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
        driver.run_until_idle(turns()).unwrap();
    }
}

/// The progress a file operation reports after each of its slices, with the thread the slice ran on.
#[derive(Debug)]
struct Reported {
    thread: String,
    items: usize,
    total_items: usize,
}

#[test]
fn a_file_operation_runs_in_slices_on_the_io_pool_and_the_window_follows_them() {
    let sample = Sample::new(&["a.txt", "b.txt"]);
    let reported: Arc<Mutex<Vec<Reported>>> = Arc::default();
    let observer = Arc::clone(&reported);
    let start = sample
        .start()
        .with_transfer_observer(Arc::new(move |progress| {
            let thread = std::thread::current().name().unwrap_or_default().to_owned();
            observer.lock().unwrap().push(Reported {
                thread,
                items: progress.items,
                total_items: progress.total_items,
            });
        }));
    let (mut driver, window) = started(start);
    wait_until(&mut driver, "the home is listed", |d| {
        shows(d, window, "a.txt") && shows(d, window, "b.txt")
    });
    // Select All, then New Folder with Selection
    key(&mut driver, window, u32::from('a'), Mods::PRIMARY);
    key(
        &mut driver,
        window,
        u32::from('n'),
        Mods::CTRL | Mods::PRIMARY,
    );
    let folder = sample.home().join("untitled folder");
    let home = sample.home();
    wait_until(&mut driver, "both items moved into the new folder", |_| {
        folder.join("a.txt").is_file()
            && folder.join("b.txt").is_file()
            && !home.join("a.txt").exists()
            && !home.join("b.txt").exists()
    });
    wait_until(&mut driver, "the operation reported its end", |_| {
        reported
            .lock()
            .unwrap()
            .last()
            .is_some_and(|r| r.items == 2)
    });
    let reported = reported.lock().unwrap();
    // the first slice makes the folder, the ones after it move the items into it
    assert!(reported.len() >= 2, "{reported:?}");
    assert_eq!(reported[0].items, 0, "{reported:?}");
    assert!(
        reported
            .iter()
            .all(|r| r.thread.starts_with(IO_THREAD) && r.total_items == 2),
        "every slice runs on the I/O pool: {reported:?}"
    );
    assert!(
        reported.windows(2).all(|w| w[0].items <= w[1].items),
        "the progress only grows: {reported:?}"
    );
}

#[test]
fn the_trashs_place_comes_with_its_folder_and_goes_with_it() {
    let sample = Sample::new(&["a.txt"]);
    let (mut driver, window) = started(sample.start());
    wait_until(&mut driver, "the home is listed", |d| {
        shows(d, window, "a.txt")
    });
    assert!(!row(&driver, window, "Trash"), "no trash yet");
    std::fs::create_dir_all(sample.trash_files()).unwrap();
    wait_until(&mut driver, "the trash's place shows", |d| {
        row(d, window, "Trash")
    });
    std::fs::remove_dir_all(sample.trash_files()).unwrap();
    wait_until(&mut driver, "the trash's place went", |d| {
        !row(d, window, "Trash")
    });
}

#[test]
fn the_places_are_read_again_when_a_folder_they_come_from_changes() {
    let sample = Sample::new(&["a.txt"]);
    let config = sample.root.join("config");
    std::fs::create_dir(&config).unwrap();
    let name = config.join("home-name");
    std::fs::write(&name, "First Home").unwrap();
    let source_places = sample.places();
    let read = name.clone();
    let source = Arc::new(move || {
        let mut places = source_places.clone();
        places[0].name = std::fs::read_to_string(&read)?;
        Ok(places)
    });
    let mut places = sample.places();
    places[0].name = "First Home".to_owned();
    let start = sample
        .start_with(places)
        .with_places_source(source, vec![config]);
    let (mut driver, window) = started(start);
    wait_until(&mut driver, "the first name shows", |d| {
        row(d, window, "First Home")
    });
    std::fs::write(&name, "Second Home").unwrap();
    wait_until(&mut driver, "the place's new name shows", |d| {
        row(d, window, "Second Home") && !row(d, window, "First Home")
    });
}

#[test]
fn a_launch_that_finds_no_session_bus_opens_its_window_on_its_own() {
    let sample = Sample::new(&["a.txt"]);
    let none = format!("unix:path={}", sample.root.join("no-bus").display());
    // set before any thread of the test starts
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &none);
    std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &none);
    let start = sample.start().with_buses(Buses::Launch(Request::Activate));
    let (mut driver, window) = started(start);
    wait_until(&mut driver, "the home is listed", |d| {
        shows(d, window, "a.txt")
    });
    assert!(!driver.runtime().exited());
}
