//! The bar and an open menu as CupertinoKit draws them, compared with `tests/goldens`: `<scene>.png` is the window's
//! frame at 1x and `<scene>.txt` its focus and accessibility tree. Pixels may differ by at most 2/255 per channel and
//! are compared on x86_64 only, as the renderer's own baselines are. `CUPERTINO_UPDATE_GOLDENS=1` re-records every
//! scene it runs and then fails, so the variable can never pass a run.
#![cfg(feature = "app")]

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Clock, Command, Key, PlaceholderIcons, RuntimeOptions, Window, WindowKey,
};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    semantics, Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp,
    WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_menubar::controller::{Effect, MenuController};
use cupertino_menubar::feed::Shared;
use cupertino_menubar::geom::Rect;
use cupertino_menubar::item::{ItemId, MenuItem, Shortcut, Toggle};
use cupertino_menubar::layout::{BarContent, BarMetrics, SlotKey};
use cupertino_menubar::tone::{BarTone, SlotTones};
use cupertino_menubar::view::{self, BarFont, BarId, BarPointer, BarScene, Input, KitMeasure};
use cupertino_theme::Theme;
use cupertino_ui_test::{AssistiveTechnology, Inspector, ManualClock, TestDriver};
use tiny_skia::Pixmap;

const BAR: BarId = BarId(1);
/// One frame at 60 Hz, in nanoseconds.
const FRAME_NS: u64 = 16_666_667;
const OUTPUT: Rect = Rect::new(0.0, 0.0, 1024.0, 600.0);
const FILE: i32 = 1;
const NEW_FOLDER: i32 = 11;
const EDIT: i32 = 2;
const FIND: i32 = 21;
/// Key codes the bar reads as menu keys.
const ESCAPE: u32 = 1;
const DOWN: u32 = 108;
const RIGHT: u32 = 106;

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
            MenuItem::submenu(
                EDIT,
                "Edit",
                vec![
                    MenuItem::new(20, "Undo"),
                    MenuItem::submenu(FIND, "Find", vec![MenuItem::new(22, "Find Next")]),
                ],
            ),
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
    /// The scene's solid strip and tone, from [`LOOK`].
    solid: bool,
    tone: BarTone,
    /// The tone of each status item over the wallpaper under it, from [`LOOK`].
    slot_tones: SlotTones,
    /// The items whose panel another process shows open on this bar, from [`PANELS`].
    open_panels: Vec<SlotKey>,
    /// The icons of menu rows, from the built-in assets.
    icons: std::rc::Rc<view::RowIcons>,
}

/// What the next bar started shows: whether its strip is solid, whether the theme's Reduce Transparency is on, and the
/// tone of its text.
#[derive(Clone, Default)]
struct Look {
    solid: bool,
    theme_reduce_transparency: bool,
    tone: BarTone,
    slot_tones: SlotTones,
}

