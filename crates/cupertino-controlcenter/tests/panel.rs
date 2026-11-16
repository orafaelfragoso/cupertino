#![cfg(feature = "app")]
//! The panel as CupertinoKit draws it, compared with `tests/goldens`: `<scene>.png` is the frame at 1x and `<scene>.txt`
//! the focus and the accessibility tree, for the shown panel, under Reduce Transparency, under Increase Contrast, in a
//! light session and with the Wi-Fi view open. The glass is the compositor's, so the frames hold what the window paints
//! over it. Pixels may differ by at most 2/255 per channel and are compared on x86_64 only, as the renderer's own
//! baselines are. `CUPERTINO_UPDATE_GOLDENS=1` re-records every scene it runs and then fails, so the variable can never
//! pass a run.

use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_controlcenter::arrangement::Arrangement;
use cupertino_controlcenter::layout::editing;
use cupertino_controlcenter::layout::{
    BAR_MARGIN, GRID_ORIGIN, SHADOW_TOP, SURFACE, WINDOW_ORIGIN,
};
use cupertino_controlcenter::model::{
    Controls, Detail, Device, MediaCommand, Module, Network, Output, Reading, Slider, VolumeState,
    WifiStatus,
};
use cupertino_controlcenter::view::{self, Editing, Held, Input, Opened, Presentation, Via};
use cupertino_kit::core::{
    AccessibleValue, Action, Application, Command, Key, PlaceholderIcons, Role, RuntimeOptions,
    Window, WindowBackground,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::GlassStyle;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowRequest, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::glass::adapts_foreground;
use cupertino_theme::Theme;
use cupertino_ui_test::{AssistiveTechnology, Inspector, ManualClock, TestDriver};
use tiny_skia::Pixmap;

/// Where the window's corner lies in the surface, past the margin its shadow takes.
const ORIGIN: Point = WINDOW_ORIGIN;
const ESCAPE: u32 = 0xff1b;
const TAB: u32 = 0xff09;
const SPACE: u32 = 0x20;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;

thread_local! {
    static PRESENTATION: Cell<Presentation> = const { Cell::new(Presentation::SHOWN) };
    /// Show the catcher instead of the panel.
    static CATCHER: Cell<bool> = const { Cell::new(false) };
    static CONTROLS: RefCell<Option<Controls>> = const { RefCell::new(None) };
    /// The controls the panel shows and their order, when not every one in reading order.
    static LAYOUT: RefCell<Option<Arrangement>> = const { RefCell::new(None) };
}

/// Every backend answering, as `--fake-backends` does, with the microphone and focus mode on and nothing playing.
fn controls() -> Controls {
    let network = |ssid: &str, connected: bool, secured: bool| Network {
        ssid: ssid.to_owned(),
        bars: Some(3),
        secured,
        known: connected,
        connected,
    };
    Controls {
        wifi: Reading::Ready(WifiStatus {
            enabled: true,
            network: Some("Home Network".to_owned()),
        }),
        bluetooth: Reading::Ready(false),
        microphone: Reading::Ready(true),
        dock_hiding: Reading::Ready(false),
        keyboard: Reading::Unavailable("there is no keyboard backlight".to_owned()),
        focus_mode: Reading::Ready(true),
        brightness: Reading::Ready(0.6),
        volume: Reading::Ready(VolumeState {
            level: 0.4,
            muted: false,
        }),
        media: Reading::Ready(None),
        networks: Reading::Ready(vec![
            network("Home Network", true, true),
            network("Guest", false, false),
        ]),
        devices: Reading::Ready(vec![Device {
            id: "00:11:22:33:44:55".to_owned(),
            name: "Headphones".to_owned(),
            connected: false,
        }]),
        outputs: Reading::Ready(vec![Output {
            id: 31,
            name: "Speakers".to_owned(),
            current: true,
        }]),
    }
}

/// The shown panel; it keeps what the user did on it and moves the focus where an arrow key asks. An edit of its
/// controls is carried out on the arrangement it shows, as the process does on the controller's, and ends with Done
/// or Cancel; what either keeps is the controller's and the process's to prove.
struct Shown;

struct State {
    controls: Controls,
    heard: Vec<Input>,
    arrangement: Arrangement,
    edit: Option<Editing>,
}

impl Application for Shown {
    type Model = State;
    type Message = Input;

    fn init() -> (Self::Model, Vec<Command<Input>>) {
        let state = State {
            controls: CONTROLS.with_borrow(|c| c.clone()).unwrap_or_else(controls),
            heard: Vec::new(),
            arrangement: LAYOUT.with_borrow(|l| l.clone()).unwrap_or_default(),
            edit: PRESENTATION.get().edit,
        };
        (state, Vec::new())
    }

    fn update(state: &mut Self::Model, input: Input) -> Vec<Command<Input>> {
        state.heard.push(input.clone());
        let focus = |key: String| vec![Command::Focus(Key::from(key))];
        match input {
            Input::Focus(key) => return focus(key),
            Input::EditControls => state.edit = Some(Editing::default()),
            Input::Done | Input::Cancel => state.edit = None,
            Input::Remove(module, via) => {
                let place = state.arrangement.position(module);
                state.arrangement.remove(module);
                let shown = state.arrangement.shown();
                if let (Via::Keyboard, Some(next)) = (via, place.and_then(|p| shown.get(p))) {
                    return focus(view::edit_key(*next));
                }
            }
            Input::Add(module, via) => {
                state.arrangement.add(module);
                if via == Via::Keyboard {
                    // the tile is there to take the focus once the view that follows has built it
                    let focus = Input::Focus(view::edit_key(module));
                    return vec![Command::Perform(Box::pin(async move { focus }))];
                }
            }
            Input::Step(module, forward) => {
                state.arrangement.step(module, forward);
            }
            Input::DragTo(module, origin) => {
                state.edit = Some(Editing {
                    held: Some(Held { module, origin }),
                });
                state.arrangement.move_near(module, origin);
            }
            Input::Drop => state.edit = Some(Editing::default()),
            _ => {}
        }
        Vec::new()
    }

    fn view(state: &Self::Model) -> Vec<Window<Input>> {
        if CATCHER.get() {
            vec![view::catcher(|input| input)]
        } else {
            let presentation = Presentation {
                edit: state.edit,
                ..PRESENTATION.get()
            };
            vec![view::arranged(
                &state.controls,
                &state.arrangement,
                presentation,
                |input| input,
            )]
        }
    }
}

/// The Wi-Fi view, open.
const WIFI_OPEN: Presentation = Presentation {
    alpha: 1.0,
    light: false,
    input: true,
    detail: Some(Opened {
        detail: Detail::WiFi,
        progress: 1.0,
        morph: true,
    }),
    edit: None,
};

fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

fn tokens(appearance: Appearance, reduce_transparency: bool) -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

struct Run {
    driver: TestDriver<Shown>,
    window: WindowId,
    /// The screen reader the panel talks to.
    reader: AssistiveTechnology,
}

impl Run {
    /// The panel in `theme` at `presentation` on a desktop that offers materials, configured, active and drawn.
    fn start(theme: ThemeTokens, presentation: Presentation) -> Self {
        PRESENTATION.set(presentation);
        Self::open(theme, false, None)
    }

    /// The shown panel at rest in the dark appearance.
    fn shown() -> Self {
        Self::start(tokens(Appearance::Dark, false), Presentation::SHOWN)
    }

    /// The catcher, configured over a 400 x 300 output.
    fn catcher() -> Self {
        Self::open(
            tokens(Appearance::Dark, false),
            true,
            Some(Size::new(400.0, 300.0)),
        )
    }

    fn open(theme: ThemeTokens, catcher: bool, size: Option<Size>) -> Self {
        CATCHER.set(catcher);
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let reader = AssistiveTechnology::new();
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 400,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 20,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: Some(reader.adapter()),
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let capabilities = Capabilities {
            app_effects: true,
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
            .configure(window, size, active)
            .unwrap();
        let mut run = Self {
            driver,
            window,
            reader,
        };
        run.settle();
        run
    }

    /// Runs until every window rests, then checks that a screen reader could read every window open.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Control Panel");
    }

    fn settle_frames(&mut self) {
        for _ in 0..8 {
            self.driver.run_until_idle(budget()).unwrap();
            let system = self.driver.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            system.frame_ready(self.window, Timestamp(0)).unwrap();
        }
        panic!("the panel keeps asking for frames");
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
        self.press(position, MouseButton::Left, pressed);
    }

    fn press(&mut self, position: Point, button: MouseButton, pressed: bool) {
        self.pointer(PointerEvent::Button {
            position,
            button,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        });
    }

    fn key(&mut self, keysym: u32) {
        for pressed in [true, false] {
            self.send(WindowEvent::Key(
                KeyEvent {
                    code: 1,
                    keysym: Keysym(keysym),
                    text: None,
                    pressed,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ));
        }
    }

    fn heard(&self) -> &[Input] {
        &self.driver.runtime().model().heard
    }

    /// The controls the panel shows, in their order.
    fn order(&self) -> Vec<Module> {
        self.driver.runtime().model().arrangement.shown().to_vec()
    }

    /// The accessible node named `name` with `role`, when there is one.
    fn node(&self, role: Role, name: &str) -> Option<cupertino_kit::core::AccessibilityNode> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot
            .accessibility
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.role == role && n.name.as_deref() == Some(name))
            .cloned()
    }

    /// Asks the node named `name` with `role` for `action`, as a screen reader does.
    fn ask(&mut self, role: Role, name: &str, action: Action) {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let target = snapshot
            .accessibility
            .iter()
            .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no {role:?} named {name}"))
            .0;
        let reader = self.reader.clone();
        self.driver.ask(&reader, target, action, None);
        self.settle();
    }

    /// The name of the node that holds the keyboard focus.
    fn focused(&self) -> Option<String> {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot
            .accessibility
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.states.focused)
            .and_then(|n| n.name.clone())
    }

    /// The bounds of the accessible node named `name` with `role`.
    fn bounds(&self, role: Role, name: &str) -> Rect {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot
            .accessibility
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.role == role && n.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no {role:?} named {name}"))
            .bounds
    }

    /// The size and colour of the glyphs of the label a screen reader reads as `name`: those of the paint node, which
    /// takes in the glyphs' ink, whose centre lies nearest the label's.
    fn glyphs(&self, name: &str) -> (f32, cupertino_kit::theme::Color) {
        use cupertino_kit::render::PaintOp;
        let label = self.bounds(Role::StaticText, name).center();
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        snapshot
            .display_list
            .nodes
            .iter()
            .filter_map(|n| {
                n.ops.iter().find_map(|op| match op {
                    PaintOp::Glyphs { run, color, .. } => {
                        let c = n.bounds.center();
                        let distance = (c.x - label.x).hypot(c.y - label.y);
                        Some((distance, run.size, *color))
                    }
                    _ => None,
                })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, size, color)| (size, color))
            .unwrap_or_else(|| panic!("no glyphs for {name}"))
    }

    /// The last frame's premultiplied pixels and the focus and accessibility lines of the dump.
    fn capture(&mut self) -> (Pixmap, String) {
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
        let mut pixmap = Pixmap::new(frame.width, frame.height).unwrap();
        pixmap.data_mut().copy_from_slice(&frame.pixels);
        (pixmap, text)
    }
}

