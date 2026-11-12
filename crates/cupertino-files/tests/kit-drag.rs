//! Items dragged out of the browser window on CupertinoKit, run headless: the files offered as a URI list, the image of
//! one item or of a stack with its count, where the pointer holds it, and the window after a drag that was cancelled.
//! With `CUPERTINO_DRAG_CAPTURES` naming a directory, the images are also written there as PNGs, light and dark at 1x
//! and 2x, for a look at them.

#![cfg(feature = "app")]

use crate::common;

use std::collections::HashMap;
use std::sync::Arc;

use common::{budget, place};
use cupertino_assets::BUILTIN_FONT;
use cupertino_files::kit::{prepare, Files, Start};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_files::trash::Trash;
use cupertino_kit::core::{AccessibilityNode, NodeId, PlaceholderIcons, Role, RuntimeOptions};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, DragImage, DragProgress, DropAction, Mods, MouseButton, PlatformEvent,
    PointerEvent, Timestamp, WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, StartedDrag, TestDriver};

/// How far the pointer moves after the press: past the distance that starts a drag.
const MOVE: f64 = 12.0;

/// A browser window on `home/Documents` (the folder `Notes`, the files `a.txt` and `b.txt`) in the icon view, on a
/// headless system that starts drags.
struct Browser {
    driver: TestDriver<Files>,
    window: WindowId,
    home: tempfile::TempDir,
}

impl Browser {
    fn new(appearance: Appearance) -> Browser {
        Browser::with(appearance, true)
    }

