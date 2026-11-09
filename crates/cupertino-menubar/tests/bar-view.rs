//! The bar and an open menu as CupertinoKit draws them, compared with `tests/goldens`: `<scene>.png` is the window's
//! frame at 1x and `<scene>.txt` its focus and accessibility tree. Pixels may differ by at most 2/255 per channel and
//! are compared on x86_64 only, as the renderer's own baselines are. `CUPERTINO_UPDATE_GOLDENS=1` re-records every
//! scene it runs and then fails, so the variable can never pass a run.
#![cfg(feature = "app")]

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, Key, PlaceholderIcons, RuntimeOptions, Window, WindowKey,
};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_menubar::controller::{Effect, MenuController};
use cupertino_menubar::feed::Shared;
use cupertino_menubar::geom::Rect;
use cupertino_menubar::item::{ItemId, MenuItem, Shortcut, Toggle};
use cupertino_menubar::layout::{BarContent, SlotKey};
use cupertino_menubar::tone::BarTone;
use cupertino_menubar::view::{self, BarFont, BarId, BarPointer, BarScene, Input, KitMeasure};
use cupertino_theme::Theme;
use cupertino_ui_test::{AssistiveTechnology, Inspector, ManualClock, TestDriver};
use tiny_skia::Pixmap;

const BAR: BarId = BarId(1);
const OUTPUT: Rect = Rect::new(0.0, 0.0, 1024.0, 600.0);
const FILE: i32 = 1;
const NEW_FOLDER: i32 = 11;

fn theme() -> Theme {
    Theme::from_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme")).unwrap()
}

fn text() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter Variable"))
}

fn content() -> BarContent {
    BarContent {
        app_name: "Files".into(),
        system_menu: vec![MenuItem::new(200, "About This Computer")],
        app_menu: vec![
            MenuItem::new(100, "About Files"),
            MenuItem::new(101, "Quit Files"),
        ],
        menus: vec![
            MenuItem::submenu(
                FILE,
                "File",
                vec![
                    MenuItem::new(10, "New Window")
                        .with_shortcut(Shortcut::from_tokens(&["Control", "N"]).unwrap()),
                    MenuItem::new(NEW_FOLDER, "New Folder"),
                    MenuItem::separator(12),
                    MenuItem::new(13, "Close").disabled(),
                    MenuItem::new(14, "Show Hidden").with_toggle(Toggle::Check(true)),
                ],
            ),
            MenuItem::submenu(2, "Edit", vec![MenuItem::new(20, "Undo")]),
        ],
        status: vec![],
        clock: "Tue 10:42".into(),
        control_panel: true,
    }
}

/// One bar on a 1024-point output; it keeps what the controller asked for.
struct Bar;

struct Model {
    controller: MenuController,
    shared: Shared,
    font: BarFont,
    effects: Vec<Effect>,
    /// The scene's solid strip, spread under the bar and tone, from [`LOOK`].
    solid: bool,
    spread: Vec<u16>,
    tone: BarTone,
}

/// What the next bar started shows: whether its strip is solid, the spread under it, whether the theme's Reduce
/// Transparency is on, and the tone of its text.
#[derive(Clone, Default)]
struct Look {
    solid: bool,
    spread: Vec<u16>,
    theme_reduce_transparency: bool,
    tone: BarTone,
}

thread_local! {
    static LOOK: std::cell::RefCell<Look> = std::cell::RefCell::new(Look::default());
}

impl Application for Bar {
    type Model = Model;
    type Message = Input;