fn check(
    scene: &str,
    appearance: Appearance,
    reduce_transparency: bool,
    presentation: Presentation,
) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens");
    let (png, txt) = (format!("{dir}/{scene}.png"), format!("{dir}/{scene}.txt"));
    let (actual, text) =
        Run::start(tokens(appearance, reduce_transparency), presentation).capture();
    assert_eq!(
        (actual.width(), actual.height()),
        (SURFACE.width as u32, SURFACE.height as u32),
        "{scene}: the panel is drawn at 1x"
    );
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
fn the_shown_panel_matches_its_golden() {
    check("shown", Appearance::Dark, false, Presentation::SHOWN);
}

#[test]
fn the_panel_under_reduce_transparency_matches_its_golden() {
    check(
        "reduce-transparency",
        Appearance::Dark,
        true,
        Presentation::SHOWN,
    );
}

#[test]
fn the_panel_under_increase_contrast_matches_its_golden() {
    check(
        "increase-contrast",
        Appearance::DarkHighContrast,
        false,
        Presentation::SHOWN,
    );
}

#[test]
fn the_panel_in_a_light_session_matches_its_golden() {
    let light = Presentation {
        light: true,
        ..Presentation::SHOWN
    };
    check("light-session", Appearance::Dark, false, light);
}

/// The panel keeps one look in both appearances: a light session draws it as a dark one does.
#[test]
fn a_light_session_draws_the_panel_as_a_dark_one_does() {
    let light = Presentation {
        light: true,
        ..Presentation::SHOWN
    };
    let (light, _) = Run::start(tokens(Appearance::Dark, false), light).capture();
    let (dark, _) = Run::start(tokens(Appearance::Dark, false), Presentation::SHOWN).capture();
    assert!(light.data() == dark.data(), "the same frame");
}

/// A tile's state is set in the theme's caption size under its title in the body size: the title in the label colour,
/// the state in the secondary label colour, quieter under the name. The slider tiles' titles and Edit Controls are in
/// the label colour, in the body size. Increase Contrast draws the theme's own colours.
#[test]
fn a_tile_s_state_is_smaller_and_quieter_than_its_title() {
    let run = Run::shown();
    let alpha = |c: cupertino_kit::theme::Color| (c.a * 100.0).round() / 100.0;
    for (title, state) in [("Wi-Fi", "Home Network"), ("Bluetooth", "Off")] {
        let (title_size, title_color) = run.glyphs(title);
        let (state_size, state_color) = run.glyphs(state);
        assert_eq!((title_size, state_size), (13.0, 11.0), "{title}");
        assert_eq!(alpha(title_color), 1.0, "{title}");
        assert!(
            alpha(state_color) < alpha(title_color) && alpha(state_color) > 0.4,
            "{title}: {state_color:?}"
        );
    }
    for full in ["Display", "Sound", "Edit Controls"] {
        let (size, color) = run.glyphs(full);
        assert_eq!((size, alpha(color)), (13.0, 1.0), "{full}");
    }
    assert_eq!(alpha(run.glyphs("Not Playing").1), 1.0);
    assert_eq!(alpha(run.glyphs("Do Not Disturb").1), 1.0);

    let contrast = Run::start(
        tokens(Appearance::DarkHighContrast, false),
        Presentation::SHOWN,
    );
    let (_, title) = contrast.glyphs("Wi-Fi");
    let (size, state) = contrast.glyphs("Home Network");
    assert_eq!(size, 11.0);
    assert!(
        alpha(title) > 0.85 && alpha(state) > 0.7,
        "Increase Contrast keeps the theme's stronger labels: {title:?} {state:?}"
    );
}

