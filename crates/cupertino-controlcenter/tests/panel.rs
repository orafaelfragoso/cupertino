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
use cupertino_controlcenter::model::{
    Controls, Detail, Device, MediaCommand, Module, Network, Output, Reading, Slider, VolumeState,
    WifiStatus,
};
use cupertino_controlcenter::view::{self, Input, Opened, Presentation, Via};
use cupertino_kit::core::{
    Application, Command, Key, PlaceholderIcons, Role, RuntimeOptions, Window, WindowBackground,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowRequest, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::glass::adapts_foreground;
use cupertino_theme::Theme;
use cupertino_ui_test::{Inspector, ManualClock, TestDriver};
use tiny_skia::Pixmap;

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

/// The shown panel; it keeps what the user did on it and moves the focus where an arrow key asks.
struct Shown;

impl Application for Shown {
    type Model = (Controls, Vec<Input>);
    type Message = Input;

    fn init() -> (Self::Model, Vec<Command<Input>>) {
        let controls = CONTROLS.with_borrow(|c| c.clone()).unwrap_or_else(controls);
        ((controls, Vec::new()), Vec::new())
    }

    fn update((_, heard): &mut Self::Model, input: Input) -> Vec<Command<Input>> {
        heard.push(input.clone());
        match input {
            Input::Focus(key) => vec![Command::Focus(Key::from(key))],
            _ => Vec::new(),
        }
    }

    fn view((controls, _): &Self::Model) -> Vec<Window<Input>> {
        if CATCHER.get() {
            vec![view::catcher(|input| input)]
        } else {
            vec![view::panel(controls, PRESENTATION.get(), |input| input)]
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
            accessibility: None,
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
        let mut run = Self { driver, window };
        run.settle();
        run
    }

    /// Runs until every window rests, then checks that a screen reader could read every window open.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Control Center");
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
        &self.driver.runtime().model().1
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
        (308, 500),
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

/// A point on the media tile's glass, clear of its artwork, text and buttons: near its top-right corner.
const MEDIA_GLASS: (u32, u32) = (280, 30);

#[test]
fn a_light_session_darkens_the_glass_by_about_a_quarter() {
    let light = Presentation {
        light: true,
        ..Presentation::SHOWN
    };
    let (frame, _) = Run::start(tokens(Appearance::Dark, false), light).capture();
    let tint = frame.pixel(MEDIA_GLASS.0, MEDIA_GLASS.1).unwrap();
    assert_eq!((tint.red(), tint.green(), tint.blue()), (0, 0, 0));
    assert!(
        (60..=78).contains(&tint.alpha()),
        "a quarter black keeps a tile over a white page mid-light grey, not dark: alpha {}",
        tint.alpha()
    );
    let (dark, _) = Run::start(tokens(Appearance::Dark, false), Presentation::SHOWN).capture();
    assert_eq!(
        dark.pixel(MEDIA_GLASS.0, MEDIA_GLASS.1).unwrap().alpha(),
        0,
        "the dark session draws the glass alone"
    );
}

/// A tile's state is set in the theme's caption size under its title in the body size, and drawn quieter: the title
/// in white at 0.85, the state at 0.70, so the state reads as a second line rather than a second title. The slider
/// tiles' titles and Edit Controls are full white, in the body size. Increase Contrast draws the theme's own colours.
#[test]
fn a_tile_s_state_is_smaller_and_quieter_than_its_title() {
    let run = Run::shown();
    let alpha = |c: cupertino_kit::theme::Color| (c.a * 100.0).round() / 100.0;
    for (title, state) in [("Wi-Fi", "Home Network"), ("Bluetooth", "Off")] {
        let (title_size, title_color) = run.glyphs(title);
        let (state_size, state_color) = run.glyphs(state);
        assert_eq!((title_size, state_size), (13.0, 11.0), "{title}");
        assert_eq!(
            (alpha(title_color), alpha(state_color)),
            (0.85, 0.7),
            "{title}"
        );
    }
    for full in ["Display", "Sound", "Edit Controls"] {
        let (size, color) = run.glyphs(full);
        assert_eq!((size, alpha(color)), (13.0, 1.0), "{full}");
    }
    assert_eq!(alpha(run.glyphs("Not Playing").1), 0.85);
    assert_eq!(alpha(run.glyphs("Do Not Disturb").1), 0.85);

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
/// own tint in a light session, which the compositor does not see, so every tile joins one container that is never
/// small: at rest, in a light session, while a view opens or cross-fades, and with it open.
#[test]
fn the_tiles_are_one_glass_too_large_for_their_text_to_follow_the_backdrop() {
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
        let containers: Vec<u32> = regions
            .iter()
            .map(|r| r.glass.expect("every tile is glass").container)
            .collect();
        assert!(
            containers.iter().all(|c| *c != 0 && *c == containers[0]),
            "{presentation:?}: the tiles join one container: {containers:?}"
        );
        let bounds = regions
            .iter()
            .map(|r| r.rect)
            .reduce(|a, b| a.union(&b))
            .unwrap();
        let short = bounds.width().min(bounds.height());
        assert!(
            !adapts_foreground(short as f32),
            "{presentation:?}: the container's short side, {short} pt, lets its text follow the backdrop"
        );
    }
}

/// The glass regions the panel last asked for.
fn glass_regions(run: &mut Run) -> Vec<Rect> {
    let requests = run.driver.window_system_mut().requests(run.window).unwrap();
    requests
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(regions) => Some(regions.iter().map(|r| r.rect).collect()),
            _ => None,
        })
        .expect("the panel asks for glass")
}

/// Edit Controls is glass as the tiles are, in a dark and in a light session: the capsule takes its colour from what lies
/// behind it (a light blue-grey over a light part of the wallpaper), with the tiles' rim, rather than a flat fill of its
/// own.
#[test]
fn edit_controls_is_glass_as_the_tiles_are() {
    let capsule = Rect::new(107.0, 468.0, 94.0, 24.0);
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
        let pixel = frame.pixel(x, y).unwrap();
        assert_eq!(pixel.alpha(), 255, "opaque at ({x}, {y})");
        assert!(
            pixel.red().abs_diff(77) <= 2,
            "the panel's grey at ({x}, {y}): {}",
            pixel.red()
        );
    }
    let capsule = frame.pixel(154, 470).unwrap();
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
    assert_eq!(shown.spec.size, Size::new(308.0, 500.0));
}