    fn init() -> (Model, Vec<Command<Input>>) {
        let font = BarFont::new(theme().tokens.typography());
        let mut controller =
            MenuController::new(OUTPUT, Box::new(KitMeasure { text: text(), font }));
        controller.set_content(content());
        let model = Model {
            controller,
            shared: Shared::default(),
            font,
            effects: Vec::new(),
            solid: LOOK.with_borrow(|l| l.solid),
            spread: LOOK.with_borrow(|l| l.spread.clone()),
            tone: LOOK.with_borrow(|l| l.tone),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, input: Input) -> Vec<Command<Input>> {
        let Input::Pointer { pointer, .. } = input else {
            return Vec::new();
        };
        let c = &mut model.controller;
        let outcome = match pointer {
            BarPointer::Move(p) => c.pointer_move(p, 0.0),
            BarPointer::Press(p) => c.pointer_press(p, 0.0),
            BarPointer::Release(p) => c.pointer_release(p, 0.0),
            BarPointer::Click(p) => {
                c.pointer_press(p, 0.0);
                c.pointer_release(p, 0.0)
            }
        };
        model.effects.extend(outcome.effects);
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Input>> {
        let bar = model.controller.layout().bar;
        view::windows(
            &BarScene {
                bar: BAR,
                controller: &model.controller,
                shared: &model.shared,
                font: model.font,
                tone: model.tone,
                solid: model.solid,
                spread: &model.spread,
                size: Size::new(bar.w, bar.h),
            },
            |input| input,
        )
    }
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

struct Run {
    driver: TestDriver<Bar>,
}

impl Run {
    fn start() -> Self {
        Self::start_with(None)
    }

    /// A bar whose windows a screen reader follows when `reader` is given.
    fn start_with(reader: Option<&AssistiveTechnology>) -> Self {
        let preferences = ThemePreferences {
            appearance: Appearance::Light,
            reduce_transparency: LOOK.with_borrow(|l| l.theme_reduce_transparency),
            reduce_motion: false,
        };
        let tokens = ThemeTokens::cupertino(&theme(), preferences).unwrap();
        let text = text();
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(tokens),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 400,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 21,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: reader.map(AssistiveTechnology::adapter),
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let capabilities = Capabilities {
            app_effects: true,
            ..Capabilities::default()
        };
        let mut run = Self {
            driver: TestDriver::new(capabilities, options, ManualClock::new(Timestamp(0))),
        };
        run.settle();
        run
    }

    fn window(&self, key: &str) -> Option<WindowId> {
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .find(|(k, _)| *k == WindowKey(Key::from(key.to_owned())))
            .map(|(_, id)| id)
    }

    /// Runs until every window rests, then checks that a screen reader could read every window open.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Menu bar");
    }

    /// Runs until idle, configuring each window it opens and drawing each frame asked for.
    fn settle_frames(&mut self) {
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        let mut configured: Vec<WindowId> = Vec::new();
        for _ in 0..16 {
            self.driver.run_until_idle(budget()).unwrap();
            let mut idle = true;
            for (_, window) in self.driver.runtime().windows() {
                let system = self.driver.window_system_mut();
                if !configured.contains(&window) {
                    system.configure(window, None, active).unwrap();
                    configured.push(window);
                    idle = false;
                }
                if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, Timestamp(0)).unwrap();
                    idle = false;
                }
            }
            if idle {
                return;
            }
        }
        panic!("the bar keeps asking for frames");
    }

    fn model(&self) -> &Model {
        self.driver.runtime().model()
    }

    fn pointer(&mut self, key: &str, event: PointerEvent) {
        let window = self
            .window(key)
            .unwrap_or_else(|| panic!("no window {key}"));
        self.driver
            .window_system_mut()
            .enqueue_window(window, WindowEvent::Pointer(event, Timestamp(0)))
            .unwrap();
        self.settle();
    }