thread_local! {
    static LOOK: std::cell::RefCell<Look> = std::cell::RefCell::new(Look::default());
    /// The items whose panel is open on the next bar started.
    static PANELS: std::cell::RefCell<Vec<SlotKey>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Application for Bar {
    type Model = Model;
    type Message = Input;

    fn init() -> (Model, Vec<Command<Input>>) {
        let font = BarFont::new(theme().tokens.typography());
        let mut controller =
            MenuController::new(OUTPUT, Box::new(KitMeasure { text: text(), font }));
        // the bar here works at one time, so a choice is made at once, without the flash a chosen row gives first
        controller.cfg.flash_choice = false;
        controller.set_content(content());
        let model = Model {
            controller,
            shared: Shared::default(),
            font,
            effects: Vec::new(),
            solid: LOOK.with_borrow(|l| l.solid),
            tone: LOOK.with_borrow(|l| l.tone),
            slot_tones: LOOK.with_borrow(|l| l.slot_tones.clone()),
            open_panels: PANELS.with_borrow(Clone::clone),
            icons: std::rc::Rc::new(view::RowIcons::new(
                cupertino_icons::ThemeIcons::new(&cupertino_assets::Assets::builtin()),
                cupertino_icons::SystemIcons::new(&cupertino_assets::Assets::builtin()),
            )),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, input: Input) -> Vec<Command<Input>> {
        let c = &mut model.controller;
        let pointer = match input {
            Input::Pointer { pointer, .. } => pointer,
            // keys and the end of the grab reach the controller as they do in the menu bar
            Input::Key { key, .. } if key.pressed => {
                if let Some(key) = cupertino_menubar::routing::menu_key(key.code) {
                    let outcome = c.key(key, 0.0);
                    model.effects.extend(outcome.effects);
                }
                return Vec::new();
            }
            Input::Dismissed(_) => {
                let outcome = c.close();
                model.effects.extend(outcome.effects);
                return Vec::new();
            }
            Input::Key { .. } | Input::Closed(_) | Input::Prompt { .. } => return Vec::new(),
        };
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
                slot_tones: &model.slot_tones,
                solid: model.solid,
                size: Size::new(bar.w, bar.h),
                open_panels: &model.open_panels,
                icons: &model.icons,
                prompt: None,
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
    clock: ManualClock,
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
        let clock = ManualClock::new(Timestamp(0));
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
            clock: Arc::new(clock.clone()),
            on_window_error: None,
        };
        let capabilities = Capabilities {
            app_effects: true,
            ..Capabilities::default()
        };
        let mut run = Self {
            driver: TestDriver::new(capabilities, options, clock.clone()),
            clock,
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

    /// Runs until idle, configuring each window it opens and drawing each frame asked for; once it has stepped a few
    /// rounds at one time, the clock moves a frame a round so that transitions finish.
    fn settle_frames(&mut self) {
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        let mut configured: Vec<WindowId> = Vec::new();
        for round in 0..16 + 120 {
            if round >= 16 {
                self.clock.advance_by(FRAME_NS);
            }
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
                    system.frame_ready(window, Clock::now(&self.clock)).unwrap();
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

    /// A bar watched by desktop control, before anything gave it an input event.
    fn controlled() -> Self {
        let mut run = Self::start();
        let system = run.driver.window_system_mut();
        system.set_no_input_yet();
        system.observe_semantics();
        run.settle();
        run
    }

    /// Desktop control clicks what window `key` publishes as `name`, with a grab serial when `grab` (a compositor that
    /// hands one out) or without (an older one); how the click ended.
    fn perform_click(&mut self, key: &str, name: &str, grab: bool) -> Option<semantics::Outcome> {
        let window = self
            .window(key)
            .unwrap_or_else(|| panic!("no window {key}"));
        let system = self.driver.window_system_mut();
        let node = system
            .published(window)
            .and_then(|s| s.nodes.iter().find(|n| n.name.as_deref() == Some(name)))
            .unwrap_or_else(|| panic!("{key} publishes nothing named {name}"))
            .id;
        let call = system.perform(window, node, semantics::Action::Click, None, grab);
        self.settle();
        self.driver.window_system_mut().outcome(call)
    }

    /// Whether window `key` is a popup that grabs, and whether it grabbed with an action's serial.
    fn grabs(&mut self, key: &str) -> (bool, bool) {
        let window = self
            .window(key)
            .unwrap_or_else(|| panic!("no window {key}"));
        let system = self.driver.window_system_mut();
        let grab = system.placement(window).unwrap().is_some_and(|p| p.grab);
        (grab, system.grabbed_by_action(window))
    }

    /// Presses and releases the key with `code` in window `key`.
    fn key(&mut self, key: &str, code: u32) {
        let window = self
            .window(key)
            .unwrap_or_else(|| panic!("no window {key}"));
        for pressed in [true, false] {
            let event = KeyEvent {
                code,
                keysym: Keysym(0),
                text: None,
                pressed,
                repeat: false,
                mods: Mods::NONE,
            };
            let system = self.driver.window_system_mut();
            if !system.windows().contains(&window) {
                break;
            }
            system
                .enqueue_window(window, WindowEvent::Key(event, Timestamp(0)))
                .unwrap();
            self.settle();
        }
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

/// How many shadows the bar's items draw under their text and symbols, for `look`.
fn shadows(look: Look) -> usize {
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

/// The session's Reduce Transparency alone shows the bar's solid strip (`tone::solid_bar`), and with it or without it,
/// in either tone and whatever the theme's own option, no item draws a shadow or halo under its text and symbols.
#[test]
fn the_items_draw_no_shadow_with_or_without_the_solid_strip() {
    use cupertino_defaults::accessibility::DisplayOptions;
    use cupertino_menubar::tone::solid_bar;
    let reduced = DisplayOptions {
        reduce_transparency: true,
        ..DisplayOptions::default()
    };
    assert!(solid_bar(reduced), "the strip shows");
    assert!(!solid_bar(DisplayOptions::default()), "the strip is hidden");
    for tone in [BarTone::Appearance, BarTone::LightText, BarTone::DarkText] {
        for solid in [false, true] {
            for theme_reduce_transparency in [false, true] {
                let look = Look {
                    solid,
                    theme_reduce_transparency,
                    tone,
                    ..Look::default()
                };
                assert_eq!(
                    shadows(look),
                    0,
                    "{tone:?}, solid {solid}, theme {theme_reduce_transparency}"
                );
            }
        }
    }
}

/// The bar's frame at 1x for `look`, with the File menu open when `open`, and the slot rectangles of the mark and of
/// the File title.
fn frame(look: Look, open: bool) -> (Pixmap, Rect, Rect) {
    LOOK.set(look);
    let mut run = Run::start();
    LOOK.set(Look::default());
    if open {
        run.open_file();
    }
    let layout = run.model().controller.layout();
    let slot = |key: &SlotKey| layout.slot(key).unwrap().rect;
    let (mark, file) = (slot(&SlotKey::System), slot(&SlotKey::Menu(ItemId(FILE))));
    (run.capture("bar-1").0, mark, file)
}

/// The sRGB luminance of the premultiplied pixel `px` taken alone, its colour without its coverage.
fn ink(px: &[u8; 4]) -> f64 {
    let a = f64::from(px[3]);
    let channel = |i: usize| f64::from(px[i]) / a;
    0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2)
}

/// The bar's text, the mark and the open title lie straight on the wallpaper: every pixel the bar draws, the
/// antialiased edges of the strokes included, is of the text's own tone, light under light text and dark under dark
/// text, so no darker ring or lighter glow surrounds a stroke and the mark has no outline. The open title's highlight
/// is the plain veil of the palette.
#[test]
fn the_strokes_lie_on_the_wallpaper_with_no_ring_around_them() {
    for (tone, light) in [(BarTone::LightText, true), (BarTone::DarkText, false)] {
        for open in [false, true] {
            let (pixmap, mark, file) = frame(
                Look {
                    tone,
                    ..Look::default()
                },
                open,
            );
            let w = pixmap.width() as usize;
            let px = pixmap.data().as_chunks::<4>().0;
            let inside = |r: Rect, x: usize, y: usize| {
                let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
                x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
            };
            let mut mark_ink = 0;
            for (i, p) in px.iter().enumerate() {
                let (x, y) = (i % w, i / w);
                // the open title's veil reaches past its slot, over the room between titles; its own colour is checked below
                if p[3] < 8
                    || (open && (file.x - 20.0..file.x + file.w + 20.0).contains(&(x as f64)))
                {
                    continue;
                }
                let l = ink(p);
                assert!(
                    if light { l > 0.6 } else { l < 0.4 },
                    "{tone:?}, open {open}: a pixel of luminance {l:.2} at ({x}, {y})"
                );
                if inside(mark, x, y) {
                    mark_ink += 1;
                }
            }
            assert!(mark_ink > 20, "{tone:?}: the mark is drawn ({mark_ink})");
            if open {
                // halfway down the highlight, a point in from its left end, clear of the text: the veil alone
                let (x, y) = ((file.x + 2.0) as usize, (file.y + file.h / 2.0) as usize);
                let p = px[y * w + x];
                assert!(p[3] > 0, "{tone:?}: the highlight shows");
                let l = ink(&p);
                assert!(
                    if light { l > 0.6 } else { l < 0.4 },
                    "{tone:?}: the highlight is the plain veil, luminance {l:.2}"
                );
                assert!(
                    p[3] < 64,
                    "{tone:?}: the highlight is a light veil, alpha {}",
                    p[3]
                );
            }
        }
    }
}

/// The bar's frame and the expanded state a screen reader reads for `name`, with `open` the items whose panel another
/// process shows open on the bar.
fn with_panels(open: Vec<SlotKey>, name: &str) -> (Pixmap, Option<bool>) {
    PANELS.set(open);
    let mut run = Run::start();
    PANELS.set(Vec::new());
    let window = run.window("bar-1").unwrap();
    let snapshot = run.driver.runtime().snapshot(window).unwrap();
    let expanded = snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("the bar has nothing named {name}"))
        .1
        .states
        .expanded;
    (run.capture("bar-1").0, expanded)
}

/// Whether two frames differ inside `inside` and nowhere else.
fn differ_only_in(a: &Pixmap, b: &Pixmap, inside: Rect) -> (bool, bool) {
    let (mut within, mut without) = (false, false);
    for y in 0..a.height() {
        for x in 0..a.width() {
            let i = (y * a.width() + x) as usize * 4;
            if a.data()[i..i + 4] == b.data()[i..i + 4] {
                continue;
            }
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            if px >= inside.x
                && px < inside.x + inside.w
                && py >= inside.y
                && py < inside.y + inside.h
            {
                within = true;
            } else {
                without = true;
            }
        }
    }
    (within, without)
}

/// While the control panel or the notification centre's list is open on the bar's output, its item is drawn
/// highlighted as an open menu's title is, and a screen reader reads it as expanded; nothing else of the bar changes
/// past the reach of the highlight.
#[test]
fn the_item_of_an_open_panel_is_highlighted_and_expanded() {
    let run = Run::start();
    let layout = run.model().controller.layout();
    // an item's highlight fills its frame inside its slot, and its edge touches the pixels it crosses
    let metrics = BarMetrics::default();
    let slot = |key: &SlotKey| {
        let r = layout.slot(key).unwrap().rect;
        let (dx, w) = metrics.highlight_span(key, r.w);
        let (left, right) = ((r.x + dx).floor(), (r.x + dx + w).ceil());
        Rect::new(left, r.y, right - left, r.h)
    };
    let (panel, clock) = (slot(&SlotKey::ControlPanel), slot(&SlotKey::Clock));
    let clock_name = run.model().controller.content().clock.clone();
    drop(run);

    let (closed, expanded) = with_panels(Vec::new(), "Control Panel");
    assert_eq!(expanded, Some(false), "a closed panel's item is collapsed");
    let (open, expanded) = with_panels(vec![SlotKey::ControlPanel], "Control Panel");
    assert_eq!(expanded, Some(true), "the open panel's item is expanded");
    assert_eq!(
        differ_only_in(&closed, &open, panel),
        (true, false),
        "only the control panel's item changes"
    );

    let (_, expanded) = with_panels(Vec::new(), &clock_name);
    assert_eq!(
        expanded,
        Some(false),
        "the closed list's clock is collapsed"
    );
    let (open, expanded) = with_panels(vec![SlotKey::Clock], &clock_name);
    assert_eq!(expanded, Some(true), "the open list's clock is expanded");
    assert_eq!(
        differ_only_in(&closed, &open, clock),
        (true, false),
        "only the clock changes"
    );
}

/// A menu desktop control opens before any input grabs with the click's serial: the keyboard walks it and opens its
/// submenu, which grabs as part of the same chain, and Escape closes one level at a time.
#[test]
fn a_menu_desktop_control_opens_grabs_and_the_keyboard_walks_it_into_its_submenu() {
    let mut run = Run::controlled();
    let outcome = run.perform_click("bar-1", "Edit", true);
    assert_eq!(outcome, Some(semantics::Outcome::Applied));
    assert!(run.model().controller.is_open(), "Edit opens");
    assert_eq!(run.grabs("menu-1-0"), (true, true));
    let menu = run.window("menu-1-0").unwrap();
    run.driver
        .window_system_mut()
        .enqueue_window(menu, WindowEvent::Focus(true))
        .unwrap();
    run.settle();
    run.key("menu-1-0", DOWN);
    run.key("menu-1-0", DOWN);
    let edit = &run.model().controller.levels()[0];
    assert_eq!(
        edit.selected,
        edit.layout.index_of(ItemId(FIND)),
        "the arrows reach Find"
    );
    run.key("menu-1-0", RIGHT);
    assert!(run.grabs("menu-1-1").0, "the submenu grabs too");
    run.key("menu-1-1", ESCAPE);
    assert_eq!(run.window("menu-1-1"), None, "Escape closes the submenu");
    assert!(run.window("menu-1-0").is_some());
    run.key("menu-1-0", ESCAPE);
    assert_eq!(run.window("menu-1-0"), None, "and then the menu");
}

/// A submenu desktop control opens from a menu it opened grabs with its own click's serial; when the compositor ends
/// the grab, after a press in another application, every level closes.
#[test]
fn a_submenu_desktop_control_opens_grabs_and_the_end_of_the_grab_closes_every_level() {
    let mut run = Run::controlled();
    run.perform_click("bar-1", "Edit", true);
    let outcome = run.perform_click("menu-1-0", "Find", true);
    assert_eq!(outcome, Some(semantics::Outcome::Applied));
    assert_eq!(run.grabs("menu-1-1"), (true, true));
    let submenu = run.window("menu-1-1").unwrap();
    run.driver
        .window_system_mut()
        .enqueue_window(submenu, WindowEvent::PopupDone)
        .unwrap();
    run.settle();
    assert!(!run.model().controller.is_open());
    assert_eq!(run.window("menu-1-0"), None);
    assert_eq!(run.window("menu-1-1"), None);
}

/// From a compositor that hands out no grab serial, a menu desktop control opens before any input opens without a
/// grab, as before.
#[test]
fn without_a_grab_serial_a_menu_desktop_control_opens_opens_without_a_grab() {
    let mut run = Run::controlled();
    run.perform_click("bar-1", "File", false);
    assert!(run.model().controller.is_open());
    assert_eq!(run.grabs("menu-1-0"), (false, false));
}
