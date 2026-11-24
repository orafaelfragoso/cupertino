//! The desktops band: its material lies in a window of its own over the output's top edge, as tall as the band at its
//! tallest and taking no input, which the application overview has none of; where the window system shows no
//! materials the window paints the material's veil across the band and the current desktop's pill over it, and
//! nothing below. The overview's own window no longer tints the band, and its names and plus are dark in the light
//! appearance and white in the dark one.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{Application, Command, Key, RuntimeOptions, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{Timestamp, WindowId};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::capture::{presented, Pixmap};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_shell_overview::chrome::{self, HolePlace, Scene};
use cupertino_shell_overview::layout::{Mode, OverviewUi, WindowEntry, Workspace};
use cupertino_wm::geometry::Rect;

/// The runtime turns between two looks at the windows.
const TURNS: u32 = 1000;
/// The output, and the area the windows are laid out in.
const OUTPUT: Size = Size::new(1000.0, 700.0);
const AREA: Rect = Rect::new(0.0, 0.0, 1000.0, 640.0);
/// The clock the windows are drawn at: everything at rest.
const NOW: f64 = 10.0;

fn ui(mode: Mode) -> OverviewUi {
    let windows = [WindowEntry {
        id: 1,
        frame: Rect::new(200.0, 200.0, 500.0, 300.0),
        title: "Notes".to_owned(),
        minimized: false,
    }];
    let workspaces = vec![
        Workspace {
            id: 1,
            current: true,
        },
        Workspace {
            id: 2,
            current: false,
        },
    ];
    OverviewUi::new(mode, AREA, &windows, workspaces)
}

thread_local! {
    static SHOWN: RefCell<Option<OverviewUi>> = const { RefCell::new(None) };
}

/// The band's window and the overview's, of the overview in [`SHOWN`].
struct Shown;

impl Application for Shown {
    type Model = Option<OverviewUi>;
    type Message = ();

    fn init() -> (Option<OverviewUi>, Vec<Command<()>>) {
        (SHOWN.with(|s| s.borrow_mut().take()), Vec::new())
    }

    fn update(_: &mut Option<OverviewUi>, (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(ui: &Option<OverviewUi>) -> Vec<Window<()>> {
        let Some(ui) = ui else {
            return Vec::new();
        };
        let band = chrome::band_window(WindowKey(Key::from("band")), OUTPUT.width, ui, NOW);
        let scene = Scene {
            key: WindowKey(Key::from("overview")),
            size: OUTPUT,
            ui,
            holes: ui
                .frames()
                .into_iter()
                .map(|(id, rect)| HolePlace { id, rect })
                .collect(),
            reduce_motion: false,
            hole_fill: None,
            chrome_opacity: 1.0,
            band_in_place: false,
        };
        band.into_iter()
            .chain([chrome::window(&scene, NOW, |_| (), None)])
            .collect()
    }
}

fn options(dark: bool) -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance: if dark {
                    Appearance::Dark
                } else {
                    Appearance::Light
                },
                reduce_transparency: false,
                reduce_motion: false,
            })
            .unwrap(),
        ),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 1000,
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
    }
}

/// The overview of `mode` drawn in the light or the dark appearance, on a window system without materials.
struct Drawn {
    driver: TestDriver<Shown>,
}

impl Drawn {
    fn new(mode: Mode, dark: bool) -> Self {
        SHOWN.with(|s| *s.borrow_mut() = Some(ui(mode)));
        let turns = NonZeroU32::new(TURNS).unwrap();
        let mut driver = TestDriver::<Shown>::new(
            Capabilities::default(),
            options(dark),
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(turns).unwrap();
        let windows: Vec<WindowId> = driver
            .runtime()
            .windows()
            .into_iter()
            .map(|w| w.1)
            .collect();
        for &window in &windows {
            driver
                .window_system_mut()
                .configure(window, None, WindowStates::default())
                .unwrap();
        }
        for _ in 0..16 {
            driver.run_until_idle(turns).unwrap();
            let system = driver.window_system_mut();
            let mut asked = false;
            for &window in &windows {
                if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, Timestamp(0)).unwrap();
                    asked = true;
                }
            }
            if !asked {
                break;
            }
        }
        Drawn { driver }
    }

    fn window(&self, key: &str) -> Option<WindowId> {
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .find(|(k, _)| k.0 == Key::from(key.to_owned()))
            .map(|(_, w)| w)
    }

    fn pixels(&self, key: &str) -> Pixmap {
        let window = self.window(key).expect("the window");
        presented(self.driver.runtime().window_system(), window).expect("a frame")
    }
}

