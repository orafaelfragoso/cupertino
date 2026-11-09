//! The images of the browser window's items on CupertinoKit, run headless: each item wears the icon of its type from
//! the asset packs (a fixture pack of solid-colour icons, never the machine's icon themes), a symbolic link the alias
//! badge, and a picture its thumbnail once a worker made it in a temporary thumbnail cache. The thumbnails follow the
//! file when it changes, only the items in view get one, and a dragged picture shows it.

#![cfg(feature = "app")]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use common::{budget, place};
use cupertino_assets::{Assets, BUILTIN_FONT};
use cupertino_files::kit::{prepare, Files, Start};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_files::thumbnail::Thumbnailer;
use cupertino_files::trash::Trash;
use cupertino_kit::core::{AccessibilityNode, NodeId, PlaceholderIcons, Role, RuntimeOptions};
use cupertino_kit::file_icons::{FileIcons, FILE_ICON_NAMES};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, DragProgress, KeyEvent, Keysym, Mods, MouseButton, PlatformEvent, PointerEvent,
    Timestamp, WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer, PixelFormat};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};

/// How long a test waits for the folder's watcher or a thumbnail worker.
const WAIT: Duration = Duration::from_secs(20);
/// How long no thumbnail worker wakes the window before its workers count as settled.
const SETTLED: Duration = Duration::from_millis(200);
/// The colour of the pictures, and of a picture after it changed.
const PICTURE: [u8; 3] = [230, 30, 60];
const CHANGED: [u8; 3] = [20, 40, 220];

/// The red component of the solid colour the fixture pack gives the icon `name`, distinct per name.
fn red(name: &str) -> u8 {
    name.bytes()
        .fold(11u8, |h, b| h.wrapping_mul(31).wrapping_add(b))
        | 1
}

/// A pack in `root` with a solid 16 px raster for every file icon: its red component from [`red`], no green or blue.
fn pack(root: &Path) -> Assets {
    let icons: Vec<String> = FILE_ICON_NAMES
        .iter()
        .map(|n| format!("\"{n}\": {{\"raster\": [16]}}"))
        .collect();
    std::fs::write(
        root.join("manifest.json"),
        format!(
            "{{\"version\": 1, \"fonts\": {{\"ui\": \"Fixture\"}}, \"icons\": {{{}}}, \"cursors\": \"fixture\", \"wallpaper\": \"wallpapers/default.png\"}}",
            icons.join(", ")
        ),
    )
    .unwrap();
    for rel in [
        "fonts/a.ttf",
        "cursors/fixture/index.theme",
        "wallpapers/default.png",
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }
    for name in FILE_ICON_NAMES {
        let dir = root.join("icons").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        picture(&dir.join("16.png"), 16, [red(name), 0, 0]);
    }
    Assets::open(root).unwrap()
}

/// A PNG `side` pixels square of one opaque colour.
fn picture(path: &Path, side: u32, [r, g, b]: [u8; 3]) {
    let mut pm = tiny_skia::Pixmap::new(side, side).unwrap();
    pm.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
    pm.save_png(path).unwrap();
}

/// A browser window on `home/Documents` with the fixture pack's icons and thumbnails in `home/cache`.
struct Browser {
    driver: TestDriver<Files>,
    window: WindowId,
    home: tempfile::TempDir,
}