/// The compositor measures the face of a glass container whose bounds are small and tells the window to draw its
/// text light or dark to match; a larger container keeps the appearance's text. The panel's labels are white over its
/// own tint in a light session, which the compositor does not see, so no container of the tiles is ever small: at rest,
/// in a light session, while a view opens or cross-fades (the view and the tiles behind it, a container each), and with
/// it open. A container is one glass of one style.
#[test]
fn the_tiles_are_glass_too_large_for_their_text_to_follow_the_backdrop() {
    let opening = |progress, morph| Presentation {
        detail: Some(Opened {
            detail: Detail::WiFi,
            progress,
            morph,
        }),
        ..Presentation::SHOWN
    };
    let scenes = [
        Presentation::SHOWN,
        Presentation {
            light: true,
            ..Presentation::SHOWN
        },
        Presentation {
            alpha: 0.5,
            ..Presentation::SHOWN
        },
        opening(0.0, true),
        opening(0.1, true),
        opening(0.3, true),
        opening(0.7, true),
        opening(0.5, false),
        WIFI_OPEN,
    ];
    for presentation in scenes {
        let mut run = Run::start(tokens(Appearance::Dark, false), presentation);
        let requests = run.driver.window_system_mut().requests(run.window).unwrap();
        let regions = requests
            .iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Materials(regions) => Some(regions.clone()),
                _ => None,
            })
            .expect("the panel asks for glass");
        assert!(!regions.is_empty(), "{presentation:?}: no glass");
        // the levels' tracks are glass of their own over the tiles, apart from the containers
        let regions: Vec<_> = regions
            .iter()
            .filter(|r| r.glass.expect("every tile is glass").style != GlassStyle::Track)
            .collect();
        let mut containers: Vec<u32> = regions
            .iter()
            .map(|r| r.glass.expect("every tile is glass").container)
            .collect();
        containers.sort_unstable();
        containers.dedup();
        assert!(
            containers.iter().all(|c| *c != 0),
            "{presentation:?}: every tile is in a container: {containers:?}"
        );
        for container in containers {
            let members: Vec<_> = regions
                .iter()
                .filter(|r| r.glass.unwrap().container == container)
                .collect();
            let style = members[0].glass.unwrap().style;
            assert!(
                members.iter().all(|r| r.glass.unwrap().style == style),
                "{presentation:?}: container {container} mixes styles"
            );
            let bounds = members
                .iter()
                .map(|r| r.rect)
                .reduce(|a, b| a.union(&b))
                .unwrap();
            let short = bounds.width().min(bounds.height());
            assert!(
                !adapts_foreground(short as f32),
                "{presentation:?}: container {container}'s short side, {short} pt, lets its text follow the backdrop"
            );
        }
    }
}

/// The glass regions the panel last asked for, but the levels' tracks.
fn glass_regions(run: &mut Run) -> Vec<Rect> {
    let requests = run.driver.window_system_mut().requests(run.window).unwrap();
    requests
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(regions) => Some(
                regions
                    .iter()
                    .filter(|r| r.glass.is_none_or(|g| g.style != GlassStyle::Track))
                    .map(|r| r.rect)
                    .collect(),
            ),
            _ => None,
        })
        .expect("the panel asks for glass")
}

/// The glass the panel last asked for, but the levels' tracks: each region's rectangle, style, container and opacity.
fn glass_requests(run: &mut Run) -> Vec<(Rect, GlassStyle, u32, f64)> {
    let requests = run.driver.window_system_mut().requests(run.window).unwrap();
    requests
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(regions) => Some(
                regions
                    .iter()
                    .filter_map(|r| r.glass.map(|g| (r.rect, r.alpha, g)))
                    .filter(|(_, _, g)| g.style != GlassStyle::Track)
                    .map(|(rect, alpha, g)| (rect, g.style, g.container, alpha))
                    .collect(),
            ),
            _ => None,
        })
        .expect("the panel asks for glass")
}

/// The Wi-Fi view `progress` of the way open, growing out of its tile (`morph`) or cross-fading with Reduce Motion.
fn wifi_at(progress: f64, morph: bool) -> Presentation {
    Presentation {
        detail: Some(Opened {
            detail: Detail::WiFi,
            progress,
            morph,
        }),
        ..Presentation::SHOWN
    }
}

/// Every tile, Edit Controls and the growing view keep the module glass of one container through a detail view's
/// motion, only their opacity changing: nothing changes look the moment the press starts the motion, and the tiles
/// fade back on the glass they left with.
#[test]
fn every_tile_keeps_its_glass_through_a_detail_views_motion() {
    let mut run = Run::start(tokens(Appearance::Dark, false), Presentation::SHOWN);
    let rest = glass_requests(&mut run);
    assert_eq!(rest.len(), Module::ALL.len() + 1, "{rest:?}");
    for morph in [true, false] {
        let mut run = Run::start(tokens(Appearance::Dark, false), wifi_at(0.0, morph));
        let mut started = glass_requests(&mut run);
        if !morph {
            // the view fades in at its own place from nothing
            started.retain(|(_, _, _, alpha)| *alpha > 0.0);
        }
        let mut sorted = rest.clone();
        let key = |r: &(Rect, GlassStyle, u32, f64)| (r.0.x(), r.0.y());
        sorted.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap());
        started.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap());
        assert_eq!(started, sorted, "morph {morph}: the press changes no glass");
        for progress in [0.05, 0.1, 0.2, 0.3, 0.5, 0.8, 1.0] {
            let mut run = Run::start(tokens(Appearance::Dark, false), wifi_at(progress, morph));
            for (rect, style, container, _) in glass_requests(&mut run) {
                assert_eq!(
                    (style, container),
                    (GlassStyle::Module, rest[0].2),
                    "morph {morph}, {progress}: {rect:?}"
                );
            }
        }
    }
}

/// How far two frames of the panel differ: the largest difference of any channel.
fn largest_difference(a: &Pixmap, b: &Pixmap) -> u8 {
    a.data()
        .iter()
        .zip(b.data())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap()
}

/// The soft shadow, between and around the tiles, does not change the moment a detail view starts to open or has
/// closed, and changes only a little over a small step of the motion: it follows the tiles fading and the tile growing
/// rather than dropping the darkening between the tiles while they move.
#[test]
fn the_shadow_follows_a_detail_views_motion_without_a_jump() {
    let (rest, _) = Run::start(tokens(Appearance::Dark, false), Presentation::SHOWN).capture();
    for morph in [true, false] {
        let (started, _) =
            Run::start(tokens(Appearance::Dark, false), wifi_at(0.0, morph)).capture();
        assert!(
            largest_difference(&rest, &started) <= 1,
            "morph {morph}: the press changes the frame by {}",
            largest_difference(&rest, &started)
        );
        let mut last = started;
        for step in 1..=10 {
            let progress = f64::from(step) * 0.01;
            let (frame, _) =
                Run::start(tokens(Appearance::Dark, false), wifi_at(progress, morph)).capture();
            // a hundredth of the motion fades the other tiles by a 25th, and the darkening under them grows by as
            // much of the 9 % between them; the gap between them keeps it
            let alpha = |frame: &Pixmap, x: u32, y: u32| {
                frame
                    .pixel(x + ORIGIN.x as u32, y + ORIGIN.y as u32)
                    .unwrap()
                    .alpha()
            };
            // the gap between the two circles of the fourth row, and the middle of the Display tile under them
            for (x, y, place) in [(78, 268, "a gap"), (154, 318, "a fading tile")] {
                let (before, after) = (alpha(&last, x, y), alpha(&frame, x, y));
                assert!(
                    before.abs_diff(after) <= 2,
                    "morph {morph}, {progress}: the darkening over {place} jumps from {before} to {after}"
                );
            }
            last = frame;
        }
    }
}