    /// The window on a system that starts drags if `drags`.
    fn with(appearance: Appearance, drags: bool) -> Browser {
        let home = tempfile::tempdir().unwrap();
        let documents = home.path().join("Documents");
        std::fs::create_dir_all(documents.join("Notes")).unwrap();
        std::fs::write(documents.join("b.txt"), "bb").unwrap();
        std::fs::write(documents.join("a.txt"), "a").unwrap();
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
            .with_trash(Trash::with_home(home.path().join("Trash"))),
        );
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let preferences = ThemePreferences {
            appearance,
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
        let capabilities = Capabilities {
            drag_and_drop: drags,
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
        browser.menu("view-icons");
        browser
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

    fn menu(&mut self, name: &str) {
        self.send(WindowEvent::MenuActivated(common::action(name)));
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

    /// Each item of the icon grid: its name, bounds and whether it is selected.
    fn cells(&self) -> Vec<(String, Rect, bool)> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let nodes: HashMap<NodeId, &AccessibilityNode> = snapshot
            .accessibility
            .iter()
            .map(|(id, node)| (*id, node))
            .collect();
        let grid = nodes
            .values()
            .find(|n| n.role == Role::List && n.name.as_deref() == Some("Documents"))
            .expect("the icon grid");
        grid.children
            .iter()
            .map(|id| nodes[id])
            .filter_map(|n| Some((n.name.clone()?, n.bounds, n.states.selected)))
            .collect()
    }

    fn cell(&self, name: &str) -> Rect {
        let cells = self.cells();
        cells.into_iter().find(|(n, _, _)| n == name).unwrap().1
    }

    fn selected(&self) -> Vec<String> {
        let cells = self.cells();
        cells
            .into_iter()
            .filter(|(_, _, selected)| *selected)
            .map(|(name, _, _)| name)
            .collect()
    }

    /// Press `name` at `at` from its cell's corner and move past the drag's start; the drag started, if one did.
    fn drag(&mut self, name: &str, at: Point) -> Option<StartedDrag> {
        let cell = self.cell(name);
        let press = Point::new(cell.origin.x + at.x, cell.origin.y + at.y);
        let before = self.driver.runtime().window_system().drags().len();
        self.pointer(PointerEvent::Motion { position: press });
        self.button(press, true);
        self.pointer(PointerEvent::Motion {
            position: Point::new(press.x + MOVE, press.y + MOVE / 2.0),
        });
        let drags = self.driver.runtime().window_system().drags();
        drags.get(before).cloned()
    }

    /// End the drag `started` as the compositor does for a drop where nothing takes it.
    fn cancel(&mut self, started: &StartedDrag) {
        self.driver
            .window_system_mut()
            .enqueue(PlatformEvent::Drag {
                token: started.token,
                progress: DragProgress::Cancelled,
            })
            .unwrap();
        self.settle();
    }

    fn click(&mut self, name: &str) {
        let cell = self.cell(name);
        let at = Point::new(
            cell.origin.x + cell.size.width / 2.0,
            cell.origin.y + cell.size.height / 3.0,
        );
        self.pointer(PointerEvent::Enter { position: at });
        self.button(at, true);
        self.button(at, false);
    }

    fn uri(&self, name: &str) -> String {
        let path = self.home.path().join("Documents").join(name);
        format!("file://{}", path.display())
    }
}

fn image(started: &StartedDrag) -> &DragImage {
    started
        .request
        .image
        .as_ref()
        .expect("the drag has an image")
}

/// The largest alpha of the image, and how many of its pixels show anything.
fn coverage(image: &DragImage) -> (u8, usize) {
    let (pixels, _) = image.pixels.as_chunks::<4>();
    let alpha = pixels.iter().map(|p| p[3]).max().unwrap_or(0);
    (alpha, pixels.iter().filter(|p| p[3] > 0).count())
}

/// Writes `image` as `<CUPERTINO_DRAG_CAPTURES>/<name>.png` when that variable is set, and beside it `<name>-over.png`,
/// the image over a plain backdrop of `appearance` as it shows over a window.
fn capture(image: &DragImage, name: &str, appearance: Appearance) {
    let Some(dir) = std::env::var_os("CUPERTINO_DRAG_CAPTURES") else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    let mut pixmap = tiny_skia::Pixmap::new(image.width, image.height).unwrap();
    pixmap.data_mut().copy_from_slice(&image.pixels);
    pixmap.save_png(dir.join(format!("{name}.png"))).unwrap();
    let mut over = tiny_skia::Pixmap::new(image.width, image.height).unwrap();
    over.fill(match appearance {
        Appearance::Light => tiny_skia::Color::from_rgba8(236, 236, 236, 255),
        _ => tiny_skia::Color::from_rgba8(40, 40, 44, 255),
    });
    over.draw_pixmap(
        0,
        0,
        pixmap.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
    over.save_png(dir.join(format!("{name}-over.png"))).unwrap();
    eprintln!("wrote {name} in {}", dir.display());
}

/// One item drags as its file's URI, copy or move, with an image of its cell held where the press grabbed it, drawn at
/// the design's opacity at the window's scale.
#[test]
fn one_item_drags_as_its_uri_with_its_icon_and_name_where_the_press_grabbed_it() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        for scale in [1u32, 2] {
            let mut browser = Browser::new(appearance);
            browser.send(WindowEvent::Scale(f64::from(scale)));
            let cell = browser.cell("a.txt");
            let grab = Point::new(cell.size.width / 2.0 - 7.0, 20.0);
            let started = browser.drag("a.txt", grab).expect("a drag");
            assert_eq!(started.window, browser.window);
            let data = &started.request.data;
            assert_eq!(data.len(), 1);
            assert_eq!(data[0].mime, "text/uri-list");
            let uri = format!("{}\r\n", browser.uri("a.txt"));
            assert_eq!(String::from_utf8(data[0].bytes.clone()).unwrap(), uri);
            assert_eq!(
                started.request.actions,
                [DropAction::Copy, DropAction::Move].into()
            );
            let image = image(&started);
            assert_eq!(image.scale, scale);
            assert_eq!(
                (image.width, image.height),
                (
                    cell.size.width.ceil() as u32 * scale,
                    cell.size.height.ceil() as u32 * scale
                ),
                "the image is the cell"
            );
            assert_eq!(
                image.hotspot, grab,
                "the pointer holds the cell where it pressed it"
            );
            let (alpha, shown) = coverage(image);
            assert!(shown > 0, "the image shows the item");
            assert!(
                alpha <= 205,
                "drawn at 0.8 opacity: the most opaque pixel is {alpha}"
            );
            let name = format!(
                "files-drag-one-{}@{scale}x",
                if appearance == Appearance::Light {
                    "light"
                } else {
                    "dark"
                }
            );
            capture(image, &name, appearance);
        }
    }
}

/// Several selected items drag together from a press on one of them: the pressed one first, under a stack and its
/// count, the pointer still where the press grabbed its cell.
#[test]
fn a_selection_drags_together_as_a_stack_with_its_count_from_the_pressed_item() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        for scale in [1u32, 2] {
            let mut browser = Browser::new(appearance);
            browser.send(WindowEvent::Scale(f64::from(scale)));
            browser.menu("select-all");
            assert_eq!(browser.selected().len(), 3);
            let cell = browser.cell("b.txt");
            let grab = Point::new(cell.size.width / 2.0, 24.0);
            let started = browser.drag("b.txt", grab).expect("a drag");
            let uris = String::from_utf8(started.request.data[0].bytes.clone()).unwrap();
            let uris: Vec<&str> = uris.split_terminator("\r\n").collect();
            let mut expected = vec![browser.uri("b.txt")];
            for (name, _, _) in browser.cells() {
                if name != "b.txt" {
                    expected.push(browser.uri(&name));
                }
            }
            assert_eq!(
                uris, expected,
                "the pressed item first, then the rest in order"
            );
            assert_eq!(browser.selected().len(), 3, "the press kept the selection");
            let image = image(&started);
            let (width, height) = (
                f64::from(image.width) / f64::from(scale),
                f64::from(image.height) / f64::from(scale),
            );
            let room = width - cell.size.width.ceil();
            assert!(
                room > 0.0,
                "the stack and the count take room beside the cell"
            );
            assert_eq!(
                height - cell.size.height.ceil(),
                room,
                "and as much above it"
            );
            assert_eq!(
                image.hotspot,
                Point::new(grab.x, grab.y + room),
                "the pointer holds the pressed cell where it pressed it"
            );
            let (alpha, shown) = coverage(image);
            assert!(shown > 0);
            assert!(alpha <= 205, "drawn at 0.8 opacity: {alpha}");
            let name = format!(
                "files-drag-three-{}@{scale}x",
                if appearance == Appearance::Light {
                    "light"
                } else {
                    "dark"
                }
            );
            capture(image, &name, appearance);
        }
    }
}

