//! Files on Kit's tasks, driven headless through its windows: a file operation runs in slices on the I/O pool, the
//! sidebar's places follow the folders they come from and the trash, and a launch that finds no session bus opens its
//! window on its own. On a private session bus a launch claims Files' name, hands its request to a Files that owns it,
//! claims the name once that one quits, and the items a call names are looked up without taking every I/O turn.

#![expect(
    clippy::disallowed_methods,
    reason = "the test makes and removes its own files on its own thread; it has no UI thread"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cupertino_assets::BUILTIN_FONT;
use cupertino_files::instance::{self, Request, Target};
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
use zbus::fdo;
use zbus::zvariant::OwnedValue;

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

#[test]
fn moving_the_first_item_to_the_trash_brings_the_trashs_place_at_once() {
    let sample = Sample::new(&["a.txt"]);
    // a trash in a folder that is not there yet: no watch hears it come, only the move itself
    let trash = sample.root.join("deep/er/Trash");
    let places = standard_places(&sample.home(), &[], &trash.join("files"), None, "Computer");
    let start = sample
        .start_with(places)
        .with_trash(Trash::with_home(trash.clone()));
    let (mut driver, window) = started(start);
    wait_until(&mut driver, "the home is listed", |d| {
        shows(d, window, "a.txt")
    });
    assert!(!row(&driver, window, "Trash"), "no trash yet");
    // Select All, then Move to Trash
    key(&mut driver, window, u32::from('a'), Mods::PRIMARY);
    key(&mut driver, window, BACKSPACE, Mods::PRIMARY);
    wait_until(&mut driver, "the trash's place shows", |d| {
        row(d, window, "Trash")
    });
    assert!(trash.join("files/a.txt").is_file());
}

/// The keysym of the Backspace key.
const BACKSPACE: u32 = 0xff08;

/// A private session bus for one test, named in the test process's environment (the system bus is none), stopped
/// when the test ends.
struct PrivateBus {
    daemon: Child,
    address: String,
}

impl PrivateBus {
    /// Start the bus. Call it before any thread of the test starts: it sets the environment.
    fn start(sample: &Sample) -> PrivateBus {
        let mut daemon = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        let none = format!("unix:path={}", sample.root.join("no-bus").display());
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", none);
        PrivateBus { daemon, address }
    }

    /// A connection of the test's own, which does its work on threads of its own.
    fn connect(&self) -> zbus::Connection {
        zbus::block_on(
            zbus::connection::Builder::address(self.address.as_str())
                .unwrap()
                .build(),
        )
        .unwrap()
    }

