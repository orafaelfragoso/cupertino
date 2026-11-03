//! The browser window on CupertinoKit, run headless: what it lists and reads to assistive technology, where the
//! sidebar and the menus take it, and a folder changing under it.

#![cfg(feature = "app")]

use std::collections::{BTreeSet, HashMap};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_assets::BUILTIN_FONT;
use cupertino_files::kit::{prepare, Files, Start};
use cupertino_files::notice::Notices;
use cupertino_files::places::{Place, PlaceGroup, PlaceKind};
use cupertino_files::trash::{encode_path, Trash};
use cupertino_kit::core::{AccessibilityNode, Key, NodeId, PlaceholderIcons, Role, RuntimeOptions};
use cupertino_kit::geometry::Point;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowRequest, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_menu_model::{ActionName, Scope};
use cupertino_test_bus::{missing_tool, PrivateBus};
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

struct Run {
    driver: TestDriver<Files>,
    window: WindowId,
    _home: tempfile::TempDir,
}

/// A notification service that hands on the summary and body of each notification.
struct Notifications(Mutex<mpsc::Sender<String>>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let _ = self.0.lock().unwrap().send(format!("{summary} / {body}"));
        1
    }
}

fn notifications(bus: &PrivateBus) -> (Connection, mpsc::Receiver<String>) {
    let (sent, received) = mpsc::channel();
    let service = bus
        .blocking_builder()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            Notifications(Mutex::new(sent)),
        )
        .unwrap()
        .build()
        .unwrap();
    (service, received)
}

fn place(name: &str, path: &Path, kind: PlaceKind) -> Place {
    Place {
        name: name.into(),
        path: path.to_path_buf(),
        group: PlaceGroup::Favorites,
        kind,
    }
}

impl Run {
    /// A window on `home/Documents`, which holds the folder `Notes` and the files `b.txt` and `a.txt`, with
    /// Documents and Downloads in the sidebar and its trash in `home/Trash`.
    fn new() -> Run {
        Run::with(false, Notices::start(None))
    }