impl Browser {
    /// The window on the folder `fill` makes, in the icon view or the list view.
    fn open(fill: impl FnOnce(&Path), list: bool) -> Browser {
        let home = tempfile::tempdir().unwrap();
        let documents = home.path().join("Documents");
        std::fs::create_dir_all(&documents).unwrap();
        fill(&documents);
        let packs = home.path().join("pack");
        std::fs::create_dir_all(&packs).unwrap();
        let icons = FileIcons::without_host(&pack(&packs), Default::default(), HashMap::new());
        prepare(
            Start::open(
                documents.clone(),
                vec![place("Documents", &documents, PlaceKind::Documents)],
                None,
                home.path().join("recently-used.xbel"),
                Notices::start(None),
                Box::new(|| Err(std::io::Error::other("no applications in this test"))),
                None,
                None,
            )
            .unwrap()
            .with_trash(Trash::with_home(home.path().join("Trash")))
            .with_file_icons(icons)
            .with_thumbnails(Thumbnailer::new(
                home.path().join("cache"),
                "cupertino-files",
            )),
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
                        image_cache_bytes: 8 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: None,
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let capabilities = Capabilities {
            drag_and_drop: true,
            ..Capabilities::default()
        };
        let mut driver = TestDriver::new(capabilities, options, ManualClock::new(Timestamp(0)));
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
        let mut browser = Browser {
            driver,
            window,
            home,
        };
        browser.settle();
        browser.send(WindowEvent::MenuActivated(common::action(if list {
            "view-list"
        } else {
            "view-icons"
        })));
        browser
    }

    fn documents(&self) -> PathBuf {
        self.home.path().join("Documents")
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

    fn pointer(&mut self, event: PointerEvent) {
        self.send(WindowEvent::Pointer(event, Timestamp(0)));
    }

    fn button(&mut self, position: Point, pressed: bool) {
        self.pointer(PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        });
    }

    fn nodes(&self) -> Vec<AccessibilityNode> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot.accessibility.into_iter().map(|(_, n)| n).collect()
    }

    /// The bounds of the item `name`'s cell in the grid, or of its row in the list.
    fn item(&self, name: &str) -> Option<Rect> {
        self.nodes()
            .into_iter()
            .find(|n| n.role == Role::ListItem && n.name.as_deref() == Some(name))
            .map(|n| n.bounds)
    }

    /// The bounds of the text `name`.
    fn label(&self, name: &str) -> Option<Rect> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let nodes: HashMap<NodeId, AccessibilityNode> =
            snapshot.accessibility.into_iter().collect();
        nodes
            .values()
            .find(|n| n.role == Role::StaticText && n.name.as_deref() == Some(name))
            .map(|n| n.bounds)
    }

    /// Item `name`'s icon: the image in its cell in the grid, the one before its name on its row in the list.
    fn icon(&self, name: &str, list: bool) -> Rect {
        let label = self
            .label(name)
            .unwrap_or_else(|| panic!("{name} is not in view"));
        let cell = self.item(name);
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let images = snapshot
            .nodes
            .iter()
            .filter(|n| n.widget.ends_with("Image"))
            .map(|n| n.rect);
        let found = if list {
            images
                .filter(|r| (r.center().y - label.center().y).abs() < 4.0)
                .filter(|r| r.max_x() <= label.min_x() + 0.5)
                .max_by(|a, b| a.max_x().total_cmp(&b.max_x()))
        } else {
            let cell = cell.expect("a cell");
            images.into_iter().find(|r| cell.contains(r.center()))
        };
        found.unwrap_or_else(|| panic!("no icon for {name}"))
    }

    /// The middle of item `name`'s icon.
    fn icon_middle(&self, name: &str, list: bool) -> Point {
        self.icon(name, list).center()
    }

    /// The colour of the last frame at `at`, unpremultiplied.
    fn pixel(&mut self, at: Point) -> [u8; 3] {
        let frame = self
            .driver
            .window_system_mut()
            .frames(self.window)
            .unwrap()
            .last()
            .unwrap()
            .clone();
        let (x, y) = (at.x.round() as u32, at.y.round() as u32);
        let i = ((y * frame.width + x) * 4) as usize;
        let p = &frame.pixels[i..i + 4];
        let a = u32::from(p[3]).max(1);
        let straight = |c: u8| match frame.format {
            PixelFormat::Rgba8 => c,
            _ => ((u32::from(c) * 255 + a / 2) / a).min(255) as u8,
        };
        [straight(p[0]), straight(p[1]), straight(p[2])]
    }

    /// The colour of item `name`'s icon in the middle.
    fn icon_colour(&mut self, name: &str, list: bool) -> [u8; 3] {
        let at = self.icon_middle(name, list);
        self.pixel(at)
    }

    /// Settle until `done` holds, for what the watcher and the thumbnail workers report from their threads: between
    /// two looks it settles again while the window waits for a frame, and otherwise waits for a thread to wake the
    /// runtime. [`WAIT`] only guards against a hang.
    #[track_caller]
    fn wait_until(&mut self, mut done: impl FnMut(&mut Browser) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(self) {
            let system = self.driver.window_system_mut();
            let progress = if system.frame_requested(self.window).unwrap() {
                Instant::now() < deadline
            } else {
                let left = deadline.saturating_duration_since(Instant::now());
                !left.is_zero() && system.wait_for_wake(left)
            };
            assert!(progress, "the window never caught up");
            self.settle();
        }
    }

    /// The thumbnails written to the cache.
    fn cached(&self) -> usize {
        std::fs::read_dir(self.home.path().join("cache/thumbnails/normal"))
            .map(|d| d.count())
            .unwrap_or(0)
    }
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 3)
}

