//! The browser window on CupertinoKit, run headless, for the tests that drive it.
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_assets::BUILTIN_FONT;
use cupertino_files::kit::{prepare, Files, Start};
use cupertino_files::notice::Notices;
use cupertino_files::places::{Place, PlaceGroup, PlaceKind};
use cupertino_files::trash::Trash;
use cupertino_kit::core::{AccessibilityNode, Key, NodeId, PlaceholderIcons, Role, RuntimeOptions};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowRequest, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_menu_model::{ActionName, Scope};
use cupertino_theme::Theme;
use cupertino_ui_test::{Inspector, ManualClock, TestDriver};
use tiny_skia::Pixmap;

pub struct Run {
    pub driver: TestDriver<Files>,
    pub window: WindowId,
    home: tempfile::TempDir,
}

pub fn place(name: &str, path: &Path, kind: PlaceKind) -> Place {
    Place {
        name: name.into(),
        path: path.to_path_buf(),
        group: PlaceGroup::Favorites,
        kind,
    }
}

impl Run {
    /// The window on `folder` of `home`, with `places` in the sidebar, its trash in `home/Trash`, posting to
    /// `notices`, and the removable media of `system`.
    pub fn start(
        home: tempfile::TempDir,
        folder: PathBuf,
        places: Vec<Place>,
        notices: Notices,
        system: Option<zbus::blocking::Connection>,
    ) -> Run {
        prepare(
            Start::open(
                folder,
                places,
                None,
                home.path().join("recently-used.xbel"),
                notices,
                Box::new(|| Err(std::io::Error::other("no applications in this test"))),
                None,
                system,
            )
            .unwrap()
            .with_trash(Trash::with_home(home.path().join("Trash"))),
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
            home,
        };
        run.settle();
        run
    }

    pub fn home(&self) -> PathBuf {
        self.home.path().to_path_buf()
    }

    pub fn settle(&mut self) {
        for _ in 0..8 {
            self.driver.run_until_idle(budget()).unwrap();
            let system = self.driver.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            system.frame_ready(self.window, Timestamp(0)).unwrap();
        }
    }

    pub fn send(&mut self, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(self.window, event)
            .unwrap();
        self.settle();
    }

    pub fn click(&mut self, key: impl Into<Key>) {
        let key = key.into();
        let runtime = self.driver.runtime();
        let id = runtime
            .find(self.window, &key)
            .unwrap_or_else(|| panic!("no node {key:?}"));
        let snapshot = runtime.snapshot(self.window).unwrap();
        let rect = snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect;
        self.click_at(rect);
    }

    /// Click the list's row that shows `name`.
    pub fn click_row(&mut self, name: &str) {
        let (_, rect, _) = self
            .listed()
            .into_iter()
            .find(|(shown, _, _)| shown == name)
            .unwrap_or_else(|| panic!("no row {name:?}"));
        self.click_at(rect);
    }