    fn button(&mut self, key: &str, position: Point, pressed: bool) {
        self.pointer(
            key,
            PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            },
        );
    }

    /// The centre of the title of the menu `id`, in the bar window's coordinates.
    fn title(&self, id: i32) -> Point {
        let r = self
            .model()
            .controller
            .layout()
            .slot(&SlotKey::Menu(ItemId(id)))
            .unwrap()
            .rect;
        Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    /// The centre of the row `id` of the first open menu, in that menu window's coordinates.
    fn row(&self, id: i32) -> Point {
        let layout = &self.model().controller.levels()[0].layout;
        let origin = cupertino_menubar::popups::snap(layout.rect);
        let r = layout.rows[layout.index_of(ItemId(id)).unwrap()].rect;
        Point::new(r.x + r.w / 2.0 - origin.x, r.y + r.h / 2.0 - origin.y)
    }

    /// The node of window `key` that a screen reader reads as `name`.
    fn node(&self, key: &str, name: &str) -> cupertino_kit::core::NodeId {
        let window = self
            .window(key)
            .unwrap_or_else(|| panic!("no window {key}"));
        let snapshot = self.driver.runtime().snapshot(window).unwrap();
        snapshot
            .accessibility
            .iter()
            .find(|(_, n)| n.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{key} has nothing named {name}"))
            .0
    }

    /// Asks for `name` in window `key` to be clicked, as `reader` does, and runs until every window rests.
    fn ask_click(&mut self, reader: &AssistiveTechnology, key: &str, name: &str) {
        let target = self.node(key, name);
        self.driver
            .ask(reader, target, cupertino_kit::core::Action::Click, None);
        self.settle();
    }

    fn open_file(&mut self) {
        let title = self.title(FILE);
        self.button("bar-1", title, true);
        self.button("bar-1", title, false);
    }

    /// The window's last frame and the focus and accessibility lines of its dump.
    fn capture(&mut self, key: &str) -> (Pixmap, String) {
        let window = self.window(key).unwrap();
        let snapshot = self.driver.runtime().snapshot(window).unwrap();
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
            .frames(window)
            .unwrap()
            .last()
            .unwrap()
            .clone();
        let mut pixmap = Pixmap::new(frame.width, frame.height).unwrap();
        pixmap.data_mut().copy_from_slice(&frame.pixels);
        (pixmap, text)
    }
}

fn check(scene: &str, (actual, text): (Pixmap, String)) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens");
    let (png, txt) = (format!("{dir}/{scene}.png"), format!("{dir}/{scene}.txt"));
    if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::create_dir_all(dir).unwrap();
        actual.save_png(&png).unwrap();
        std::fs::write(&txt, &text).unwrap();
        panic!(
            "{scene}: re-recorded; review the diff and run again without CUPERTINO_UPDATE_GOLDENS"
        );
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

#[test]
fn the_bar_matches_its_golden() {
    let mut run = Run::start();
    assert_eq!(run.window("menu-1-0"), None, "no menu is open");
    check("bar", run.capture("bar-1"));
}

#[test]
fn the_bar_with_the_file_menu_open_matches_its_golden() {
    let mut run = Run::start();
    run.open_file();
    check("bar-file-open", run.capture("bar-1"));
}

#[test]
fn the_open_file_menu_matches_its_golden() {
    let mut run = Run::start();
    run.open_file();
    check("file-menu", run.capture("menu-1-0"));
}

#[test]
fn a_press_on_a_title_opens_its_menu_in_a_popup_of_the_bar() {
    let mut run = Run::start();
    run.open_file();
    assert!(run.model().controller.is_open());
    assert!(run.window("menu-1-0").is_some(), "the File menu's popup");
}

#[test]
fn a_click_on_a_row_chooses_its_item() {
    let mut run = Run::start();
    run.open_file();
    let row = run.row(NEW_FOLDER);
    run.pointer("menu-1-0", PointerEvent::Motion { position: row });
    run.button("menu-1-0", row, true);
    run.button("menu-1-0", row, false);
    assert!(
        run.model().effects.iter().any(|e| matches!(
            e,
            Effect::Activate { item, .. } if *item == ItemId(NEW_FOLDER)
        )),
        "New Folder is chosen: {:?}",
        run.model().effects
    );
    assert_eq!(run.window("menu-1-0"), None, "choosing closes the menu");
}

#[test]
fn a_screen_reader_opens_a_menu_by_its_title_and_chooses_a_row() {
    let reader = AssistiveTechnology::new();
    let mut run = Run::start_with(Some(&reader));
    run.ask_click(&reader, "bar-1", "File");
    assert!(run.model().controller.is_open(), "File opens");
    let menu = run.window("menu-1-0").expect("the File menu's popup");
    assert!(reader.tree(menu).is_some(), "the reader follows the menu");
    run.ask_click(&reader, "menu-1-0", "New Folder");
    assert!(
        run.model().effects.iter().any(|e| matches!(
            e,
            Effect::Activate { item, .. } if *item == ItemId(NEW_FOLDER)
        )),
        "New Folder is chosen: {:?}",
        run.model().effects
    );
    assert_eq!(run.window("menu-1-0"), None, "choosing closes the menu");
}

/// How many of the bar's items draw a halo under their text and symbols, for `look`.
fn halos(look: Look) -> usize {
    LOOK.set(look);
    let run = Run::start();
    LOOK.set(Look::default());
    let window = run.window("bar-1").unwrap();
    let snapshot = run.driver.runtime().snapshot(window).unwrap();
    Inspector::default()
        .dump(&snapshot)
        .lines()
        .filter(|l| l.contains("DropShadow"))
        .count()
}

/// The session's Reduce Transparency is the one source of both the bar's solid strip and its items' halos: the strip
/// shows exactly when it is on (`tone::solid_bar`), and a bar drawn with it draws no halo over the busiest wallpaper,
/// even with the theme's own Reduce Transparency off; with the session's off, the theme's does not take the halos
/// away.
#[test]
fn reduce_transparency_gives_the_solid_strip_and_no_halo_from_one_source() {
    use cupertino_defaults::accessibility::DisplayOptions;
    use cupertino_menubar::tone::solid_bar;
    let busy = vec![1000; 64];
    let reduced = DisplayOptions {
        reduce_transparency: true,
        ..DisplayOptions::default()
    };
    assert!(solid_bar(reduced), "the strip shows");
    assert!(!solid_bar(DisplayOptions::default()), "the strip is hidden");
    let items = halos(Look {
        solid: solid_bar(DisplayOptions::default()),
        spread: busy.clone(),
        theme_reduce_transparency: false,
        ..Look::default()
    });
    assert!(
        items >= 4,
        "each item over a busy wallpaper has a halo: {items}"
    );
    assert_eq!(
        halos(Look {
            solid: solid_bar(reduced),
            spread: busy.clone(),
            theme_reduce_transparency: false,
            ..Look::default()
        }),
        0,
        "a solid bar draws no halo"
    );
    assert_eq!(
        halos(Look {
            solid: solid_bar(DisplayOptions::default()),
            spread: busy,
            theme_reduce_transparency: true,
            ..Look::default()
        }),
        items,
        "the theme's own option is not a second source"
    );
}

/// Over a calm wallpaper no item draws a halo; over a wallpaper busy under one part of the bar only, only the items
/// over that part do; with no measure every item keeps the halo it drew before the spread was measured.
#[test]
fn only_the_items_over_a_busy_part_of_the_wallpaper_draw_a_halo() {
    let all = halos(Look::default());
    assert!(all >= 4, "with no measure every item has a halo: {all}");
    assert_eq!(
        halos(Look {
            spread: vec![30; 64],
            ..Look::default()
        }),
        0,
        "calm"
    );
    // busy under the left eighth of the bar only, where the application's menus are
    let mut left = vec![0; 64];
    left[..8].fill(400);
    let some = halos(Look {
        spread: left,
        ..Look::default()
    });
    assert!(some > 0 && some < all, "{some} of {all}");
}

/// The bar's frame at 1x for `look`, with the File menu open when `open`, and the File title's rectangle.
fn frame(look: Look, open: bool) -> (Pixmap, Rect) {
    LOOK.set(look);
    let mut run = Run::start();
    LOOK.set(Look::default());
    if open {
        run.open_file();
    }
    let file = run
        .model()
        .controller
        .layout()
        .slot(&SlotKey::Menu(ItemId(FILE)))
        .unwrap()
        .rect;
    (run.capture("bar-1").0, file)
}

/// The linear luminance of the premultiplied pixel `px` laid over the opaque sRGB `backdrop`.
fn over(px: &[u8; 4], backdrop: [u8; 3]) -> f64 {
    let a = f64::from(px[3]) / 255.0;
    let channel = |i: usize| {
        let v = f64::from(px[i]) / 255.0 + f64::from(backdrop[i]) / 255.0 * (1.0 - a);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2)
}

fn contrast(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The text of every item over the busiest wallpaper reads against what lies right around its strokes, wherever the
/// wallpaper is as light as light text or as dark as dark text: within a point of the strokes the halo's core makes the
/// pixels 4.5:1 or more from the text on average, and 3:1 or more at nine in ten of them. The same bar over a calm
/// wallpaper draws no halo, so there the wallpaper right beside the strokes is left as it is.
#[test]
fn over_the_busiest_wallpaper_every_item_reads_next_to_its_strokes() {
    // light text over white and over a bright yellow, dark text over black and over a deep blue
    for (tone, backdrop) in [
        (BarTone::LightText, [255, 255, 255]),
        (BarTone::LightText, [242, 202, 68]),
        (BarTone::DarkText, [0, 0, 0]),
        (BarTone::DarkText, [31, 69, 240]),
    ] {
        let look = |spread| Look {
            spread: vec![spread; 64],
            tone,
            ..Look::default()
        };
        let (calm, _) = frame(look(0), false);
        let (busy, _) = frame(look(1000), false);
        let (w, h) = (calm.width() as usize, calm.height() as usize);
        let calm_px = calm.data().as_chunks::<4>().0;
        let busy_px = busy.data().as_chunks::<4>().0;
        // the text's strokes, where the calm bar is (almost) opaque, and the pixels within a point of them it leaves
        let stroke = |x: usize, y: usize| calm_px[y * w + x][3] >= 200;
        let (mut text, mut ring) = (Vec::new(), Vec::new());
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                if stroke(x, y) {
                    text.push(over(&busy_px[i], backdrop));
                } else if calm_px[i][3] == 0
                    && (y - 1..=y + 1).any(|yy| (x - 1..=x + 1).any(|xx| stroke(xx, yy)))
                {
                    ring.push(over(&busy_px[i], backdrop));
                    assert_eq!(
                        over(&calm_px[i], backdrop),
                        over(&[0, 0, 0, 0], backdrop),
                        "{tone:?}: a calm bar leaves the wallpaper beside the strokes"
                    );
                }
            }
        }
        assert!(
            text.len() > 50 && ring.len() > 200,
            "{tone:?}: {} {}",
            text.len(),
            ring.len()
        );
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let ink = mean(&text);
        let average = contrast(ink, mean(&ring));
        let mut each: Vec<f64> = ring.iter().map(|&l| contrast(ink, l)).collect();
        each.sort_by(f64::total_cmp);
        let tenth = each[each.len() / 10];
        eprintln!(
            "{tone:?} over {backdrop:?}: {average:.2}:1 on average, {tenth:.2}:1 at the tenth"
        );
        assert!(
            average >= 4.5,
            "{tone:?} over {backdrop:?}: {average:.2}:1 beside the strokes"
        );
        assert!(
            tenth >= 3.0,
            "{tone:?} over {backdrop:?}: {tenth:.2}:1 at the tenth of the pixels beside the strokes"
        );
    }
}