/// A picture, a text file, a folder, a file of an unknown type and a link to the text file.
fn mixed(dir: &Path) {
    picture(&dir.join("photo.png"), 40, PICTURE);
    std::fs::write(dir.join("notes.txt"), "notes").unwrap();
    std::fs::create_dir(dir.join("Projects")).unwrap();
    std::fs::write(dir.join("data.unknownkind"), [0u8, 1, 2]).unwrap();
    std::os::unix::fs::symlink(dir.join("notes.txt"), dir.join("link.txt")).unwrap();
}

#[test]
fn items_wear_their_type_s_icon_and_a_picture_its_thumbnail_in_both_views() {
    for list in [false, true] {
        let mut browser = Browser::open(mixed, list);
        browser.wait_until(|b| b.label("photo.png").is_some());
        let view = if list { "list" } else { "icon" };
        assert_eq!(
            browser.icon_colour("notes.txt", list),
            [red("document.text"), 0, 0],
            "{view} view"
        );
        assert_eq!(
            browser.icon_colour("Projects", list),
            [red("folder"), 0, 0],
            "{view} view"
        );
        assert_eq!(
            browser.icon_colour("data.unknownkind", list),
            [red("document.generic"), 0, 0],
            "{view} view"
        );
        browser.wait_until(|b| near(b.icon_colour("photo.png", list), PICTURE));
        assert!(
            browser.cached() >= 1,
            "the thumbnail is in the cache it was given"
        );
    }
}

#[test]
fn a_symbolic_link_wears_the_alias_badge_in_its_bottom_left_corner() {
    let mut browser = Browser::open(mixed, false);
    browser.wait_until(|b| b.label("link.txt").is_some());
    // the badge of a 64 point icon is 32 points square, from 6 to 38 across and 20 to 52 down
    let link = browser.icon("link.txt", false);
    assert_eq!(link.size.width, 64.0);
    let at = |icon: Rect, x: f64, y: f64| Point::new(icon.min_x() + x, icon.min_y() + y);
    assert_eq!(
        browser.pixel(at(link, 56.0, 8.0)),
        [red("document.text"), 0, 0]
    );
    assert_eq!(
        browser.pixel(at(link, 16.0, 44.0)),
        [red("alias.badge"), 0, 0]
    );
    let notes = browser.icon("notes.txt", false);
    assert_eq!(
        browser.pixel(at(notes, 16.0, 44.0)),
        [red("document.text"), 0, 0],
        "no badge"
    );
}

#[test]
fn a_picture_that_changes_shows_its_new_thumbnail() {
    let mut browser = Browser::open(mixed, false);
    browser.wait_until(|b| {
        b.label("photo.png")
            .is_some_and(|_| near(b.icon_colour("photo.png", false), PICTURE))
    });
    let path = browser.documents().join("photo.png");
    picture(&path, 48, CHANGED);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    browser.wait_until(|b| near(b.icon_colour("photo.png", false), CHANGED));
}

