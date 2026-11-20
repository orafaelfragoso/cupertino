//! The on-screen display drawn to PNGs for look checks:
//! `cargo run -p cupertino-osd --example osd_capture -- osd/overlay [flags]` (crates/cupertino-kit/README.md).
//!
//! One surface, `overlay`: the 200 x 200 pt window over the HUD material, as the server shows it. `--kind` names the
//! overlay (`volume`, the default, `mute`, `brightness`, `keyboard-brightness`, `microphone`, `microphone-mute`),
//! `--level` its meter's level from 0 to 1 (default 0.5) and `--alpha` the opacity of the fade (default 1). The sheet
//! shows each kind. The fonts, symbols and theme are the system's, as an application's; nothing moves, so a command
//! gives the same pixels on every run.

use std::cell::Cell;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_osd::model::{Kind, Overlay, View};
use cupertino_osd::view;

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames the window may ask for before it rests.
const FRAMES: usize = 16;
/// The overlays the sheet shows, each at its default level.
const KINDS: [&str; 6] = [
    "volume",
    "mute",
    "brightness",
    "keyboard-brightness",
    "microphone",
    "microphone-mute",
];
/// The level and the opacity a capture shows unless its flags say otherwise.
const LEVEL: f64 = 0.5;
const ALPHA: f64 = 1.0;

thread_local! {
    /// What the next capture's window shows.
    static SHOWN: Cell<Option<View>> = const { Cell::new(None) };
}

/// The overlay's window showing [`SHOWN`].
struct Shown;

impl Application for Shown {
    type Model = Option<View>;
    type Message = ();

    fn init() -> (Option<View>, Vec<Command<()>>) {
        (SHOWN.get(), Vec::new())
    }

    fn update(_: &mut Option<View>, (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(shown: &Option<View>) -> Vec<Window<()>> {
        vec![view::window(*shown, None)]
    }
}

struct Osd;

/// The number a flag gives, from 0 to 1.
fn fraction(request: &Request<'_>, flag: &str, default: f64) -> Result<f64, String> {
    match request.state.iter().find(|(f, _)| f == flag) {
        None => Ok(default),
        Some((_, value)) => value
            .parse::<f64>()
            .ok()
            .filter(|v| (0.0..=1.0).contains(v))
            .ok_or_else(|| format!("--{flag} {value}: a number from 0 to 1")),
    }
}

impl App for Osd {
    const NAME: &'static str = "osd";
    const STATE_FLAGS: &'static [&'static str] = &["kind", "level", "alpha"];

    fn surfaces(&self) -> Vec<String> {
        vec!["overlay".to_owned()]
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        KINDS
            .map(|kind| vec![("kind".to_owned(), kind.to_owned())])
            .to_vec()
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let kind = match request.state.iter().find(|(f, _)| f == "kind") {
            None => Kind::Volume,
            Some((_, name)) => Kind::from_name(name)
                .ok_or_else(|| format!("--kind {name}: one of {}", KINDS.join(", ")))?,
        };
        let overlay =
            Overlay::new(kind, fraction(request, "level", LEVEL)?).map_err(|e| e.to_string())?;
        let alpha = fraction(request, "alpha", ALPHA)?;
        SHOWN.set(Some(View { overlay, alpha }));
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
    let mut driver = TestDriver::<Shown>::new(
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
        Ok(Osd)
    })
}
