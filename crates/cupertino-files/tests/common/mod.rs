//! The browser window on CupertinoKit, run headless, for the tests that drive it.

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
use cupertino_files::window::ViewMode;
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
use cupertino_menu_model::{ActionName, Command, Entry, Scope};
use cupertino_theme::Theme;
use cupertino_ui_test::{Inspector, ManualClock, TestDriver};
use tiny_skia::Pixmap;

/// Longer than the flash a chosen menu row gives before the choice is sent.
pub const MENU_FLASH_MS: u64 = 150;

pub struct Run {
    pub driver: TestDriver<Files>,
    /// The window the helpers act on and read.
    pub window: WindowId,
    home: tempfile::TempDir,
    /// The clock's time, which frames are presented at.
    now: Timestamp,
}

pub fn place(name: &str, path: &Path, kind: PlaceKind) -> Place {
    Place {
        name: name.into(),
        path: path.to_path_buf(),
        group: PlaceGroup::Favorites,
        kind,
    }
}

/// What the window system of a [`Run`] offers besides its windows.
#[derive(Default)]
pub struct Host {
    /// Publishes the windows' accessible trees, as over AT-SPI.
    pub accessibility: Option<Box<dyn cupertino_kit::core::AccessibilityAdapter>>,
    pub capabilities: Capabilities,
    /// The renderer's limits, when not the tests' small ones (a 4K window needs more).
    pub renderer: Option<CpuConfig>,
    /// The most nodes a window may have, when not the tests' small number.
    pub node_limit: Option<usize>,
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
        let preferences = ThemePreferences {
            appearance: Appearance::Light,
            reduce_transparency: false,
            reduce_motion: false,
        };
        Run::start_with(home, folder, places, notices, system, preferences)
    }

    /// [`Run::start`] in the theme of `preferences`.
    pub fn start_with(
        home: tempfile::TempDir,
        folder: PathBuf,
        places: Vec<Place>,
        notices: Notices,
        system: Option<zbus::blocking::Connection>,
        preferences: ThemePreferences,
    ) -> Run {
        Run::start_configured(home, folder, places, notices, system, preferences, |s| s)
    }

    /// [`Run::start_with`] on a headless system with `capabilities`.
    pub fn start_on(
        home: tempfile::TempDir,
        folder: PathBuf,
        places: Vec<Place>,
        notices: Notices,
        system: Option<zbus::blocking::Connection>,
        preferences: ThemePreferences,
        capabilities: Capabilities,
    ) -> Run {
        let host = Host {
            capabilities,
            ..Host::default()
        };
        let configure = |s| s;
        Run::start_hosted(
            home,
            folder,
            places,
            notices,
            system,
            preferences,
            configure,
            host,
        )
    }

    /// [`Run::start_with`] with what `configure` makes of the window's [`Start`].
    pub fn start_configured(
        home: tempfile::TempDir,
        folder: PathBuf,
        places: Vec<Place>,
        notices: Notices,
        system: Option<zbus::blocking::Connection>,
        preferences: ThemePreferences,
        configure: impl FnOnce(Start) -> Start,
    ) -> Run {
        let host = Host::default();
        Run::start_hosted(
            home,
            folder,
            places,
            notices,
            system,
            preferences,
            configure,
            host,
        )
    }

    /// [`Run::start_configured`] on a window system that can do what `host` says.
    #[allow(clippy::too_many_arguments)]
    pub fn start_hosted(
        home: tempfile::TempDir,
        folder: PathBuf,
        places: Vec<Place>,
        notices: Notices,
        system: Option<zbus::blocking::Connection>,
        preferences: ThemePreferences,
        configure: impl FnOnce(Start) -> Start,
        host: Host,
    ) -> Run {
        prepare(configure(
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
            .with_trash(Trash::with_home(home.path().join("Trash")))
            .with_home(home.path().to_path_buf())
            .with_view(ViewMode::List),
        ));
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let theme = ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap();
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let config = host.renderer.unwrap_or(CpuConfig {
            max_pixels: 1 << 22,
            glyph_cache_bytes: 1 << 20,
            image_cache_bytes: 1 << 20,
            font_smoothing: true,
        });
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: host.node_limit.unwrap_or(4000),
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    config,
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: host.accessibility,
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let mut driver =
            TestDriver::new(host.capabilities, options, ManualClock::new(Timestamp(0)));
        driver.run_until_idle(budget()).unwrap();
        let window = driver.runtime().windows()[0].1;
        let mut run = Run {
            driver,
            window,
            home,
            now: Timestamp(0),
        };
        run.activate(window);
        run
    }

    pub fn windows(&self) -> Vec<WindowId> {
        let windows = self.driver.runtime().windows();
        windows.into_iter().map(|(_, id)| id).collect()
    }

    /// Make `window` the key window and the one the helpers act on.
    pub fn activate(&mut self, window: WindowId) {
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

    pub fn home(&self) -> PathBuf {
        self.home.path().to_path_buf()
    }

    /// Runs until no window waits for a frame, then checks that a screen reader could read every window open.
    #[track_caller]
    pub fn settle(&mut self) {
        for _ in 0..8 {
            self.driver.run_until_idle(budget()).unwrap();
            let windows = self.windows();
            let system = self.driver.window_system_mut();
            let waiting: Vec<WindowId> = windows
                .into_iter()
                .filter(|&w| system.frame_requested(w).unwrap())
                .collect();
            if waiting.is_empty() {
                break;
            }
            for window in waiting {
                system.frame_ready(window, self.now).unwrap();
            }
        }
        self.driver.assert_accessible("Files");
    }

    /// Move the clock on by `ms` milliseconds a frame of `step_ms` at a time, presenting the frames the windows ask
    /// for at each.
    pub fn frames(&mut self, ms: u64, step_ms: u64) {
        let end = self.now.0 + ms * 1_000_000;
        while self.now.0 < end {
            self.now = Timestamp((self.now.0 + step_ms * 1_000_000).min(end));
            self.driver.advance_to(self.now).unwrap();
            self.driver.run_until_idle(budget()).unwrap();
            let windows = self.windows();
            let system = self.driver.window_system_mut();
            for window in windows {
                if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, self.now).unwrap();
                }
            }
            self.driver.run_until_idle(budget()).unwrap();
        }
    }

    /// The bounds of the accessible node with `role` and `name`.
    pub fn bounds(&self, role: Role, name: &str) -> Rect {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot
            .accessibility
            .iter()
            .find(|(_, node)| node.role == role && node.name.as_deref() == Some(name))
            .map(|(_, node)| node.bounds)
            .unwrap_or_else(|| panic!("no {role:?} {name:?}"))
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

    /// Click the accessible node with `role` and `name`.
    pub fn click_named(&mut self, role: Role, name: &str) {
        let rect = self.bounds(role, name);
        self.click_at(rect);
    }

    /// Every name, value and description the window reads to assistive technology.
    pub fn read_aloud(&self) -> Vec<String> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot
            .accessibility
            .iter()
            .flat_map(|(_, node)| {
                [
                    node.name.clone(),
                    node.description.clone(),
                    node.value.as_ref().map(|v| format!("{v:?}")),
                ]
            })
            .flatten()
            .collect()
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
        self.press_at(rect.center(), MouseButton::Left);
    }

    /// Press and release `button` at `position`.
    pub fn press_at(&mut self, position: Point, button: MouseButton) {
        self.press_with(position, button, Mods::NONE);
    }

    /// Press and release `button` at `position` with `mods` held.
    pub fn press_with(&mut self, position: Point, button: MouseButton, mods: Mods) {
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        self.send(pointer(PointerEvent::Motion { position }));
        for pressed in [true, false] {
            self.send(pointer(PointerEvent::Button {
                position,
                button,
                pressed,
                serial: 1,
                mods,
            }));
        }
    }

    /// Press the left button at `from` with `mods` held, move to `to` in a few steps and let go there.
    pub fn drag(&mut self, from: Point, to: Point, mods: Mods) {
        self.hold(from, to, mods);
        self.release(to, mods);
    }

    /// The first part of [`Self::drag`]: a left press at `from` with `mods` held, moved to `to` in four steps.
    pub fn hold(&mut self, from: Point, to: Point, mods: Mods) {
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        self.send(pointer(PointerEvent::Motion { position: from }));
        self.send(pointer(PointerEvent::Button {
            position: from,
            button: MouseButton::Left,
            pressed: true,
            serial: 1,
            mods,
        }));
        for step in 1..=4 {
            let t = f64::from(step) / 4.0;
            let position = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            self.send(pointer(PointerEvent::Motion { position }));
        }
    }

    /// The left button held since [`Self::hold`] let go at `at`.
    pub fn release(&mut self, at: Point, mods: Mods) {
        let event = PointerEvent::Button {
            position: at,
            button: MouseButton::Left,
            pressed: false,
            serial: 1,
            mods,
        };
        self.send(WindowEvent::Pointer(event, Timestamp(0)));
    }

    /// Click the list's row that shows `name` with `mods` held.
    pub fn click_row_with(&mut self, name: &str, mods: Mods) {
        let (_, rect, _) = self
            .listed()
            .into_iter()
            .find(|(shown, _, _)| shown == name)
            .unwrap_or_else(|| panic!("no row {name:?}"));
        self.press_with(rect.center(), MouseButton::Left, mods);
    }

    /// A secondary click on the list's row that shows `name`.
    pub fn right_click_row(&mut self, name: &str) {
        let (_, rect, _) = self
            .listed()
            .into_iter()
            .find(|(shown, _, _)| shown == name)
            .unwrap_or_else(|| panic!("no row {name:?}"));
        self.press_at(rect.center(), MouseButton::Right);
    }

    /// The open context menu's window.
    pub fn context_menu(&self) -> Option<WindowId> {
        let windows = self.driver.runtime().windows();
        windows
            .into_iter()
            .find(|(key, _)| format!("{key:?}").contains("context-menu"))
            .map(|(_, id)| id)
    }

    /// The rows of the open context menu: each label, and whether it can be chosen.
    pub fn menu_rows(&mut self) -> Vec<(String, bool)> {
        let menu = self.context_menu().expect("a context menu is open");
        self.show(menu);
        let snapshot = self.driver.runtime().snapshot(menu).unwrap();
        snapshot
            .accessibility
            .iter()
            .filter(|(_, node)| node.role == Role::MenuItem)
            .filter_map(|(_, node)| Some((node.name.clone()?, !node.states.disabled)))
            .collect()
    }

    /// Choose the row `label` of the open context menu.
    pub fn choose(&mut self, label: &str) {
        let menu = self.context_menu().expect("a context menu is open");
        self.show(menu);
        let window = self.window;
        self.window = menu;
        let rect = self.bounds(Role::MenuItem, label);
        self.click_at(rect);
        self.window = window;
        self.settle();
        self.finish_flash();
    }

    /// Lets the flash a chosen menu row gives run to its end, when the choice is sent.
    pub fn finish_flash(&mut self) {
        self.frames(MENU_FLASH_MS, 16);
        self.settle();
    }

    /// Escape in the open context menu.
    pub fn close_menu(&mut self) {
        let menu = self.context_menu().expect("a context menu is open");
        self.show(menu);
        let window = self.window;
        self.window = menu;
        // the press closes the popup, so no release reaches it
        self.send(WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(0xff1b),
                text: None,
                pressed: true,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        ));
        self.window = window;
        self.settle();
    }

    /// The compositor shows the popup `popup`.
    fn show(&mut self, popup: WindowId) {
        self.driver
            .window_system_mut()
            .configure(popup, None, WindowStates::default())
            .unwrap();
        self.settle();
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
                    code: key_code(keysym),
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

    /// The label of the menu item that runs `name`.
    pub fn menu_label(&self, name: &str) -> String {
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

    pub fn full_screen_requests(&self) -> Vec<bool> {
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

    /// The bounds of the list's row that shows `name`.
    pub fn row_bounds(&self, name: &str) -> Rect {
        let listed = self.listed();
        let row = listed.into_iter().find(|(shown, _, _)| shown == name);
        row.unwrap_or_else(|| panic!("no row {name:?}")).1
    }

    /// Each row of the list: the text of its first cell, its bounds and whether it is selected.
    pub fn listed(&self) -> Vec<(String, Rect, bool)> {
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
        let mut rows = snapshot
            .accessibility
            .iter()
            .filter(|(_, node)| node.role == Role::Row)
            .filter(|(_, node)| node.children.iter().all(|c| nodes[c].role == Role::Cell))
            .filter_map(|(id, node)| Some((first_text(*id)?, node.bounds, node.states.selected)))
            .collect::<Vec<_>>();
        // the rows in the order the list shows them
        rows.sort_by(|a, b| a.1.min_y().total_cmp(&b.1.min_y()));
        rows
    }

    /// The items the icon grid shows', by name, and whether each is selected.
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
    pub fn icon_cells(&self) -> Vec<(String, Rect, bool)> {
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

    /// Settles until `done` holds, for changes a watcher or worker thread reports: between two looks it settles again
    /// while a window waits for a frame, and otherwise waits for a thread to wake the runtime. The ten seconds only
    /// guard against a hang.
    #[track_caller]
    pub fn wait_until(&mut self, mut done: impl FnMut(&Run) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(
                self.wait_for_progress(deadline),
                "the window never caught up"
            );
            self.settle();
        }
    }

    /// Whether the runtime has something to do before `deadline`: a window waits for a frame, or a thread woke the
    /// runtime (a task finished), which this waits for. `false` once the deadline passed with neither.
    pub fn wait_for_progress(&mut self, deadline: Instant) -> bool {
        let windows = self.windows();
        let system = self.driver.window_system_mut();
        if windows
            .into_iter()
            .any(|w| system.frame_requested(w).unwrap_or(false))
        {
            return Instant::now() < deadline;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        !left.is_zero() && system.wait_for_wake(left)
    }

    /// The runtime's waker, for a test's own thread to wake it once it is done.
    pub fn waker(&self) -> std::task::Waker {
        use cupertino_kit::platform::WindowSystem;
        self.driver.runtime().window_system().waker()
    }
}

/// The key code of the editing keys a text field reads by code (Backspace, Delete and the arrows); 0 for the others.
fn key_code(keysym: u32) -> u32 {
    match keysym {
        0xff08 => 14,
        0xffff => 111,
        0xff51 => 105,
        0xff52 => 103,
        0xff53 => 106,
        0xff54 => 108,
        _ => 0,
    }
}

pub fn action(name: &str) -> ActionName {
    let scope = if name == "new" {
        Scope::App
    } else {
        Scope::Win
    };
    ActionName::new(scope, name).unwrap()
}

pub fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}
