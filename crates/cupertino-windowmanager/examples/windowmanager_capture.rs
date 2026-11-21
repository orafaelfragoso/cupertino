//! The stage strip drawn to PNGs for look checks:
//! `cargo run -p cupertino-windowmanager --example windowmanager_capture -- windowmanager/SURFACE [flags]`
//! (crates/cupertino-kit/README.md).
//!
//! The strip's surface on a 1280 x 800 pt output with the stage on, its thumbnails where the stage model rests them:
//! `strip`, three applications open, two of them in the strip; `full`, six applications open, the strip's rows full
//! and the oldest group hidden. A capture has no server: each hole is painted a flat grey where the server would draw
//! the window. Nothing moves, so a command gives the same pixels on every run.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_kit::core::{Application, Command, Key, Window, WindowKey};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_windowmanager::strip::{self, Scene, Thumb};
use cupertino_windowmanager::{Change, Controller, Window as Arranged};
use cupertino_wm::geometry::Rect as WmRect;
use cupertino_wm::stage::StageConfig;

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames the window may ask for before it rests.
const FRAMES: usize = 16;
/// The output's height; the strip's surface spans it.
const OUTPUT_HEIGHT: f64 = 800.0;
/// The area the windows are laid out in: the output below a 33 pt menu bar.
const USABLE: WmRect = WmRect::new(0.0, 33.0, 1280.0, 767.0);
/// The applications' windows, opened in this order: the last is the key window, on the stage.
const WINDOWS: [(&str, WmRect); 6] = [
    ("org.example.one", WmRect::new(300.0, 120.0, 700.0, 480.0)),
    ("org.example.two", WmRect::new(340.0, 140.0, 640.0, 440.0)),
    ("org.example.three", WmRect::new(260.0, 100.0, 760.0, 520.0)),
    ("org.example.four", WmRect::new(320.0, 160.0, 600.0, 400.0)),
    ("org.example.files", WmRect::new(280.0, 90.0, 720.0, 500.0)),
    ("org.example.notes", WmRect::new(360.0, 130.0, 660.0, 460.0)),
];
/// What a hole is painted in a capture.
const HOLE: [f32; 4] = [0.55, 0.57, 0.62, 1.0];
/// The model's clock at the capture: every swap at rest.
const NOW: f64 = 10.0;

thread_local! {
    static SHOWN: RefCell<Option<(Size, Vec<Thumb>)>> = const { RefCell::new(None) };
}

/// The strip's window showing [`SHOWN`].
struct Captured;

impl Application for Captured {
    type Model = Option<(Size, Vec<Thumb>)>;
    type Message = ();

    fn init() -> (Self::Model, Vec<Command<()>>) {
        (SHOWN.with(|s| s.borrow_mut().take()), Vec::new())
    }

    fn update(_: &mut Self::Model, (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(shown: &Self::Model) -> Vec<Window<()>> {
        let Some((size, thumbs)) = shown else {
            return Vec::new();
        };
        let scene = Scene {
            key: WindowKey(Key::from("strip")),
            size: *size,
            thumbs: thumbs.clone(),
            input: thumbs.iter().map(|t| t.rect).collect(),
            hole_fill: Some(HOLE),
        };
        vec![strip::window(&scene, |_| (), |_| ())]
    }
}

/// The strip's size and thumbnails with the first `open` of [`WINDOWS`] open, as the component lays them out.
fn shown(open: usize) -> (Size, Vec<Thumb>) {
    let mut controller = Controller::new(StageConfig::default(), true);
    let mut group = vec![Change::UsableArea(USABLE)];
    group.extend(
        WINDOWS[..open]
            .iter()
            .enumerate()
            .map(|(i, (app_id, frame))| {
                Change::Window(Arranged {
                    id: i as u32 + 1,
                    app_id: (*app_id).to_owned(),
                    frame: *frame,
                    active: i + 1 == open,
                })
            }),
    );
    controller.apply(&group, NOW);
    let column = controller.strip_column();
    let thumbs = controller
        .strip()
        .into_iter()
        .map(|(id, r)| Thumb {
            id,
            rect: Rect::new(r.x, r.y, r.w, r.h),
            name: WINDOWS[id as usize - 1].0.to_owned(),
            live: true,
        })
        .collect();
    (Size::new(column.right(), OUTPUT_HEIGHT), thumbs)
}

struct Capture;

impl App for Capture {
    const NAME: &'static str = "windowmanager";
    const STATE_FLAGS: &'static [&'static str] = &[];

    fn surfaces(&self) -> Vec<String> {
        ["strip", "full"].map(str::to_owned).to_vec()
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        vec![Vec::new()]
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let shown = match request.surface {
            "strip" => shown(3),
            "full" => shown(WINDOWS.len()),
            other => return Err(format!("no surface {other}")),
        };
        SHOWN.with(|s| *s.borrow_mut() = Some(shown));
        draw(request.look, request.scale)
    }
}

// a capture loads the theme of the appearance it draws, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn draw(look: Look, scale: f32) -> Result<Pixmap, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    let preferences = cupertino_kit::test::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
        steady_caret: true,
    });
    let theme = cupertino_assets::theme::load().map_err(|e| e.to_string())?;
    let tokens = ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?;
    options.theme = Arc::new(tokens);
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<Captured>::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    );
    driver
        .runtime_mut()
        .set_theme_for(Box::new(move |appearance: &SystemAppearance| {
            let preferences = cupertino_kit::test::preferences(appearance);
            Arc::new(ThemeTokens::cupertino(&theme, preferences).expect("the theme loaded once"))
        }));
    driver.run_until_idle(turns).map_err(|e| fail(&e))?;
    let window: WindowId = driver.runtime().windows()[0].1;
    let system = driver.window_system_mut();
    system
        .enqueue_window(
            window,
            cupertino_kit::test::WindowEvent::Scale(f64::from(scale)),
        )
        .map_err(|e| fail(&e))?;
    system
        .configure(window, None, WindowStates::default())
        .map_err(|e| fail(&e))?;
    for _ in 0..FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if !system.frame_requested(window).map_err(|e| fail(&e))? {
            break;
        }
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| fail(&e))?;
    }
    presented(driver.runtime().window_system(), window)
}

fn main() -> ExitCode {
    cupertino_kit::test::capture::run(|| {
        cupertino_kit::test::capture::assets()?;
        Ok(Capture)
    })
}