/// The shadow above the tiles fades out by the menu bar's edge: nothing of the panel darkens the bar, and the
/// darkening grows smoothly from nothing at the bar's edge to its strength at the tiles, with no step where the bar
/// begins. At rest, with a view open and halfway through its motion.
#[test]
fn the_shadow_fades_out_before_the_menu_bar() {
    let bar_edge = (SHADOW_TOP - f64::from(BAR_MARGIN)) as u32;
    let grid_top = GRID_ORIGIN.y as u32;
    for presentation in [Presentation::SHOWN, wifi_at(0.5, true), WIFI_OPEN] {
        let (frame, _) = Run::start(tokens(Appearance::Dark, false), presentation).capture();
        for y in 0..bar_edge {
            for x in 0..frame.width() {
                assert_eq!(
                    frame.pixel(x, y).unwrap().alpha(),
                    0,
                    "{presentation:?}: the panel paints ({x}, {y}) in the menu bar"
                );
            }
        }
        // over the middle of the grid, and past its left edge where the shadow's corner lies
        for x in [GRID_ORIGIN.x as u32 + 146, GRID_ORIGIN.x as u32 - 20] {
            let alphas: Vec<u8> = (bar_edge..grid_top)
                .map(|y| frame.pixel(x, y).unwrap().alpha())
                .collect();
            assert!(
                alphas[0] <= 1,
                "{presentation:?}: at the bar's edge above x {x} the shadow is {alphas:?}"
            );
            assert!(
                // a band thinner than a pixel can come out a step lighter than the one above it
                alphas.windows(2).all(|w| w[1] + 1 >= w[0] && w[1] <= w[0] + 3),
                "{presentation:?}: the shadow above x {x} grows smoothly towards the tiles: {alphas:?}"
            );
        }
        let at_grid = frame
            .pixel(GRID_ORIGIN.x as u32 + 146, grid_top - 1)
            .unwrap()
            .alpha();
        assert!(
            at_grid >= 18,
            "{presentation:?}: the shadow keeps its strength at the tiles: {at_grid}"
        );
    }
}

/// Edit Controls is glass as the tiles are, in a dark and in a light session: the capsule takes its colour from what lies
/// behind it (a light blue-grey over a light part of the wallpaper), with the tiles' rim, rather than a flat fill of its
/// own.
#[test]
fn edit_controls_is_glass_as_the_tiles_are() {
    let capsule = Rect::new(107.0 + ORIGIN.x, 468.0 + ORIGIN.y, 94.0, 24.0);
    for light in [false, true] {
        let mut run = Run::start(
            tokens(Appearance::Dark, false),
            Presentation {
                light,
                ..Presentation::SHOWN
            },
        );
        let regions = glass_regions(&mut run);
        assert_eq!(
            regions.len(),
            Module::ALL.len() + 1,
            "a glass region per tile and Edit Controls (light {light})"
        );
        assert!(
            regions.contains(&capsule),
            "Edit Controls is glass (light {light}): {regions:?}"
        );
    }
}

#[test]
fn the_opaque_panel_of_reduce_transparency_holds_edit_controls_too() {
    let (frame, _) = Run::start(tokens(Appearance::Dark, true), Presentation::SHOWN).capture();
    // beside the Edit Controls capsule, at its height, and just above the panel's bottom edge under it
    for (x, y) in [(40, 480), (154, 496)] {
        let (x, y) = (x + ORIGIN.x as u32, y + ORIGIN.y as u32);
        let pixel = frame.pixel(x, y).unwrap();
        assert_eq!(pixel.alpha(), 255, "opaque at ({x}, {y})");
        assert!(
            pixel.red().abs_diff(77) <= 2,
            "the panel's grey at ({x}, {y}): {}",
            pixel.red()
        );
    }
    let capsule = frame
        .pixel(154 + ORIGIN.x as u32, 470 + ORIGIN.y as u32)
        .unwrap();
    assert!(
        capsule.red() < 40,
        "the capsule is a dark tile on the panel: {}",
        capsule.red()
    );
}

#[test]
fn the_open_wifi_view_matches_its_golden() {
    check("wifi-detail", Appearance::Dark, false, WIFI_OPEN);
}

/// With `CUPERTINO_LOOK_DIR` set, the panel as is, with Reduce Transparency and with the Wi-Fi view open, for review
/// by eye; otherwise nothing is written.
#[test]
fn the_panel_can_be_looked_at() {
    for (reduce_transparency, presentation, name) in [
        (false, Presentation::SHOWN, "controlcenter"),
        (true, Presentation::SHOWN, "controlcenter-rt"),
        (false, WIFI_OPEN, "controlcenter-wifi"),
    ] {
        let mut run = Run::start(tokens(Appearance::Dark, reduce_transparency), presentation);
        run.driver.look(run.window, name).unwrap();
    }
}

#[test]
fn the_panel_is_a_transparent_window_of_its_own_tiles() {
    let shown = view::panel::<()>(&controls(), Presentation::SHOWN, |_| ());
    assert_eq!(shown.background, WindowBackground::None);
    assert_eq!(shown.spec.app_id, view::APP_ID);
    assert_eq!(shown.spec.size, SURFACE);
    assert_eq!(
        SURFACE,
        Size::new(436.0, 646.0),
        "the window with its shadow's margin"
    );
}

fn frame(r: Rect) -> (f64, f64, f64, f64) {
    (r.x(), r.y(), r.width(), r.height())
}

#[test]
fn the_tiles_sit_on_their_grid_eight_points_in() {
    let run = Run::shown();
    // in the window, whose corner lies past the shadow's margin
    let within = |r: Rect| {
        let (x, y, w, h) = frame(r);
        (x - ORIGIN.x, y - ORIGIN.y, w, h)
    };
    let group = |name| within(run.bounds(Role::Group, name));
    assert_eq!(group("Wi-Fi"), (8.0, 8.0, 140.0, 64.0));
    assert_eq!(group("Bluetooth"), (8.0, 84.0, 140.0, 64.0));
    assert_eq!(group("Now Playing"), (160.0, 8.0, 140.0, 140.0));
    assert_eq!(group("Microphone"), (8.0, 160.0, 140.0, 64.0));
    assert_eq!(group("Dock Hiding"), (160.0, 160.0, 64.0, 64.0));
    assert_eq!(group("Lock Screen"), (236.0, 160.0, 64.0, 64.0));
    assert_eq!(group("Mute"), (8.0, 236.0, 64.0, 64.0));
    assert_eq!(group("Keyboard Brightness"), (84.0, 236.0, 64.0, 64.0));
    assert_eq!(group("Focus"), (160.0, 236.0, 140.0, 64.0));
    assert_eq!(group("Display"), (8.0, 312.0, 292.0, 64.0));
    assert_eq!(group("Sound"), (8.0, 388.0, 292.0, 64.0));
    assert_eq!(group("Edit Controls"), (107.0, 468.0, 94.0, 24.0));
    assert_eq!(
        within(run.bounds(Role::Switch, "Wi-Fi")),
        (22.0, 22.0, 36.0, 36.0),
        "the badge is 36 across, 14 in from the capsule's edges"
    );
    assert_eq!(
        within(run.bounds(Role::Slider, "Sound")),
        (37.0, 418.0, 188.0, 24.0),
        "the level's band is centred 42 below the tile's top and reaches 4 past each end of the track"
    );
}