/// A plain click on one item of a selection, without moving, selects it alone; a press that does not move far enough
/// starts no drag.
#[test]
fn a_click_on_a_selected_item_without_a_drag_selects_it_alone() {
    let mut browser = Browser::new(Appearance::Light);
    browser.menu("select-all");
    let cell = browser.cell("Notes");
    let at = Point::new(cell.origin.x + cell.size.width / 2.0, cell.origin.y + 20.0);
    browser.button(at, true);
    browser.pointer(PointerEvent::Motion {
        position: Point::new(at.x + 2.0, at.y + 1.0),
    });
    assert!(browser.driver.runtime().window_system().drags().is_empty());
    assert_eq!(browser.selected().len(), 3);
    browser.button(at, false);
    assert_eq!(browser.selected(), ["Notes"]);
}

/// A plain click on the only selected item puts the anchor back on it, as a click on any item does: a Ctrl-click that
/// deselected another item had left the anchor there, and the next Shift-click extends from the clicked item.
#[test]
fn a_click_on_the_only_selected_item_moves_the_anchor_to_it() {
    let mut browser = Browser::new(Appearance::Light);
    let names: Vec<String> = browser.cells().into_iter().map(|(n, _, _)| n).collect();
    let [first, middle, last] = &names[..] else {
        panic!("three items: {names:?}");
    };
    let centre = |browser: &Browser, name: &str| {
        let cell = browser.cell(name);
        Point::new(
            cell.origin.x + cell.size.width / 2.0,
            cell.origin.y + cell.size.height / 3.0,
        )
    };
    let click = |browser: &mut Browser, name: &str, mods: Mods| {
        let at = centre(browser, name);
        for pressed in [true, false] {
            browser.pointer(PointerEvent::Button {
                position: at,
                button: MouseButton::Left,
                pressed,
                serial: 1,
                mods,
            });
        }
    };
    click(&mut browser, first, Mods::NONE);
    click(&mut browser, last, Mods::PRIMARY);
    click(&mut browser, last, Mods::PRIMARY);
    assert_eq!(browser.selected(), std::slice::from_ref(first));
    click(&mut browser, first, Mods::NONE);
    click(&mut browser, middle, Mods::SHIFT);
    assert_eq!(
        browser.selected(),
        [first.clone(), middle.clone()],
        "the Shift-click extends from the clicked item, not the one the Ctrl-click left"
    );
}