    /// The window of [`Run::new`] posting to `notices`, with the trash's folder as a place after Downloads when
    /// `trash_listed`.
    fn with(trash_listed: bool, notices: Notices) -> Run {
        let home = tempfile::tempdir().unwrap();
        let documents = home.path().join("Documents");
        let downloads = home.path().join("Downloads");
        let trash = home.path().join("Trash");
        std::fs::create_dir_all(documents.join("Notes")).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(documents.join("b.txt"), "bb").unwrap();
        std::fs::write(documents.join("a.txt"), "a").unwrap();
        let mut places = vec![
            place("Documents", &documents, PlaceKind::Documents),
            place("Downloads", &downloads, PlaceKind::Downloads),
        ];
        if trash_listed {
            places.push(place("Trash", &trash.join("files"), PlaceKind::Trash));
        }
        prepare(
            Start::open(
                documents,
                places,
                None,
                home.path().join("recently-used.xbel"),
                notices,
                Box::new(|| Err(std::io::Error::other("no applications in this test"))),
                None,
            )
            .unwrap()
            .with_trash(Trash::with_home(trash)),
        );
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let preferences = ThemePreferences {
            appearance: Appearance::Light,
            reduce_transparency: false,
            reduce_motion: false,
        };
        let theme = ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap();
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 4000,
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
        };
        let mut driver = TestDriver::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(budget()).unwrap();
        let window = driver.runtime().windows()[0].1;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(window, None, active)
            .unwrap();
        let mut run = Run {
            driver,
            window,
            _home: home,
        };
        run.settle();
        run
    }

    fn home(&self) -> PathBuf {
        self._home.path().to_path_buf()
    }

    fn settle(&mut self) {
        for _ in 0..8 {
            self.driver.run_until_idle(budget()).unwrap();
            let system = self.driver.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            system.frame_ready(self.window, Timestamp(0)).unwrap();
        }
    }

    fn send(&mut self, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(self.window, event)
            .unwrap();
        self.settle();
    }

    fn click(&mut self, key: impl Into<Key>) {
        let key = key.into();
        let runtime = self.driver.runtime();
        let id = runtime
            .find(self.window, &key)
            .unwrap_or_else(|| panic!("no node {key:?}"));
        let snapshot = runtime.snapshot(self.window).unwrap();
        let rect = snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect;
        let position = Point::new(
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        );
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        self.send(pointer(PointerEvent::Motion { position }));
        for pressed in [true, false] {
            self.send(pointer(PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            }));
        }
    }

    /// Presses and releases the key of `keysym`, which types `text`.
    fn key(&mut self, keysym: u32, text: Option<char>) {
        for pressed in [true, false] {
            self.send(WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(keysym),
                    text: text.filter(|_| pressed),
                    pressed,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ));
        }
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(c as u32, Some(c));
        }
    }

    fn menu(&mut self, name: &str) {
        self.send(WindowEvent::MenuActivated(action(name)));
    }

    /// The names of the accessible nodes with `role`, in tree order.
    fn named(&self, role: Role) -> Vec<String> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot
            .accessibility
            .iter()
            .filter(|(_, node)| node.role == role)
            .filter_map(|(_, node)| node.name.clone())
            .collect()
    }

    /// The items the list shows, by the text of each row's first cell.
    fn rows(&self) -> Vec<String> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let nodes: HashMap<NodeId, &AccessibilityNode> = snapshot
            .accessibility
            .iter()
            .map(|(id, node)| (*id, node))
            .collect();
        let first_text = |id: NodeId| {
            let mut stack = vec![id];
            while let Some(id) = stack.pop() {
                let node = nodes[&id];
                if node.role == Role::StaticText {
                    return node.name.clone();
                }
                stack.extend(node.children.iter().rev());
            }
            None
        };
        snapshot
            .accessibility
            .iter()
            .filter(|(_, node)| node.role == Role::Row)
            .filter(|(_, node)| node.children.iter().all(|c| nodes[c].role == Role::Cell))
            .filter_map(|(id, _)| first_text(*id))
            .collect()
    }

    /// The window's title: the last one it set, else the one it opened with.
    fn title(&self) -> String {
        let system = self.driver.runtime().window_system();
        let set = system
            .requests(self.window)
            .unwrap()
            .iter()
            .rev()
            .find_map(|request| match request {
                WindowRequest::SetTitle(title) => Some(title.clone()),
                _ => None,
            });
        set.unwrap_or_else(|| system.spec(self.window).unwrap().title.clone())
    }

    fn disabled(&self) -> BTreeSet<ActionName> {
        let menus = self
            .driver
            .runtime()
            .window_system()
            .menus(self.window)
            .unwrap();
        menus
            .expect("the window publishes its menus")
            .disabled
            .clone()
    }

    /// Settles until `done` holds, for changes a watcher thread reports.
    fn wait_until(&mut self, mut done: impl FnMut(&Run) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(Instant::now() < deadline, "the window never caught up");
            std::thread::sleep(Duration::from_millis(10));
            self.settle();
        }
    }
}

fn action(name: &str) -> ActionName {
    ActionName::new(Scope::Win, name).unwrap()
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

#[test]
fn the_folder_is_listed_by_name_under_its_columns_and_read_as_a_table() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    assert_eq!(run.title(), "Documents");
    assert_eq!(run.named(Role::Table), ["Documents"]);
    assert_eq!(
        run.named(Role::ColumnHeader),
        ["Name", "Date Modified", "Size", "Kind"]
    );
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes"]);
    run.driver.look(run.window, "files-browser").unwrap();
}

#[test]
fn a_place_in_the_sidebar_opens_its_folder_and_back_returns() {
    let mut run = Run::new();
    run.click("place-Downloads");
    assert_eq!(run.title(), "Downloads");
    assert_eq!(run.rows(), Vec::<String>::new());
    assert!(!run.disabled().contains(&action("go-back")));
    run.menu("go-back");
    assert_eq!(run.title(), "Documents");
}

#[test]
fn the_menus_disable_what_the_window_cannot_do() {
    let mut run = Run::new();
    let disabled = run.disabled();
    for name in [
        "go-back",
        "go-forward",
        "open",
        "rename",
        "view-icons",
        "eject",
    ] {
        assert!(disabled.contains(&action(name)), "{name}");
    }
    run.menu("select-all");
    let disabled = run.disabled();
    assert!(!disabled.contains(&action("open")));
    assert!(!disabled.contains(&action("duplicate")));
    assert!(disabled.contains(&action("rename")), "one item at a time");
}