/// The premultiplied colour of the pixel at `x`, `y`, from 0 to 1.
fn at(pixmap: &Pixmap, x: u32, y: u32) -> [f32; 4] {
    let p = pixmap.pixel(x, y).expect("inside the frame");
    [p.red(), p.green(), p.blue(), p.alpha()].map(|c| f32::from(c) / 255.0)
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.02)
}

#[test]
fn the_band_has_a_window_of_its_own_that_takes_no_input() {
    let all = ui(Mode::All);
    let window: Window<()> =
        chrome::band_window(WindowKey(Key::from("band")), OUTPUT.width, &all, NOW)
            .expect("the overview of every window has a band");
    let tallest = all.band_bottom(NOW).expect("a band");
    assert!(
        tallest > all.bar(NOW).expect("a band").rect.h,
        "room for the open band"
    );
    assert_eq!(window.spec.size, Size::new(OUTPUT.width, tallest));
    assert_eq!(window.spec.input_region, Some(Vec::new()));
    assert_eq!(window.spec.app_id, chrome::BAND_APP_ID);
    let app = ui(Mode::App);
    assert!(
        chrome::band_window::<()>(WindowKey(Key::from("band")), OUTPUT.width, &app, NOW).is_none()
    );
    assert!(app.band_bottom(NOW).is_none());
}

#[test]
fn without_materials_the_band_window_paints_the_veil_and_the_pill() {
    for (dark, veil) in [(false, 0.9646 * 0.36), (true, 0.1569 * 0.4)] {
        let drawn = Drawn::new(Mode::All, dark);
        let band = drawn.pixels("band");
        let ui = ui(Mode::All);
        let bar = ui.bar(NOW).unwrap();
        // the band, away from the names and the plus
        let a = if dark { 0.4 } else { 0.36 };
        let expected = [veil, veil, veil, a];
        assert!(
            near(at(&band, 20, 20), expected),
            "{dark}: {:?}",
            at(&band, 20, 20)
        );
        // nothing below it
        let below = (bar.rect.bottom() + 4.0) as u32;
        assert_eq!(at(&band, 20, below)[3], 0.0, "{dark}");
        // the current desktop's pill over it: darker in the light appearance, lighter in the dark one
        let current = bar.slots.iter().find(|s| s.current).unwrap();
        let (px, py) = (current.name.cx() as u32, (current.name.y + 3.0) as u32);
        let pill = at(&band, px, py);
        assert!(pill[3] > a, "{dark}: the pill covers more than the veil");
        let other = bar.slots.iter().find(|s| !s.current).unwrap();
        let bare = at(&band, other.name.cx() as u32, py);
        assert!(
            near(bare, expected),
            "{dark}: no pill under another desktop's name"
        );
    }
}

#[test]
fn the_overview_window_leaves_the_band_to_its_window_and_draws_the_plus_by_appearance() {
    for dark in [false, true] {
        let drawn = Drawn::new(Mode::All, dark);
        let overview = drawn.pixels("overview");
        assert_eq!(
            at(&overview, 20, 20)[3],
            0.0,
            "{dark}: no tint over the band"
        );
        let add = ui(Mode::All).bar(NOW).unwrap().add;
        let plus = at(&overview, add.cx() as u32, add.cy() as u32);
        assert!(plus[3] > 0.8, "{dark}: {plus:?}");
        let level = plus[0] / plus[3];
        if dark {
            assert!(level > 0.9, "white in the dark appearance: {plus:?}");
        } else {
            assert!(level < 0.1, "dark in the light appearance: {plus:?}");
        }
    }
}