/// A drag the compositor cancels (a drop nothing takes, or one the target refused) leaves the files and the selection
/// as they were, and the window takes clicks and starts drags again as before.
#[test]
fn after_a_cancelled_drag_the_window_selects_and_drags_as_before() {
    let mut browser = Browser::new(Appearance::Light);
    let started = browser.drag("a.txt", Point::new(30.0, 20.0)).unwrap();
    browser.cancel(&started);
    assert!(browser.home.path().join("Documents/a.txt").exists());
    assert_eq!(browser.selected(), ["a.txt"]);
    browser.click("b.txt");
    assert_eq!(browser.selected(), ["b.txt"]);
    let again = browser
        .drag("Notes", Point::new(30.0, 20.0))
        .expect("a new drag");
    assert_ne!(again.token, started.token);
    assert_eq!(browser.selected(), ["Notes"]);
}

/// On a system that starts no drags, as on a compositor without drag and drop, the drag is cancelled at once and the
/// window goes on: the press selected the item, the release ends the press, the next click selects as before.
#[test]
fn a_drag_the_system_refuses_leaves_the_window_as_it_was() {
    let mut browser = Browser::with(Appearance::Light, false);
    assert!(browser.drag("a.txt", Point::new(30.0, 20.0)).is_none());
    let cell = browser.cell("a.txt");
    browser.button(
        Point::new(cell.origin.x + 30.0, cell.origin.y + 20.0),
        false,
    );
    assert_eq!(browser.selected(), ["a.txt"]);
    browser.click("b.txt");
    assert_eq!(browser.selected(), ["b.txt"]);
}

/// The bounds of the list's text `name`.
fn text_bounds(browser: &Browser, name: &str) -> Rect {
    let snapshot = browser.driver.runtime().snapshot(browser.window).unwrap();
    snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.role == Role::StaticText && n.name.as_deref() == Some(name))
        .map(|(_, n)| n.bounds)
        .unwrap_or_else(|| panic!("no text {name:?}"))
}

/// Rows of the list drag as their URIs, the pressed row first, with an image of the pressed row's name cell held where
/// the press grabbed it and the count of the rows.
#[test]
fn rows_of_the_list_drag_together_with_the_pressed_row_first() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        for scale in [1u32, 2] {
            let mut browser = Browser::new(appearance);
            browser.send(WindowEvent::Scale(f64::from(scale)));
            browser.menu("view-list");
            let a = text_bounds(&browser, "a.txt");
            let b = text_bounds(&browser, "b.txt");
            // on the name's text, which the cell's text spans the start of
            let center = |r: Rect| Point::new(r.min_x() + 6.0, r.min_y() + r.height() / 2.0);
            browser.button(center(a), true);
            browser.button(center(a), false);
            for pressed in [true, false] {
                browser.pointer(PointerEvent::Button {
                    position: center(b),
                    button: MouseButton::Left,
                    pressed,
                    serial: 1,
                    mods: Mods::SHIFT,
                });
            }
            let press = center(b);
            let before = browser.driver.runtime().window_system().drags().len();
            browser.pointer(PointerEvent::Motion { position: press });
            browser.button(press, true);
            browser.pointer(PointerEvent::Motion {
                position: Point::new(press.x + MOVE, press.y + MOVE / 2.0),
            });
            let drags = browser.driver.runtime().window_system().drags();
            let started = drags.get(before).cloned().expect("a drag");
            let uris = format!("{}\r\n{}\r\n", browser.uri("b.txt"), browser.uri("a.txt"));
            assert_eq!(
                String::from_utf8(started.request.data[0].bytes.clone()).unwrap(),
                uris,
                "both rows, the pressed one first"
            );
            let image = image(&started);
            assert_eq!(image.scale, scale);
            assert!(
                image.height < image.width / 4,
                "a row: {}×{}",
                image.width,
                image.height
            );
            assert!(image.hotspot.x > 0.0 && image.hotspot.y > 0.0);
            let (alpha, shown) = coverage(image);
            assert!(shown > 0 && alpha <= 205, "drawn at 0.8 opacity");
            let name = format!(
                "files-drag-rows-{}@{scale}x",
                if appearance == Appearance::Light {
                    "light"
                } else {
                    "dark"
                }
            );
            capture(image, &name, appearance);
        }
    }
}