fn click(run: &mut Run, at: Point) {
    run.pointer(PointerEvent::Motion { position: at });
    run.button(at, true);
    run.button(at, false);
}

#[test]
fn a_press_on_a_badge_switches_its_module_and_on_the_text_opens_its_view() {
    let mut run = Run::shown();
    let badge = run.bounds(Role::Switch, "Bluetooth").center();
    click(&mut run, badge);
    let more = run.bounds(Role::Button, "Bluetooth Devices").center();
    click(&mut run, more);
    assert_eq!(
        run.heard(),
        [
            Input::Press(Module::Bluetooth),
            Input::Expand(Detail::Bluetooth, Via::Pointer)
        ]
    );
}

#[test]
fn the_circles_the_output_button_and_the_media_buttons_act() {
    let mut run = Run::shown();
    for (role, name) in [
        (Role::Button, "Lock Screen"),
        (Role::Switch, "Dock Hiding"),
        (Role::Button, "Output"),
    ] {
        let at = run.bounds(role, name).center();
        click(&mut run, at);
    }
    assert_eq!(
        run.heard(),
        [
            Input::Press(Module::LockScreen),
            Input::Press(Module::DockHiding),
            Input::Expand(Detail::Sound, Via::Pointer),
        ]
    );
    // nothing plays, so the media buttons are disabled
    let play = run.bounds(Role::Button, "Play").center();
    click(&mut run, play);
    assert_eq!(run.heard().len(), 3);
    let keyboard = run.bounds(Role::Switch, "Keyboard Brightness").center();
    click(&mut run, keyboard);
    assert_eq!(
        run.heard().len(),
        3,
        "a module without a backend is disabled"
    );
}

#[test]
fn a_playing_player_takes_the_media_buttons() {
    CONTROLS.with_borrow_mut(|c| {
        let mut playing = controls();
        playing.media = Reading::Ready(Some(cupertino_controlcenter::model::Media {
            title: "Song".to_owned(),
            artist: Some("Band".to_owned()),
            playing: true,
            can_previous: true,
            can_next: true,
            can_play_pause: true,
        }));
        *c = Some(playing);
    });
    let mut run = Run::shown();
    CONTROLS.with_borrow_mut(|c| *c = None);
    for name in ["Previous", "Pause", "Next"] {
        let at = run.bounds(Role::Button, name).center();
        click(&mut run, at);
    }
    assert_eq!(
        run.heard(),
        [
            Input::Media(MediaCommand::Previous),
            Input::Media(MediaCommand::PlayPause),
            Input::Media(MediaCommand::Next),
        ]
    );
}

#[test]
fn a_drag_slides_the_level_and_letting_go_releases_it() {
    let mut run = Run::shown();
    let track = run.bounds(Role::Slider, "Sound");
    let at = |x: f64| Point::new(x, track.center().y);
    run.pointer(PointerEvent::Motion {
        position: at(track.max_x() - 1.0),
    });
    run.button(at(track.max_x() - 1.0), true);
    run.button(at(track.max_x() - 1.0), false);
    assert_eq!(
        run.heard(),
        [
            Input::Slide(Slider::Sound, 1.0),
            Input::Release(Slider::Sound, 0.4)
        ],
        "the model has not taken the new level, so the end reports the level it shows"
    );
}

#[test]
fn escape_dismisses_the_panel_and_collapses_an_open_view() {
    let mut run = Run::shown();
    run.key(ESCAPE);
    assert_eq!(run.heard(), [Input::Dismiss]);
    let mut run = Run::start(tokens(Appearance::Dark, false), WIFI_OPEN);
    run.key(ESCAPE);
    assert_eq!(run.heard(), [Input::Collapse(Via::Keyboard)]);
}

#[test]
fn a_press_between_the_tiles_dismisses_the_panel_and_one_on_a_tile_does_not() {
    let mut run = Run::shown();
    // inside the media tile, away from its buttons
    click(&mut run, Point::new(280.0 + ORIGIN.x, 30.0 + ORIGIN.y));
    assert_eq!(run.heard(), []);
    // the gap between the two columns
    click(&mut run, Point::new(154.0 + ORIGIN.x, 40.0 + ORIGIN.y));
    assert_eq!(run.heard(), [Input::Dismiss]);
}

#[test]
fn a_press_outside_an_open_view_collapses_it() {
    let mut run = Run::start(tokens(Appearance::Dark, false), WIFI_OPEN);
    click(&mut run, Point::new(150.0, 420.0));
    assert_eq!(run.heard(), [Input::Collapse(Via::Pointer)]);
}

#[test]
fn the_wifi_view_lists_the_networks_and_joins_one() {
    let mut run = Run::start(tokens(Appearance::Dark, false), WIFI_OPEN);
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let names: Vec<_> = snapshot
        .accessibility
        .iter()
        .map(|(_, n)| n)
        .filter(|n| n.role == Role::ListItem)
        .map(|n| (n.name.clone().unwrap(), n.states.selected))
        .collect();
    assert_eq!(
        names,
        [
            ("Home Network".to_owned(), true),
            ("Guest".to_owned(), false)
        ]
    );
    assert!(
        snapshot
            .accessibility
            .iter()
            .all(|(_, n)| n.name.as_deref() != Some("Bluetooth")),
        "the other tiles are gone once the view is open"
    );
    for (role, name) in [
        (Role::ListItem, "Guest"),
        (Role::Button, "Wi-Fi Settings…"),
        (Role::Switch, "Wi-Fi"),
    ] {
        let at = run.bounds(role, name).center();
        click(&mut run, at);
    }
    assert_eq!(
        run.heard(),
        [
            Input::Join("Guest".to_owned()),
            Input::Settings(Detail::WiFi),
            Input::Press(Module::WiFi),
        ]
    );
}

#[test]
fn the_keyboard_reaches_every_tile() {
    let mut run = Run::shown();
    run.key(TAB);
    run.key(SPACE);
    assert_eq!(
        run.heard(),
        [Input::Press(Module::WiFi)],
        "Tab starts on Wi-Fi"
    );
    run.key(DOWN);
    run.key(RIGHT);
    run.key(SPACE);
    assert_eq!(
        run.heard()[1..],
        [
            Input::Focus(view::focus_key(Module::Bluetooth)),
            Input::Focus(format!("{}.more", view::focus_key(Module::Bluetooth))),
            Input::Expand(Detail::Bluetooth, Via::Keyboard),
        ]
    );
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let focusable = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.states.focusable)
        .count();
    assert_eq!(
        focusable, 15,
        "every enabled control is a Tab stop: four badges, four view openers, three circles (the keyboard's has no \
         backend), two levels, the output button and Edit Controls; nothing plays, so no media button"
    );
}

#[test]
fn a_left_or_right_press_on_the_catcher_dismisses_the_panel() {
    for button in [MouseButton::Left, MouseButton::Right] {
        let mut run = Run::catcher();
        let at = Point::new(40.0, 250.0);
        run.pointer(PointerEvent::Motion { position: at });
        run.press(at, button, true);
        assert_eq!(run.heard(), [Input::Dismiss], "{button:?}");
        run.press(at, button, false);
        assert_eq!(
            run.heard(),
            [Input::Dismiss],
            "{button:?}: the release says nothing"
        );
    }
}