/// The open title's highlight shows as a shape of its own over the busiest wallpaper, even where the picture is as
/// light as the highlight of light text (or as dark as that of dark text): at its ends, clear of the text, it differs
/// from the wallpaper by 1.5:1 or more. Over a calm wallpaper it is the plain highlight.
#[test]
fn the_open_title_shows_over_the_busiest_wallpaper() {
    for (tone, backdrop) in [
        (BarTone::LightText, [255, 255, 255]),
        (BarTone::DarkText, [0, 0, 0]),
    ] {
        let look = |spread| Look {
            spread: vec![spread; 64],
            tone,
            ..Look::default()
        };
        let end = |pixmap: &Pixmap, file: Rect| {
            let w = pixmap.width() as usize;
            let px = pixmap.data().as_chunks::<4>().0;
            // the highlight's left end, a point in from its edge, halfway down
            let (x, y) = ((file.x + 2.0) as usize, (file.y + file.h / 2.0) as usize);
            over(&px[y * w + x], backdrop)
        };
        let (busy, file) = frame(look(1000), true);
        let (calm, _) = frame(look(0), true);
        let wallpaper = over(&[0, 0, 0, 0], backdrop);
        let busy = contrast(end(&busy, file), wallpaper);
        let calm = contrast(end(&calm, file), wallpaper);
        eprintln!("{tone:?}: the highlight's end at {busy:.2}:1 (calm {calm:.2}:1)");
        assert!(busy >= 1.5, "{tone:?}: {busy:.2}:1");
        assert!(
            busy > calm,
            "{tone:?}: the backing shows over busy wallpaper only"
        );
    }
}