fn frame(r: Rect) -> (f64, f64, f64, f64) {
    (r.x(), r.y(), r.width(), r.height())
}

#[test]
fn the_tiles_sit_on_their_grid_eight_points_in() {
    let run = Run::shown();
    let group = |name| frame(run.bounds(Role::Group, name));
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
        frame(run.bounds(Role::Switch, "Wi-Fi")),
        (22.0, 22.0, 36.0, 36.0),
        "the badge is 36 across, 14 in from the capsule's edges"
    );
    assert_eq!(
        frame(run.bounds(Role::Slider, "Sound")),
        (37.0, 420.0, 188.0, 20.0),
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
fn the_circles_the_media_buttons_and_edit_controls_act() {
    let mut run = Run::shown();
    for (role, name) in [
        (Role::Button, "Lock Screen"),
        (Role::Switch, "Dock Hiding"),
        (Role::Button, "Edit Controls"),
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
            Input::EditControls,
            Input::Expand(Detail::Sound, Via::Pointer),
        ]
    );
    // nothing plays, so the media buttons are disabled
    let play = run.bounds(Role::Button, "Play").center();
    click(&mut run, play);
    assert_eq!(run.heard().len(), 4);
    let keyboard = run.bounds(Role::Switch, "Keyboard Brightness").center();
    click(&mut run, keyboard);
    assert_eq!(
        run.heard().len(),
        4,
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
    click(&mut run, Point::new(280.0, 30.0));
    assert_eq!(run.heard(), []);
    // the gap between the two columns
    click(&mut run, Point::new(154.0, 40.0));
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