#[test]
fn other_buttons_and_the_pointer_moving_over_the_catcher_do_not() {
    let mut run = Run::catcher();
    let at = Point::new(40.0, 250.0);
    run.pointer(PointerEvent::Motion { position: at });
    for button in [MouseButton::Middle, MouseButton::Back, MouseButton::Forward] {
        run.press(at, button, true);
        run.press(at, button, false);
    }
    run.pointer(PointerEvent::Leave);
    assert_eq!(run.heard(), []);
}

/// The catcher takes the output's size and is shown as one transparent colour without a frame: the compositor
/// stretches one pixel over the output, so it costs no frame of the output's size, and it still takes the presses.
#[test]
fn the_catcher_covers_the_output_and_draws_nothing() {
    let shown = view::catcher::<()>(|_| ());
    assert_eq!(shown.spec.app_id, view::CATCHER_APP_ID);
    assert_eq!(
        shown.spec.size,
        Size::ZERO,
        "the layer surface takes the output's size"
    );
    assert_eq!(shown.spec.input_region, None, "all of it takes input");
    assert_eq!(shown.background, WindowBackground::INPUT_ONLY);

    let run = Run::catcher();
    let system = run.driver.runtime().window_system();
    assert!(system.frames(run.window).unwrap().is_empty(), "no frame");
    let solids = system.solids(run.window).unwrap();
    assert_eq!(solids.len(), 1);
    assert_eq!(solids[0].color.a, 0.0, "transparent");
    assert!(solids[0].acknowledged.is_some(), "it answers the configure");
}

/// The panel with its controls being edited and no tile held.
const EDITING: Presentation = Presentation {
    alpha: 1.0,
    light: false,
    input: true,
    detail: None,
    edit: Some(Editing { held: None }),
};

const DELETE: u32 = 0xffff;
const LEFT: u32 = 0xff51;

/// The frame of a rect of the window from the window's corner, past the shadow's margin.
fn within(r: Rect) -> (f64, f64, f64, f64) {
    let (x, y, w, h) = frame(r);
    (x - ORIGIN.x, y - ORIGIN.y, w, h)
}

/// The panel in the dark appearance with its controls being edited.
fn editing_run() -> Run {
    Run::start(tokens(Appearance::Dark, false), EDITING)
}

/// Some controls shown, in an order of their own, and three left out.
fn some_controls() -> Arrangement {
    Arrangement::from_ids(&[
        "wifi",
        "now-playing",
        "microphone",
        "lock-screen",
        "dock-hiding",
        "focus",
        "display",
        "sound",
    ])
}

/// Tab until the control named `name` holds the focus.
fn tab_to(run: &mut Run, name: &str) {
    for _ in 0..40 {
        if run.focused().as_deref() == Some(name) {
            return;
        }
        run.key(TAB);
    }
    panic!(
        "Tab never reaches {name}; it stopped on {:?}",
        run.focused()
    );
}

#[test]
fn edit_controls_starts_editing_on_the_panel() {
    let mut run = Run::shown();
    let edit = run.bounds(Role::Button, "Edit Controls").center();
    click(&mut run, edit);
    assert_eq!(run.heard(), [Input::EditControls]);
    assert!(
        run.node(Role::Button, "Edit Controls").is_none(),
        "Cancel and Done take its place"
    );
    assert_eq!(
        within(run.bounds(Role::Button, "Cancel")),
        (92.5, 468.0, 62.0, 24.0)
    );
    assert_eq!(
        within(run.bounds(Role::Button, "Done")),
        (166.5, 468.0, 49.0, 24.0)
    );
    let count = Module::ALL.len();
    for (index, module) in Module::ALL.into_iter().enumerate() {
        let title = module.title();
        let handle = run
            .node(Role::Slider, title)
            .unwrap_or_else(|| panic!("{title} can be moved"));
        assert!(handle.states.focusable, "{title}");
        assert_eq!(
            handle.value,
            Some(AccessibleValue::Number {
                value: (index + 1) as f64,
                min: 1.0,
                max: count as f64,
                step: Some(1.0)
            }),
            "{title}: its place in the order"
        );
        assert_eq!(
            handle.description,
            Some(format!(
                "Place {} of {count}. The arrow keys move it, Delete removes it.",
                index + 1
            ))
        );
        assert_eq!(
            (
                handle.actions.contains(&Action::Decrement),
                handle.actions.contains(&Action::Increment)
            ),
            (index > 0, index + 1 < count),
            "{title}: earlier and later, where there is such a place"
        );
        assert_eq!(
            handle.bounds,
            run.bounds(Role::Group, title),
            "{title}: the whole tile moves it"
        );
        let remove = run
            .node(Role::Button, &format!("Remove {title}"))
            .unwrap_or_else(|| panic!("{title} can be removed"));
        assert!(remove.states.focusable && remove.actions.contains(&Action::Click));
        let (x, y, w, h) = frame(remove.bounds);
        // on the leading corner; on the trailing one of the wide tiles, whose title starts on the leading one
        let leading = match module {
            Module::Display | Module::Sound => 292.0 - 4.0 - 18.0,
            _ => 4.0,
        };
        assert_eq!(
            (x - handle.bounds.x(), y - handle.bounds.y(), w, h),
            (leading, 4.0, 18.0, 18.0),
            "{title}: the mark on its corner"
        );
    }
    // a tile's own controls show and are no controls while the tiles are edited
    for (role, name) in [
        (Role::Switch, "Wi-Fi"),
        (Role::Button, "Bluetooth Devices"),
        (Role::Switch, "Dock Hiding"),
        (Role::Button, "Lock Screen"),
        (Role::Button, "Output"),
    ] {
        assert!(run.node(role, name).is_none(), "{name}");
    }
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let focusable = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.states.focusable)
        .count();
    assert_eq!(
        focusable,
        2 * count + 2,
        "each tile and its mark, Cancel and Done"
    );
}

#[test]
fn the_keyboard_starts_editing_with_the_focus_staying_where_done_is() {
    let mut run = Run::shown();
    tab_to(&mut run, "Edit Controls");
    run.key(SPACE);
    assert_eq!(run.heard(), [Input::EditControls]);
    assert_eq!(run.focused().as_deref(), Some("Done"));
    run.key(SPACE);
    assert_eq!(run.heard()[1..], [Input::Done]);
    assert_eq!(run.focused().as_deref(), Some("Edit Controls"));
}

#[test]
fn a_tile_s_mark_removes_it_and_its_chip_adds_it_again() {
    let mut run = editing_run();
    assert!(
        run.node(Role::Button, "Add Bluetooth").is_none(),
        "every control shows: none is offered"
    );
    let mark = run.bounds(Role::Button, "Remove Bluetooth").center();
    click(&mut run, mark);
    assert_eq!(
        run.heard(),
        [Input::Remove(Module::Bluetooth, Via::Pointer)]
    );
    assert!(run.node(Role::Slider, "Bluetooth").is_none());
    assert_eq!(
        within(run.bounds(Role::Group, "Microphone")),
        (8.0, 84.0, 140.0, 64.0),
        "the tiles close up"
    );
    // the control is offered under Cancel and Done, a chip centred on the grid
    let chip = run.bounds(Role::Button, "Add Bluetooth");
    assert_eq!(within(chip), (136.0, 508.0, 36.0, 36.0));
    assert_eq!(
        frame(chip),
        frame(editing::chip(0, 1, 444.0).translate(GRID_ORIGIN.x, GRID_ORIGIN.y))
    );
    click(&mut run, chip.center());
    assert_eq!(
        run.heard()[1..],
        [Input::Add(Module::Bluetooth, Via::Pointer)]
    );
    assert_eq!(run.order().last(), Some(&Module::Bluetooth));
    assert!(run.node(Role::Button, "Add Bluetooth").is_none());
    assert!(run.node(Role::Slider, "Bluetooth").is_some());
}

