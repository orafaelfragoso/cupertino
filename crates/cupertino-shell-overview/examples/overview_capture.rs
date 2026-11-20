//! The overview drawn to PNGs for look checks:
//! `cargo run -p cupertino-shell-overview --example overview_capture -- overview/SURFACE [flags]`
//! (crates/cupertino-kit/README.md).
//!
//! Three surfaces of a 1440 x 900 pt output with four windows and three desktops: `rest`, the overview open;
//! `transition`, on its way in at `--progress` from 0 (the windows at their own frames, no chrome) to 1 (default 0.5);
//! `workspaces`, open with the pointer at the top, so the band shows the desktops' pictures. A capture has no server:
//! each hole is painted a flat grey where the server would draw the window or the desktop. Nothing moves, so a command
//! gives the same pixels on every run.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_kit::core::{Application, Command, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_shell_overview::chrome::{self, HolePlace, Scene};
use cupertino_shell_overview::layout::{Mode, OverviewUi, WindowEntry, Workspace};
use cupertino_wm::geometry::{Point, Rect};

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames the window may ask for before it rests.
const FRAMES: usize = 16;
/// The output, and the area the windows are laid out in: below a 33 pt menu bar, above a 70 pt dock.
const OUTPUT: Size = Size::new(1440.0, 900.0);
const AREA: Rect = Rect::new(0.0, 0.0, 1440.0, 830.0);
/// The windows' own frames and titles.
const WINDOWS: [(u32, Rect, &str); 4] = [
    (1, Rect::new(80.0, 60.0, 900.0, 600.0), "Files"),
    (2, Rect::new(520.0, 140.0, 760.0, 520.0), "Terminal"),
    (3, Rect::new(200.0, 300.0, 640.0, 420.0), "Notes"),
    (4, Rect::new(980.0, 90.0, 380.0, 520.0), ""),
];
/// The desktops, the first current.
const DESKTOPS: [u32; 3] = [1, 2, 3];
/// What a hole is painted in a capture.
const HOLE: [f32; 4] = [0.55, 0.57, 0.62, 1.0];
/// The model's clock at the capture: every spring of the layout and the band at rest.
const NOW: f64 = 10.0;
/// The progress a transition capture shows unless its flags say otherwise.
const PROGRESS: f64 = 0.5;

/// What the next capture's window shows.
struct Shown {
    ui: OverviewUi,
    holes: Vec<HolePlace>,
    chrome_opacity: f64,
}

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

/// The overview's window showing [`SHOWN`].
struct Captured;

impl Application for Captured {
    type Model = Option<Shown>;
    type Message = ();

    fn init() -> (Option<Shown>, Vec<Command<()>>) {
        (SHOWN.with(|s| s.borrow_mut().take()), Vec::new())
    }

    fn update(_: &mut Option<Shown>, (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(shown: &Option<Shown>) -> Vec<Window<()>> {
        let Some(shown) = shown else {
            return Vec::new();
        };
        let scene = Scene {
            key: WindowKey(Key::from("overview")),
            size: OUTPUT,
            ui: &shown.ui,
            holes: shown.holes.clone(),
            reduce_motion: false,
            hole_fill: Some(HOLE),
            chrome_opacity: shown.chrome_opacity,
        };
        vec![chrome::window(&scene, NOW, |_| (), None)]
    }
}

struct Capture;

/// The point `t` of the way from `a` to `b`.
fn between(a: Rect, b: Rect, t: f64) -> Rect {
    let mix = |p: f64, q: f64| p + (q - p) * t;
    Rect::new(mix(a.x, b.x), mix(a.y, b.y), mix(a.w, b.w), mix(a.h, b.h))
}

/// The overview of the fixture, at `progress` of its way in; with `band` the pointer rests at the top.
fn shown(progress: f64, band: bool) -> Shown {
    let entries: Vec<WindowEntry> = WINDOWS
        .iter()
        .map(|&(id, frame, title)| WindowEntry {
            id,
            frame,
            title: title.to_owned(),
            minimized: false,
        })
        .collect();
    let workspaces = DESKTOPS
        .iter()
        .map(|&id| Workspace {
            id,
            current: id == DESKTOPS[0],
        })
        .collect();
    let mut ui = OverviewUi::new(Mode::All, AREA, &entries, workspaces);
    if band {
        ui.pointer_moved(Some(Point::new(OUTPUT.width / 2.0, 4.0)), 0.0);
    } else {
        // the pointer on the first window: its ring and name show
        if let Some((_, frame)) = ui.frames().first() {
            ui.pointer_moved(Some(Point::new(frame.cx(), frame.cy())), 0.0);
        }
    }
    let holes = ui
        .frames()
        .into_iter()
        .map(|(id, target)| {
            let start = entries
                .iter()
                .find(|e| e.id == id)
                .map_or(target, |e| e.frame);
            HolePlace {
                id,
                rect: between(start, target, progress),
            }
        })
        .collect();
    Shown {
        ui,
        holes,
        chrome_opacity: progress,
    }
}

impl App for Capture {
    const NAME: &'static str = "overview";
    const STATE_FLAGS: &'static [&'static str] = &["progress"];

    fn surfaces(&self) -> Vec<String> {
        ["rest", "transition", "workspaces"]
            .map(str::to_owned)
            .to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        match surface {
            "transition" => ["0.25", "0.5", "0.75"]
                .map(|p| vec![("progress".to_owned(), p.to_owned())])
                .to_vec(),
            _ => vec![Vec::new()],
        }
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let progress = match request.state.iter().find(|(f, _)| f == "progress") {
            None => PROGRESS,
            Some((_, value)) => value
                .parse::<f64>()
                .ok()
                .filter(|v| (0.0..=1.0).contains(v))
                .ok_or_else(|| format!("--progress {value}: a number from 0 to 1"))?,
        };
        let shown = match request.surface {
            "rest" => shown(1.0, false),
            "transition" => shown(progress, false),
            "workspaces" => shown(1.0, true),
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