    fn click_at(&mut self, rect: Rect) {
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
    pub fn key(&mut self, keysym: u32, text: Option<char>) {
        self.key_with(keysym, text, Mods::NONE);
    }

    /// Presses and releases the key of `keysym` with `mods` held.
    pub fn key_with(&mut self, keysym: u32, text: Option<char>, mods: Mods) {
        for pressed in [true, false] {
            self.send(WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(keysym),
                    text: text.filter(|_| pressed),
                    pressed,
                    repeat: false,
                    mods,
                },
                Timestamp(0),
            ));
        }
    }

    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(c as u32, Some(c));
        }
    }

    pub fn menu(&mut self, name: &str) {
        self.send(WindowEvent::MenuActivated(action(name)));
    }

    /// The names of the accessible nodes with `role`, in tree order.
    pub fn named(&self, role: Role) -> Vec<String> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot
            .accessibility
            .iter()
            .filter(|(_, node)| node.role == role)
            .filter_map(|(_, node)| node.name.clone())
            .collect()
    }

    /// The items the list shows, by the text of each row's first cell.
    pub fn rows(&self) -> Vec<String> {
        self.listed().into_iter().map(|(name, _, _)| name).collect()
    }

    /// The items the list shows selected.
    pub fn selected_rows(&self) -> Vec<String> {
        self.listed()
            .into_iter()
            .filter(|(_, _, selected)| *selected)
            .map(|(name, _, _)| name)
            .collect()
    }

    /// Each row of the list: the text of its first cell, its bounds and whether it is selected.
    fn listed(&self) -> Vec<(String, Rect, bool)> {
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
            .filter_map(|(id, node)| Some((first_text(*id)?, node.bounds, node.states.selected)))
            .collect()
    }

    /// The items the icon grid shows, by name, and whether each is selected.
    pub fn icons(&self) -> Vec<(String, bool)> {
        self.icon_cells()
            .into_iter()
            .map(|(name, _, selected)| (name, selected))
            .collect()
    }

    /// The names of the items the icon grid shows selected.
    pub fn selected_icons(&self) -> Vec<String> {
        self.icons()
            .into_iter()
            .filter(|(_, selected)| *selected)
            .map(|(name, _)| name)
            .collect()
    }

    /// Click the icon grid's item named `name`.
    pub fn click_icon(&mut self, name: &str) {
        let (_, rect, _) = self
            .icon_cells()
            .into_iter()
            .find(|(shown, _, _)| shown == name)
            .unwrap_or_else(|| panic!("no icon {name:?}"));
        self.click_at(rect);
    }

    /// Each item of the icon grid, the list named as the window: its name, bounds and whether it is selected.
    fn icon_cells(&self) -> Vec<(String, Rect, bool)> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let nodes: HashMap<NodeId, &AccessibilityNode> = snapshot
            .accessibility
            .iter()
            .map(|(id, node)| (*id, node))
            .collect();
        let title = self.title();
        let Some(grid) = nodes
            .values()
            .find(|n| n.role == Role::List && n.name.as_deref() == Some(title.as_str()))
        else {
            return Vec::new();
        };
        grid.children
            .iter()
            .map(|id| nodes[id])
            .filter_map(|n| Some((n.name.clone()?, n.bounds, n.states.selected)))
            .collect()
    }

    /// The window's title: the last one it set, else the one it opened with.
    pub fn title(&self) -> String {
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

    pub fn disabled(&self) -> BTreeSet<ActionName> {
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

    /// Compares the last frame and the focus and accessibility tree with `tests/goldens/<scene>.png` and `.txt`.
    /// Pixels may differ by at most 2/255 per channel and are compared on x86_64 only, as the renderer's own baselines
    /// are. `CUPERTINO_UPDATE_GOLDENS=1` re-records the scene and then fails, so the variable can never pass a run.
    pub fn check_golden(&mut self, scene: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens");
        let (png, txt) = (format!("{dir}/{scene}.png"), format!("{dir}/{scene}.txt"));
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let dump = Inspector::default().dump(&snapshot);
        let focus = dump.lines().find(|l| l.starts_with("focus ")).unwrap();
        let mut text = format!("{focus}\n");
        for line in dump
            .lines()
            .skip_while(|l| *l != "semantics")
            .skip(1)
            .take_while(|l| *l != "display")
        {
            text.push_str(line);
            text.push('\n');
        }
        let frame = self
            .driver
            .window_system_mut()
            .frames(self.window)
            .unwrap()
            .last()
            .unwrap()
            .clone();
        let mut actual = Pixmap::new(frame.width, frame.height).unwrap();
        actual.data_mut().copy_from_slice(&frame.pixels);
        if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
            std::fs::create_dir_all(dir).unwrap();
            actual.save_png(&png).unwrap();
            std::fs::write(&txt, &text).unwrap();
            panic!("{scene}: re-recorded; review the diff and run again without CUPERTINO_UPDATE_GOLDENS");
        }
        let expected_text = std::fs::read_to_string(&txt).unwrap();
        if let Some(difference) = Inspector::first_difference(&expected_text, &text) {
            panic!("{scene}: the semantics differ: {difference}");
        }
        if !cfg!(target_arch = "x86_64") {
            eprintln!("SKIPPED: {scene} pixels are measured on x86_64 only");
            return;
        }
        let expected = Pixmap::load_png(&png).unwrap();
        assert_eq!(
            (expected.width(), expected.height()),
            (actual.width(), actual.height()),
            "{scene}: the frame's size"
        );
        let off = expected
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(actual.data().as_chunks::<4>().0)
            .position(|(e, a)| e.iter().zip(a.iter()).any(|(x, y)| x.abs_diff(*y) > 2));
        if let Some(i) = off {
            let (x, y) = (i % actual.width() as usize, i / actual.width() as usize);
            panic!("{scene}: differs at ({x}, {y})");
        }
    }

    /// Settles until `done` holds, for changes a watcher thread reports.
    pub fn wait_until(&mut self, mut done: impl FnMut(&Run) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(Instant::now() < deadline, "the window never caught up");
            std::thread::sleep(Duration::from_millis(10));
            self.settle();
        }
    }
}

pub fn action(name: &str) -> ActionName {
    ActionName::new(Scope::Win, name).unwrap()
}

pub fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}