#[test]
fn the_window_takes_presses_on_the_controls_offered_under_it() {
    let arrangement = Arrangement::from_ids(&["wifi", "sound"]);
    LAYOUT.set(Some(arrangement.clone()));
    let offered = Module::ALL.len() - 2;
    let bottom = GRID_ORIGIN.y + editing::bottom(offered, arrangement.height());
    let region = |presentation| {
        view::arranged::<()>(&controls(), &arrangement, presentation, |_| ())
            .spec
            .input_region
            .unwrap()
    };
    let window = Rect::from_origin_size(ORIGIN, Size::new(308.0, 500.0));
    assert_eq!(region(Presentation::SHOWN), [window]);
    let reach = region(EDITING);
    assert_eq!(reach.len(), 1);
    assert!(reach[0].max_y() >= bottom && reach[0].max_y() <= SURFACE.height);
    // six rows of tiles and a row of chips reach under the window, into the margin its shadow takes
    let mut tall = Arrangement::default();
    tall.remove(Module::Mute);
    let under = view::arranged::<()>(&controls(), &tall, EDITING, |_| ())
        .spec
        .input_region
        .unwrap();
    assert!(under[0].max_y() > window.max_y() && under[0].max_y() <= SURFACE.height);
    let mut run = editing_run();
    let chips = arrangement
        .available()
        .into_iter()
        .map(|m| run.bounds(Role::Button, &format!("Add {}", m.title())))
        .collect::<Vec<_>>();
    assert_eq!(chips.len(), offered);
    for chip in &chips {
        assert!(reach[0].contains(chip.center()), "{chip:?} in {reach:?}");
    }
    click(&mut run, chips[8].center());
    assert_eq!(run.heard(), [Input::Add(Module::Display, Via::Pointer)]);
}

#[test]
fn a_drag_carries_a_tile_and_the_others_make_room() {
    let mut run = editing_run();
    let sound = run.bounds(Role::Group, "Sound");
    let display = run.bounds(Role::Group, "Display");
    let from = Point::new(sound.x() + 100.0, sound.y() + 30.0);
    run.pointer(PointerEvent::Motion { position: from });
    run.button(from, true);
    run.pointer(PointerEvent::Motion {
        position: Point::new(from.x + 1.0, from.y + 1.0),
    });
    assert!(run.heard().is_empty(), "a small move holds nothing");
    let to = Point::new(from.x + 2.0, from.y - 70.0);
    run.pointer(PointerEvent::Motion { position: to });
    let held = Point::new(
        sound.x() - GRID_ORIGIN.x + 2.0,
        sound.y() - GRID_ORIGIN.y - 70.0,
    );
    assert_eq!(run.heard(), [Input::DragTo(Module::Sound, held)]);
    assert_eq!(run.order()[9..], [Module::Sound, Module::Display]);
    assert_eq!(
        frame(run.bounds(Role::Group, "Sound")),
        frame(sound.translate(2.0, -70.0)),
        "the held tile follows the pointer"
    );
    assert_eq!(
        frame(run.bounds(Role::Group, "Display")),
        frame(sound),
        "the display tile makes room"
    );
    run.button(to, false);
    assert_eq!(run.heard()[1..], [Input::Drop]);
    assert_eq!(
        frame(run.bounds(Role::Group, "Sound")),
        frame(display),
        "let go, the tile takes its place"
    );
    // a press and release without a move holds and drops nothing
    click(&mut run, to);
    assert_eq!(run.heard().len(), 2);
}

#[test]
fn the_keyboard_moves_removes_and_adds_a_tile() {
    let mut run = editing_run();
    run.key(TAB);
    assert_eq!(run.focused().as_deref(), Some("Wi-Fi"));
    run.key(RIGHT);
    assert_eq!(run.heard(), [Input::Step(Module::WiFi, true)]);
    assert_eq!(run.order()[..2], [Module::NowPlaying, Module::WiFi]);
    assert_eq!(
        run.focused().as_deref(),
        Some("Wi-Fi"),
        "the focus moves with the tile"
    );
    run.key(DOWN);
    run.key(LEFT);
    run.key(LEFT);
    assert_eq!(run.order(), Module::ALL, "and back");
    run.key(DELETE);
    assert_eq!(
        run.heard().last(),
        Some(&Input::Remove(Module::WiFi, Via::Keyboard))
    );
    assert_eq!(
        run.focused().as_deref(),
        Some("Now Playing"),
        "the focus goes to the tile that took the place"
    );
    tab_to(&mut run, "Add Wi-Fi");
    run.key(SPACE);
    assert_eq!(
        run.heard()[run.heard().len() - 2..],
        [
            Input::Add(Module::WiFi, Via::Keyboard),
            Input::Focus(view::edit_key(Module::WiFi))
        ],
        "the focus is asked onto the new tile once it is there"
    );
    assert_eq!(run.focused().as_deref(), Some("Wi-Fi"));
    assert_eq!(run.order().last(), Some(&Module::WiFi));
    // the mark is a control of its own for the keyboard too
    tab_to(&mut run, "Remove Wi-Fi");
    run.key(SPACE);
    assert_eq!(
        run.heard().last(),
        Some(&Input::Remove(Module::WiFi, Via::Keyboard))
    );
}

#[test]
fn a_screen_reader_moves_removes_and_adds_a_tile() {
    let mut run = editing_run();
    run.ask(Role::Slider, "Sound", Action::Decrement);
    assert_eq!(run.heard(), [Input::Step(Module::Sound, false)]);
    assert_eq!(run.order()[9..], [Module::Sound, Module::Display]);
    run.ask(Role::Slider, "Sound", Action::Increment);
    assert_eq!(run.order(), Module::ALL);
    run.ask(Role::Button, "Remove Mute", Action::Click);
    assert_eq!(
        run.heard().last(),
        Some(&Input::Remove(Module::Mute, Via::Keyboard))
    );
    run.ask(Role::Button, "Add Mute", Action::Click);
    assert!(run
        .heard()
        .contains(&Input::Add(Module::Mute, Via::Keyboard)));
    assert_eq!(run.order().last(), Some(&Module::Mute));
    run.ask(Role::Button, "Done", Action::Click);
    assert_eq!(run.heard().last(), Some(&Input::Done));
}

#[test]
fn done_cancel_and_escape_end_editing_and_a_press_between_the_tiles_does_not() {
    let mut run = editing_run();
    // between the first two rows of tiles
    click(&mut run, Point::new(ORIGIN.x + 80.0, ORIGIN.y + 78.0));
    assert!(
        run.heard().is_empty(),
        "no edit is lost to a press that misses"
    );
    run.key(ESCAPE);
    assert_eq!(run.heard(), [Input::Cancel], "Escape cancels the edit");
    assert!(run.node(Role::Button, "Edit Controls").is_some());
    for (name, input) in [("Cancel", Input::Cancel), ("Done", Input::Done)] {
        let edit = run.bounds(Role::Button, "Edit Controls").center();
        click(&mut run, edit);
        let button = run.bounds(Role::Button, name).center();
        click(&mut run, button);
        assert_eq!(run.heard().last(), Some(&input));
        assert!(run.node(Role::Button, name).is_none());
    }
    // not editing, Escape dismisses the panel as before
    run.key(ESCAPE);
    assert_eq!(run.heard().last(), Some(&Input::Dismiss));
}