#[test]
fn only_the_pictures_in_view_get_thumbnails_until_the_others_scroll_in() {
    const COUNT: usize = 150;
    let mut browser = Browser::open(
        |dir| {
            for i in 0..COUNT {
                picture(&dir.join(format!("p{i:03}.png")), 8, PICTURE);
            }
        },
        false,
    );
    browser.wait_until(|b| b.label("p000.png").is_some());
    browser.wait_until(|b| near(b.icon_colour("p000.png", false), PICTURE));
    // the workers settle on the items in view and near it: each thumbnail made wakes the window, and the end of the
    // work has no event of its own, so they have settled once none wakes it for a while. Reading early can only
    // count fewer thumbnails than the bound below allows.
    let deadline = Instant::now() + WAIT;
    while browser.driver.window_system_mut().wait_for_wake(SETTLED) {
        assert!(Instant::now() < deadline, "the workers never settled");
        browser.settle();
    }
    let last = browser.cached();
    assert!(
        last < COUNT / 2,
        "{last} of {COUNT} thumbnails were made for a window that shows a few rows"
    );
    // End goes to the last item, which scrolls into view and gets its thumbnail
    let first = browser.item("p000.png").unwrap();
    browser.button(first.center(), true);
    browser.button(first.center(), false);
    browser.send(WindowEvent::Key(
        KeyEvent {
            keysym: Keysym(0xff57),
            text: None,
            code: 0,
            pressed: true,
            mods: Mods::NONE,
            repeat: false,
        },
        Timestamp(0),
    ));
    let name = format!("p{:03}.png", COUNT - 1);
    browser.wait_until(|b| b.label(&name).is_some());
    browser.wait_until(|b| near(b.icon_colour(&name, false), PICTURE));
}

#[test]
fn a_dragged_picture_shows_its_thumbnail_and_a_file_its_type_s_icon() {
    let mut browser = Browser::open(mixed, false);
    browser.wait_until(|b| {
        b.label("photo.png")
            .is_some_and(|_| near(b.icon_colour("photo.png", false), PICTURE))
    });
    for (name, colour) in [
        ("photo.png", PICTURE),
        ("notes.txt", [red("document.text"), 0, 0]),
    ] {
        let cell = browser.item(name).unwrap();
        let middle = browser.icon_middle(name, false);
        let before = browser.driver.runtime().window_system().drags().len();
        browser.pointer(PointerEvent::Motion { position: middle });
        browser.button(middle, true);
        browser.pointer(PointerEvent::Motion {
            position: Point::new(middle.x + 12.0, middle.y + 6.0),
        });
        let drags = browser.driver.runtime().window_system().drags();
        let started = drags.get(before).expect("a drag").clone();
        let image = started.request.image.as_ref().expect("an image");
        // the press was on the icon's middle: the hotspot is there in the image, which is drawn at 0.8 opacity
        let (x, y) = (
            (image.hotspot.x * f64::from(image.scale)).round() as u32,
            (image.hotspot.y * f64::from(image.scale)).round() as u32,
        );
        let i = ((y * image.width + x) * 4) as usize;
        let p = &image.pixels[i..i + 4];
        let a = u32::from(p[3]);
        assert!(a > 150, "{name}: the icon shows under the pointer");
        let straight: Vec<u8> = p[..3]
            .iter()
            .map(|&c| ((u32::from(c) * 255 + a / 2) / a).min(255) as u8)
            .collect();
        assert!(
            straight.iter().zip(colour).all(|(s, c)| s.abs_diff(c) <= 4),
            "{name}: {straight:?} is not {colour:?}; cell {cell:?}"
        );
        browser.button(middle, false);
        browser
            .driver
            .window_system_mut()
            .enqueue(PlatformEvent::Drag {
                token: started.token,
                progress: DragProgress::Cancelled,
            })
            .unwrap();
        browser.settle();
    }
}
