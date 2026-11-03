//! The card as CupertinoKit draws it, compared with `tests/goldens`: `<scene>.png` is the frame at 1x and `<scene>.txt`
//! the focus and the accessibility tree, for the shown panel, under Reduce Transparency and under Increase Contrast.
//! Pixels may differ by at most 2/255 per channel and are compared on x86_64 only, as the renderer's own baselines are.
//! `CUPERTINO_UPDATE_GOLDENS=1` re-records every scene it runs and then fails, so the variable can never pass a run.

use std::cell::Cell;
use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_controlcenter::model::{
    Battery, Controls, Module, Reading, Slider, VolumeState, WifiStatus,
};
use cupertino_controlcenter::view::{self, Input};
use cupertino_kit::core::{
    Application, Command, PlaceholderIcons, Role, RuntimeOptions, Window, WindowBackground,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, Surface, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{Inspector, ManualClock, TestDriver};
use tiny_skia::Pixmap;

const ESCAPE: u32 = 0xff1b;

thread_local! {
    static CONTROLS: Cell<bool> = const { Cell::new(true) };
}

/// Every backend answering, as `--fake-backends` does, with focus mode on; with or without a battery.
fn controls(battery: bool) -> Controls {
    Controls {
        wifi: Reading::Ready(WifiStatus {
            enabled: true,
            network: Some("Home Network".to_owned()),
        }),
        bluetooth: Reading::Ready(false),
        focus_mode: Reading::Ready(true),
        brightness: Reading::Ready(0.6),
        volume: Reading::Ready(VolumeState {
            level: 0.4,
            muted: false,
        }),
        battery: battery.then_some(Battery {
            percent: 80,
            charging: false,
        }),
    }
}

/// The shown card; it keeps what the user did on it.
struct Shown;

impl Application for Shown {
    type Model = (Controls, Vec<Input>);
    type Message = Input;

    fn init() -> (Self::Model, Vec<Command<Input>>) {
        ((controls(CONTROLS.get()), Vec::new()), Vec::new())
    }

    fn update((_, heard): &mut Self::Model, input: Input) -> Vec<Command<Input>> {
        heard.push(input);
        Vec::new()
    }

    fn view((controls, _): &Self::Model) -> Vec<Window<Input>> {
        vec![view::panel(controls, 1.0, |input| input)]
    }
}

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
    /// The card in `theme` on a desktop that offers materials, configured, active and drawn.
    fn start(theme: ThemeTokens, battery: bool) -> Self {
        CONTROLS.set(battery);
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 200,
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
            .configure(window, None, active)
            .unwrap();
        let mut run = Self { driver, window };
        run.settle();
        run
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
        panic!("the card keeps asking for frames");
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

fn check(scene: &str, appearance: Appearance, reduce_transparency: bool) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens");
    let (png, txt) = (format!("{dir}/{scene}.png"), format!("{dir}/{scene}.txt"));
    let (actual, text) = Run::start(tokens(appearance, reduce_transparency), true).capture();
    assert_eq!(
        (actual.width(), actual.height()),
        (298, 408),
        "{scene}: the card is drawn at 1x"
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
    check("shown", Appearance::Light, false);
}

#[test]
fn the_panel_under_reduce_transparency_matches_its_golden() {
    check("reduce-transparency", Appearance::Light, true);
}

#[test]
fn the_panel_under_increase_contrast_matches_its_golden() {
    check("increase-contrast", Appearance::LightHighContrast, false);
}

#[test]
fn the_card_has_the_menu_material_and_a_window_for_each_battery_case() {
    let shown = view::panel::<()>(&controls(true), 0.5, |_| ());
    assert_eq!(
        shown.background,
        WindowBackground::Surface {
            surface: Surface::Menu,
            radius: 16.0,
            alpha: 0.5
        }
    );
    assert_eq!(shown.spec.app_id, view::APP_ID);
    assert_eq!(shown.spec.size, Size::new(298.0, 408.0));
    let without = view::panel::<()>(&controls(false), 1.0, |_| ());
    assert_eq!(without.spec.size, Size::new(298.0, 370.0));
    assert_ne!(shown.key, without.key, "the surface keeps its first size");
}

#[test]
fn the_modules_sit_on_the_two_column_grid_below_the_battery_row() {
    let run = Run::start(tokens(Appearance::Light, false), true);
    let frame = |r: Rect| (r.x(), r.y(), r.width(), r.height());
    let switch = |name| frame(run.bounds(Role::Switch, name));
    let group = |name| frame(run.bounds(Role::Group, name));
    assert_eq!(switch("Wi-Fi"), (10.0, 48.0, 134.0, 40.0));
    assert_eq!(switch("Bluetooth"), (10.0, 95.0, 134.0, 40.0));
    assert_eq!(switch("File Sharing"), (10.0, 142.0, 134.0, 40.0));
    assert_eq!(group("Focus Mode"), (154.0, 48.0, 134.0, 62.0));
    assert_eq!(group("Stage"), (154.0, 120.0, 62.0, 62.0));
    assert_eq!(group("Mirroring"), (226.0, 120.0, 62.0, 62.0));
    assert_eq!(group("Display"), (10.0, 192.0, 278.0, 62.0));
    assert_eq!(group("Sound"), (10.0, 264.0, 278.0, 62.0));
    assert_eq!(group("Now Playing"), (10.0, 336.0, 278.0, 62.0));
    assert_eq!(
        frame(run.bounds(Role::Slider, "Display")),
        (22.0, 222.0, 254.0, 22.0),
        "the track is 22 high, 12 from the tile's sides and 10 above its bottom"
    );
}

#[test]
fn a_press_on_a_switch_asks_for_its_module() {
    let mut run = Run::start(tokens(Appearance::Light, false), false);
    let centre = run.bounds(Role::Switch, "Bluetooth").center();
    run.pointer(PointerEvent::Motion { position: centre });
    run.button(centre, true);
    run.button(centre, false);
    assert_eq!(run.heard(), [Input::Press(Module::Bluetooth)]);
}

#[test]
fn a_drag_slides_the_level_and_letting_go_releases_it() {
    let mut run = Run::start(tokens(Appearance::Light, false), false);
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
fn escape_dismisses_the_panel() {
    let mut run = Run::start(tokens(Appearance::Light, false), false);
    run.key(ESCAPE);
    assert_eq!(run.heard(), [Input::Dismiss]);
}
