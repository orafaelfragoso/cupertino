//! The browser window on CupertinoKit, run headless: what it lists and reads to assistive technology, where the
//! sidebar and the menus take it, and a folder changing under it.

#![cfg(feature = "app")]

use std::collections::{BTreeSet, HashMap};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_assets::BUILTIN_FONT;
use cupertino_files::kit::{prepare, Files, Start};
use cupertino_files::notice::Notices;
use cupertino_files::places::{Place, PlaceGroup, PlaceKind};
use cupertino_kit::core::{AccessibilityNode, Key, NodeId, PlaceholderIcons, Role, RuntimeOptions};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowId, WindowRequest,
    WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_menu_model::{ActionName, Command, Entry, Scope};
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};

struct Run {
    driver: TestDriver<Files>,
    /// The window the helpers act on and read.
    window: WindowId,
    _home: tempfile::TempDir,
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
    /// Documents and Downloads in the sidebar.
    fn new() -> Run {
        let home = tempfile::tempdir().unwrap();
        let documents = home.path().join("Documents");
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(documents.join("Notes")).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(documents.join("b.txt"), "bb").unwrap();
        std::fs::write(documents.join("a.txt"), "a").unwrap();
        let places = vec![
            place("Documents", &documents, PlaceKind::Documents),
            place("Downloads", &downloads, PlaceKind::Downloads),
        ];
        prepare(
            Start::open(
                documents,
                places,
                None,
                home.path().join("recently-used.xbel"),
                Notices::start(None),
                Box::new(|| Err(std::io::Error::other("no applications in this test"))),
                None,
            )
            .unwrap(),
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
        let mut run = Run {
            driver,
            window,
            _home: home,
        };
        run.activate(window);
        run
    }

    fn windows(&self) -> Vec<WindowId> {
        let windows = self.driver.runtime().windows();
        windows.into_iter().map(|(_, id)| id).collect()
    }

    /// Make `window` the key window and the one the helpers act on.
    fn activate(&mut self, window: WindowId) {
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        self.driver
            .window_system_mut()
            .configure(window, None, active)
            .unwrap();
        self.window = window;
        self.settle();
    }

    fn home(&self) -> PathBuf {
        self._home.path().to_path_buf()
    }

    fn settle(&mut self) {
        for _ in 0..8 {
            self.driver.run_until_idle(budget()).unwrap();
            let windows = self.windows();
            let system = self.driver.window_system_mut();
            let waiting: Vec<WindowId> = windows
                .into_iter()
                .filter(|&w| system.frame_requested(w).unwrap())
                .collect();
            if waiting.is_empty() {
                return;
            }
            for window in waiting {
                system.frame_ready(window, Timestamp(0)).unwrap();
            }
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
        self.click_in(rect);
    }

    /// Click the list's row whose first cell reads `name`.
    fn click_row(&mut self, name: &str) {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let cell = snapshot
            .accessibility
            .iter()
            .find(|(_, node)| node.role == Role::StaticText && node.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no row {name}"));
        let rect = cell.1.bounds;
        self.click_in(rect);
    }

    fn click_in(&mut self, rect: Rect) {
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

    fn menu(&mut self, name: &str) {
        self.send(WindowEvent::MenuActivated(action(name)));
    }

    /// The label of the menu item that runs `name`.
    fn menu_label(&self, name: &str) -> String {
        let system = self.driver.runtime().window_system();
        let menus = system.menus(self.window).unwrap().unwrap();
        let command = Command::Action(action(name));
        let item = menus
            .bar
            .menus
            .iter()
            .flat_map(|menu| menu.sections.iter().flatten())
            .find_map(|entry| match entry {
                Entry::Item(item) if item.command == command => Some(item.label.clone()),
                _ => None,
            });
        item.unwrap_or_else(|| panic!("no menu item {name}"))
    }

    fn full_screen_requests(&self) -> Vec<bool> {
        let system = self.driver.runtime().window_system();
        let requests = system.requests(self.window).unwrap();
        requests
            .iter()
            .filter_map(|request| match request {
                WindowRequest::Fullscreen(on) => Some(*on),
                _ => None,
            })
            .collect()
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
    let scope = if name == "new" {
        Scope::App
    } else {
        Scope::Win
    };
    ActionName::new(scope, name).unwrap()
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
    assert!(disabled.contains(&action("rename")), "not yet");
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

#[test]
fn a_folder_opens_in_a_second_window_whose_menus_act_on_it_alone() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let first = run.window;
    assert!(run.disabled().contains(&action("open-in-new-window")));
    run.click_row("Notes");
    assert!(!run.disabled().contains(&action("open-in-new-window")));
    run.menu("open-in-new-window");
    let windows = run.windows();
    assert_eq!(windows.len(), 2);
    run.activate(windows[1]);
    assert_eq!(run.title(), "Notes");

    run.menu("go-enclosing-folder");
    assert_eq!(run.title(), "Documents");
    run.menu("fullscreen");
    assert_eq!(run.full_screen_requests(), [true]);
    assert_eq!(run.menu_label("fullscreen"), "Exit Full Screen");
    run.activate(first);
    assert_eq!(run.title(), "Documents");
    assert!(run.disabled().contains(&action("go-back")));
    assert_eq!(run.full_screen_requests(), Vec::<bool>::new());
    assert_eq!(run.menu_label("fullscreen"), "Enter Full Screen");

    run.menu("new");
    assert_eq!(run.windows().len(), 3);
    run.send(WindowEvent::CloseRequested);
    assert_eq!(run.windows().len(), 2);
    assert!(!run.windows().contains(&first));
}

#[test]
fn full_screen_asks_the_platform_and_the_menu_follows() {
    let mut run = Run::new();
    assert_eq!(run.menu_label("fullscreen"), "Enter Full Screen");
    run.menu("fullscreen");
    assert_eq!(run.full_screen_requests(), [true]);
    assert_eq!(run.menu_label("fullscreen"), "Exit Full Screen");
    run.menu("fullscreen");
    assert_eq!(run.full_screen_requests(), [true, false]);
    assert_eq!(run.menu_label("fullscreen"), "Enter Full Screen");
}