#[test]
fn a_new_folder_is_named_in_its_row_and_a_duplicate_sits_next_to_its_file() {
    const RETURN: u32 = 0xff0d;
    let mut run = Run::new();
    let documents = run.home().join("Documents");
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("new-folder");
    run.wait_until(|run| {
        let runtime = run.driver.runtime();
        runtime.find(run.window, &Key::from("name-field")).is_some()
    });
    run.type_text("Projects");
    run.key(RETURN, None);
    assert!(documents.join("Projects").is_dir());
    assert!(!documents.join("untitled folder").exists());
    run.wait_until(|run| run.rows().contains(&"Projects".to_owned()));

    run.type_text("b");
    run.menu("duplicate");
    assert_eq!(std::fs::read(documents.join("b copy.txt")).unwrap(), b"bb");
    let mut names: Vec<String> = std::fs::read_dir(&documents)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["Notes", "Projects", "a.txt", "b copy.txt", "b.txt"]);
}

#[test]
fn sorting_by_size_from_the_menu_puts_the_largest_first() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("sort-size");
    assert_eq!(run.rows()[..2], ["b.txt", "a.txt"]);
}

#[test]
fn the_bars_show_the_path_and_the_item_count() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("toggle-path-bar");
    run.menu("toggle-status-bar");
    let texts = run.named(Role::StaticText);
    assert!(texts.iter().any(|t| t.starts_with("3 items")), "{texts:?}");
    assert!(texts.contains(&"Documents".to_owned()), "{texts:?}");
}

#[test]
fn items_moved_to_the_trash_are_put_back_where_they_were() {
    let mut run = Run::with(true, Notices::start(None));
    run.wait_until(|run| run.rows().len() == 3);
    let a = run.home().join("Documents/a.txt");
    let trash = run.home().join("Trash");
    run.menu("select-all");
    run.menu("move-to-trash");
    run.wait_until(|run| run.rows().is_empty());
    let info = std::fs::read_to_string(trash.join("info/a.txt.trashinfo")).unwrap();
    let path = format!("[Trash Info]\nPath={}\n", encode_path(&a.to_string_lossy()));
    assert!(info.starts_with(&path), "{info}");
    assert_eq!(
        std::fs::read_to_string(trash.join("files/a.txt")).unwrap(),
        "a"
    );
    assert!(!a.exists());

    run.click("place-Trash");
    assert_eq!(run.title(), "Trash");
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("select-all");
    assert!(!run.disabled().contains(&action("put-back")));
    run.menu("put-back");
    run.wait_until(|run| run.rows().is_empty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
    assert!(run.home().join("Documents/Notes").is_dir());
    assert!(!trash.join("info/a.txt.trashinfo").exists());
}

#[test]
fn items_that_cannot_go_to_the_trash_stay_and_are_reported_in_one_notice() {
    let Some(bus) = PrivateBus::start() else {
        missing_tool("dbus-daemon");
        return;
    };
    let (_service, received) = notifications(&bus);
    let mut run = Run::with(false, Notices::start(Some(bus.connect())));
    run.wait_until(|run| run.rows().len() == 3);
    std::fs::write(run.home().join("Trash"), "").unwrap();
    run.menu("select-all");
    run.menu("move-to-trash");
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap(),
        "3 items couldn't be moved to the Trash / File exists."
    );
    assert!(received.recv_timeout(Duration::from_millis(200)).is_err());
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes"]);
}

#[test]
fn a_file_made_in_the_folder_is_listed_without_input() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    std::fs::write(run.home().join("Documents/c.txt"), "c").unwrap();
    run.wait_until(|run| run.rows().contains(&"c.txt".to_owned()));
}

#[test]
fn a_name_longer_than_its_column_stays_inside_its_cell() {
    let mut run = Run::new();
    let long = "a name long enough to run past the column and into the next one ".repeat(4);
    std::fs::write(run.home().join("Documents").join(long.trim()), "").unwrap();
    run.wait_until(|run| run.rows().len() == 4);
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let nodes: HashMap<NodeId, &AccessibilityNode> = snapshot
        .accessibility
        .iter()
        .map(|(id, node)| (*id, node))
        .collect();
    for cell in nodes.values().filter(|node| node.role == Role::Cell) {
        for text in cell.children.iter().map(|id| nodes[id]) {
            assert!(
                text.bounds.min_x() >= cell.bounds.min_x()
                    && text.bounds.max_x() <= cell.bounds.max_x(),
                "{:?} spills out of its cell {:?}",
                text.name,
                cell.bounds
            );
        }
    }
}