    /// The connection that owns `name`, if any.
    fn owner(&self, name: &str) -> Option<String> {
        let connection = self.connect();
        zbus::block_on(async {
            fdo::DBusProxy::new(&connection)
                .await
                .unwrap()
                .get_name_owner(name.try_into().unwrap())
                .await
                .ok()
                .map(|owner| owner.to_string())
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// The calls a stand-in of a running Files was made, as `Method(arguments)`.
type Calls = Arc<Mutex<Vec<String>>>;

/// A stand-in of a running Files at [`instance::OBJECT_PATH`]. Given `quitting`, it is quitting: it refuses the
/// first `Activate` as a quitting Files does once `quitting` lets it answer, keeping the name, and lets go of the
/// name at each later one before it refuses it.
struct Running {
    calls: Calls,
    quitting: Option<async_channel::Receiver<()>>,
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl Running {
    async fn activate(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        _platform_data: HashMap<String, OwnedValue>,
    ) -> fdo::Result<()> {
        let calls = {
            let mut calls = self.calls.lock().unwrap();
            calls.push("Activate".to_owned());
            calls.len()
        };
        let Some(answer) = &self.quitting else {
            return Ok(());
        };
        if calls == 1 {
            let _ = answer.recv().await;
        } else {
            connection.release_name(instance::BUS_NAME).await?;
        }
        Err(fdo::Error::Disconnected("Files is quitting".into()))
    }

    fn open(&self, uris: Vec<String>, _hint: String, _platform_data: HashMap<String, OwnedValue>) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("Open({})", uris.join(" ")));
    }
}

impl PrivateBus {
    /// A stand-in of a running Files that owns [`instance::BUS_NAME`] ([`Running`]), and the calls it was made.
    fn running(&self, quitting: Option<async_channel::Receiver<()>>) -> (zbus::Connection, Calls) {
        let calls = Calls::default();
        let running = Running {
            calls: Arc::clone(&calls),
            quitting,
        };
        let connection = zbus::block_on(
            zbus::connection::Builder::address(self.address.as_str())
                .unwrap()
                .serve_at(instance::OBJECT_PATH, running)
                .unwrap()
                .name(instance::BUS_NAME)
                .unwrap()
                .build(),
        )
        .unwrap();
        (connection, calls)
    }
}

/// A driver of Files on `start`, which may open no window.
fn launched(start: Start) -> TestDriver<Files> {
    prepare(start);
    TestDriver::<Files>::new(
        Capabilities::default(),
        options(),
        ManualClock::new(Timestamp(0)),
    )
}

/// [`wait_until`] for what may hold without waking Files (a call a stand-in was made, a look-up under way), looking
/// again whenever nothing wakes it for [`QUIET`], and then moving the clock on by `tick`: given
/// [`instance::RETRY_PAUSE`], the pauses between a launch's tries pass.
fn wait_looking(
    driver: &mut TestDriver<Files>,
    what: &str,
    tick: Duration,
    done: impl Fn(&TestDriver<Files>) -> bool,
) {
    let began = Instant::now();
    loop {
        driver.run_until_idle(turns()).unwrap();
        if done(driver) {
            return;
        }
        assert!(began.elapsed() < GUARD, "Files never came to {what}");
        if !driver.window_system_mut().wait_for_wake(QUIET) {
            driver.clock().advance_by(nanos(tick));
        }
    }
}

/// Run Files until nothing wakes it for [`QUIET`]: the answers on the bus it waits for came.
fn settle(driver: &mut TestDriver<Files>) {
    loop {
        driver.run_until_idle(turns()).unwrap();
        if !driver.window_system_mut().wait_for_wake(QUIET) {
            return;
        }
    }
}

/// How long nothing wakes Files before a test takes it as waiting for time to pass.
const QUIET: Duration = Duration::from_millis(200);

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap()
}

#[test]
fn a_launch_claims_the_name_when_no_files_runs_and_opens_its_window() {
    let sample = Sample::new(&["a.txt"]);
    let bus = PrivateBus::start(&sample);
    let start = sample.start().with_buses(Buses::Launch(Request::Activate));
    let (mut driver, window) = started(start);
    wait_until(&mut driver, "the home is listed", |d| {
        shows(d, window, "a.txt")
    });
    assert!(!driver.runtime().exited());
    assert!(
        bus.owner(instance::BUS_NAME).is_some(),
        "the launch owns {}",
        instance::BUS_NAME
    );
}

#[test]
fn a_launch_hands_its_folder_to_the_running_files_and_exits_without_a_window() {
    let sample = Sample::new(&["a.txt"]);
    let bus = PrivateBus::start(&sample);
    let (_running, calls) = bus.running(None);
    let target = Target::Folder(sample.home());
    let start = sample
        .start()
        .with_buses(Buses::Launch(Request::Open(vec![target.clone()])));
    let mut driver = launched(start);
    wait_until(&mut driver, "the launch exits", |d| d.runtime().exited());
    assert!(driver.runtime().windows().is_empty(), "no window opened");
    assert_eq!(*calls.lock().unwrap(), [format!("Open({})", target.uri())]);
}

#[test]
fn a_launch_claims_the_name_once_the_quitting_files_lets_go_of_it() {
    let sample = Sample::new(&["a.txt"]);
    let bus = PrivateBus::start(&sample);
    let (answer, quitting) = async_channel::bounded(1);
    let (_running, calls) = bus.running(Some(quitting));
    let start = sample.start().with_buses(Buses::Launch(Request::Activate));
    let mut driver = launched(start);
    wait_looking(
        &mut driver,
        "the launch hands its request over",
        Duration::ZERO,
        |_| !calls.lock().unwrap().is_empty(),
    );
    // the quitting Files answers just before the hand-over's time is up, still owning the name ...
    driver
        .clock()
        .advance_by(nanos(instance::FORWARD_TIMEOUT - Duration::from_millis(10)));
    driver.run_until_idle(turns()).unwrap();
    answer.send_blocking(()).unwrap();
    settle(&mut driver);
    // ... so the hand-over's time is up before the pause after it: the try after the pause decides
    driver
        .clock()
        .advance_by(nanos(instance::RETRY_PAUSE + Duration::from_millis(20)));
    wait_looking(&mut driver, "its own window", instance::RETRY_PAUSE, |d| {
        assert!(!d.runtime().exited(), "the launch failed");
        !d.runtime().windows().is_empty()
    });
    assert!(
        calls.lock().unwrap().len() >= 2,
        "{:?}",
        calls.lock().unwrap()
    );
    let own = bus.owner(instance::BUS_NAME);
    assert!(own.is_some(), "the launch owns {}", instance::BUS_NAME);
}

/// A look-up of items that waits, as on a mount that does not answer, until it is let go, and counts the look-ups
/// under way.
#[derive(Clone, Default)]
struct Stalled {
    gone: Arc<(Mutex<bool>, Condvar)>,
    under_way: Arc<AtomicUsize>,
    most: Arc<AtomicUsize>,
}

impl Stalled {
    fn look_up(&self, path: &Path) -> std::io::Result<()> {
        let now = self.under_way.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        let (gone, let_go) = &*self.gone;
        let mut gone = gone.lock().unwrap();
        while !*gone {
            gone = let_go.wait(gone).unwrap();
        }
        self.under_way.fetch_sub(1, Ordering::SeqCst);
        std::fs::symlink_metadata(path).map(drop)
    }

    fn let_go(&self) {
        let (gone, let_go) = &*self.gone;
        *gone.lock().unwrap() = true;
        let_go.notify_all();
    }
}

#[test]
fn look_ups_on_a_mount_that_does_not_answer_leave_the_listings_a_turn() {
    let names = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"];
    let sample = Sample::new(&names);
    std::fs::create_dir(sample.root.join("other")).unwrap();
    std::fs::write(sample.root.join("other/listed.txt"), "").unwrap();
    let bus = PrivateBus::start(&sample);
    let stalled = Stalled::default();
    let look = stalled.clone();
    let start = sample
        .start()
        .with_buses(Buses::Launch(Request::Activate))
        .with_look_up(Arc::new(move |path| look.look_up(path)));
    let (mut driver, _) = started(start);
    // a caller of its own for each call, which waits for its answer while Files runs here
    let callers: Vec<_> = names
        .iter()
        .map(|name| {
            let connection = bus.connect();
            let uri = Target::Folder(sample.home().join(name)).uri();
            std::thread::spawn(move || {
                zbus::block_on(connection.call_method(
                    Some(instance::BUS_NAME),
                    instance::FILE_MANAGER_PATH,
                    Some(instance::FILE_MANAGER_INTERFACE),
                    "ShowItems",
                    &(vec![uri], ""),
                ))
                .map(drop)
            })
        })
        .collect();
    let most = cupertino_kit::task::IO_SHARE - 1;
    wait_looking(
        &mut driver,
        "the look-ups hold their share",
        Duration::ZERO,
        |_| stalled.under_way.load(Ordering::SeqCst) == most,
    );
    // with every look-up held, a folder is still listed
    let other = Target::Folder(sample.root.join("other")).uri();
    let opener = bus.connect();
    let opened = std::thread::spawn(move || {
        zbus::block_on(opener.call_method(
            Some(instance::BUS_NAME),
            instance::OBJECT_PATH,
            Some(instance::INTERFACE),
            "Open",
            &(vec![other], "", instance::platform_data(None)),
        ))
        .map(drop)
    });
    wait_until(&mut driver, "the other folder is listed", |d| {
        d.runtime()
            .windows()
            .into_iter()
            .any(|(_, window)| shows(d, window, "listed.txt"))
    });
    opened.join().unwrap().unwrap();
    assert_eq!(stalled.most.load(Ordering::SeqCst), most);
    stalled.let_go();
    wait_looking(
        &mut driver,
        "every call is answered",
        Duration::ZERO,
        |_| callers.iter().all(|caller| caller.is_finished()),
    );
    for caller in callers {
        caller.join().unwrap().unwrap();
    }
    assert!(stalled.most.load(Ordering::SeqCst) <= most);
}