#[test]
fn the_panel_shows_the_arrangement_it_is_given() {
    LAYOUT.set(Some(Arrangement::from_ids(&["sound", "wifi", "mute"])));
    let mut run = Run::shown();
    let group = |run: &Run, name| within(run.bounds(Role::Group, name));
    assert_eq!(group(&run, "Sound"), (8.0, 8.0, 292.0, 64.0));
    assert_eq!(group(&run, "Wi-Fi"), (8.0, 84.0, 140.0, 64.0));
    assert_eq!(group(&run, "Mute"), (160.0, 84.0, 64.0, 64.0));
    assert_eq!(
        group(&run, "Edit Controls"),
        (107.0, 164.0, 94.0, 24.0),
        "16 below the last row of tiles"
    );
    for hidden in ["Bluetooth", "Display", "Now Playing"] {
        assert!(run.node(Role::Group, hidden).is_none(), "{hidden}");
    }
    assert_eq!(
        glass_regions(&mut run).len(),
        4,
        "three tiles and Edit Controls"
    );
}

#[test]
fn the_editing_panel_is_glass_as_the_tiles_are() {
    LAYOUT.set(Some(Arrangement::from_ids(&[
        "wifi",
        "now-playing",
        "display",
    ])));
    let mut run = editing_run();
    let requests = glass_requests(&mut run);
    assert_eq!(
        requests.len(),
        3 + 2 + 8,
        "the tiles, Cancel and Done, and a chip for each control offered"
    );
    let first = requests[0];
    assert!(
        requests.iter().all(|r| r.2 == first.2 && r.1 == first.1),
        "one container of module glass: {requests:?}"
    );
}

#[test]
fn the_editing_panel_matches_its_golden() {
    LAYOUT.set(Some(some_controls()));
    check("editing", Appearance::Dark, false, EDITING);
}

#[test]
fn the_editing_panel_under_reduce_transparency_matches_its_golden() {
    LAYOUT.set(Some(some_controls()));
    check(
        "editing-reduce-transparency",
        Appearance::Light,
        true,
        EDITING,
    );
}

#[test]
fn the_editing_panel_under_increase_contrast_matches_its_golden() {
    LAYOUT.set(Some(some_controls()));
    check(
        "editing-increase-contrast",
        Appearance::DarkHighContrast,
        false,
        EDITING,
    );
}

/// The marks stand out from the tiles where the tiles are opaque: white on the dark tiles, dark on the light ones, with
/// a sign of the other tone, with Reduce Transparency in either appearance and with Increase Contrast.
#[test]
fn the_marks_stand_out_from_opaque_tiles_in_either_appearance() {
    LAYOUT.set(Some(some_controls()));
    let luma = |frame: &Pixmap, x: f64, y: f64| {
        let pixel = frame.pixel(x as u32, y as u32).unwrap();
        assert_eq!(pixel.alpha(), 255, "opaque at ({x}, {y})");
        f64::from(pixel.red())
    };
    for (appearance, reduce_transparency, name) in [
        (Appearance::Dark, true, "dark, Reduce Transparency"),
        (Appearance::Light, true, "light, Reduce Transparency"),
    ] {
        let mut run = Run::start(tokens(appearance, reduce_transparency), EDITING);
        let remove = run.bounds(Role::Button, "Remove Wi-Fi");
        let add = run.bounds(Role::Button, "Add Bluetooth");
        let (frame, _) = run.capture();
        let centre = remove.center();
        // the disc above its bar, the bar, and the tile beside the mark
        let disc = luma(&frame, centre.x, centre.y - 5.0);
        let sign = luma(&frame, centre.x, centre.y);
        let tile = luma(&frame, remove.max_x() + 6.0, centre.y - 5.0);
        assert!((disc - tile).abs() > 150.0, "{name}: disc {disc} on {tile}");
        assert!((disc - sign).abs() > 150.0, "{name}: sign {sign} on {disc}");
        // the chip's mark: its disc beside the cross, against the chip
        let disc = luma(&frame, add.x() + 3.0, add.y() + 4.0);
        let chip = luma(&frame, add.center().x, add.max_y() - 4.0);
        assert!((disc - chip).abs() > 150.0, "{name}: disc {disc} on {chip}");
    }
}

/// With `CUPERTINO_LOOK_DIR` set, the editing panel with some controls offered and a tile held, for review by eye.
#[test]
fn the_editing_panel_can_be_looked_at() {
    LAYOUT.set(Some(some_controls()));
    let held = Presentation {
        edit: Some(Editing {
            held: Some(Held {
                module: Module::FocusMode,
                origin: Point::new(120.0, 190.0),
            }),
        }),
        ..EDITING
    };
    for (appearance, reduce_transparency, presentation, name) in [
        (Appearance::Dark, false, EDITING, "controlcenter-editing"),
        (Appearance::Dark, true, EDITING, "controlcenter-editing-rt"),
        (
            Appearance::Light,
            true,
            EDITING,
            "controlcenter-editing-rt-light",
        ),
        (
            Appearance::DarkHighContrast,
            false,
            EDITING,
            "controlcenter-editing-ic",
        ),
        (Appearance::Dark, false, held, "controlcenter-editing-held"),
    ] {
        let mut run = Run::start(tokens(appearance, reduce_transparency), presentation);
        run.driver.look(run.window, name).unwrap();
    }
}

/// The rims the tiles draw with Increase Contrast on a display of `scale`: each stroke's rectangle and width.
fn contrast_rims(scale: f64) -> Vec<(Rect, f64)> {
    use cupertino_kit::render::{PaintOp, Shape};
    let mut run = Run::start(
        tokens(Appearance::DarkHighContrast, false),
        Presentation::SHOWN,
    );
    run.send(WindowEvent::Scale(scale));
    run.settle();
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    snapshot
        .display_list
        .nodes
        .iter()
        .flat_map(|n| n.ops.iter())
        .filter_map(|op| match op {
            PaintOp::Stroke {
                shape: Shape::RoundedRect { rect, .. },
                width,
                ..
            } => Some((*rect, *width)),
            _ => None,
        })
        .collect()
}

/// With Increase Contrast every tile's rim is one whole device pixel wide, lying on the outermost pixel of the tile: half
/// a point where a point is two pixels, and a whole point where it is one, where half a point would cover half of each
/// pixel and draw the line at half its strength.
#[test]
fn a_tile_s_contrast_rim_is_one_whole_device_pixel_at_one_and_at_two_pixels_a_point() {
    for (scale, width) in [(1.0, 1.0), (2.0, 0.5)] {
        let rims: Vec<(Rect, f64)> = contrast_rims(scale)
            .into_iter()
            .filter(|(rect, _)| rect.width() > 40.0)
            .collect();
        assert!(rims.len() >= 6, "{scale}: a rim a tile: {rims:?}");
        for (rect, w) in rims {
            assert_eq!(w, width, "{scale}: one device pixel");
            assert_eq!(
                (rect.min_x(), rect.min_y()),
                (width / 2.0, width / 2.0),
                "{scale}: centred on the tile's outermost pixel"
            );
        }
    }
}
