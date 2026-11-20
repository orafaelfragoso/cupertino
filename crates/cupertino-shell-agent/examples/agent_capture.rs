//! The session agent's panels drawn to PNGs for look checks:
//! `cargo run -p cupertino-shell-agent --example agent_capture -- agent/SURFACE [flags]` (crates/cupertino-kit/README.md).
//!
//! Surfaces: `log-out`, `restart` and `shut-down` (the confirmation panels, their countdown at its start), `power-button` (the
//! panel a held power key shows) and `interrupt` (the panel that interrupts a log out when an application does not quit).
//! On a confirmation panel `--reopen on|off` checks or clears "Open my windows again". On the interrupt panel `--reason
//! cancelled|failed|not-responding` says why the application is still there and `--stage ask|force-quit` which question the
//! panel asks. The application's name, the countdown and the words are fixed, so a command gives the same pixels on every
//! run. The fonts, symbols and theme are the system's, as an application's.

use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_kit::core::{Application, Command, Key, Window, WindowKey};
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::text::TextSystem;
use cupertino_kit::theme::ThemeTokens;
use cupertino_shell_agent::interrupt::{Reason, Stage};
use cupertino_shell_agent::panel::{Action, Kind, Panel};
use cupertino_shell_agent::view::{self, Input};

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames the window may ask for before it rests.
const FRAMES: usize = 16;
/// The application the interrupt panel names.
const APP: &str = "Text Editor";

/// What a capture's window shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shot {
    Panel { kind: Kind, reopen: bool },
    Interrupt { reason: Reason, stage: Stage },
}

thread_local! {
    /// What the next capture's window shows, and the text system its frames are measured with.
    static SHOT: Cell<Option<Shot>> = const { Cell::new(None) };
    static TEXT: RefCell<Option<Arc<TextSystem>>> = const { RefCell::new(None) };
}

/// The panel's window showing [`SHOT`].
struct Shown;

struct Model {
    shot: Shot,
    text: Arc<TextSystem>,
}

impl Application for Shown {
    type Model = Model;
    type Message = Input;

    fn init() -> (Model, Vec<Command<Input>>) {
        let shot = SHOT.get().expect("the capture sets the shot first");
        let text = TEXT.take().expect("the capture sets the text system first");
        (Model { shot, text }, Vec::new())
    }

    fn update(_: &mut Model, _: Input) -> Vec<Command<Input>> {
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Input>> {
        let key = WindowKey(Key::from("capture"));
        vec![match model.shot {
            Shot::Panel { kind, reopen } => {
                let frames = view::panel_frames(kind, &model.text);
                let seconds = Panel::new(kind, 0.0, reopen).seconds_left(0.0);
                view::panel(key, kind, seconds, reopen, &frames, |input| input)
            }
            Shot::Interrupt { reason, stage } => view::interrupt(
                key,
                view::InterruptView {
                    app: APP,
                    reason,
                    stage,
                    action: Action::LogOut,
                    text: &model.text,
                },
                |input| input,
            ),
        }]
    }
}

struct Agent;

/// The value of `flag`, one of `values` (the first by default), by its place among them.
fn pick(request: &Request<'_>, flag: &str, values: &[&str]) -> Result<usize, String> {
    match request.state.iter().find(|(f, _)| f == flag) {
        None => Ok(0),
        Some((_, value)) => values
            .iter()
            .position(|v| v == value)
            .ok_or_else(|| format!("--{flag} {value}: one of {}", values.join(", "))),
    }
}

impl App for Agent {
    const NAME: &'static str = "agent";
    const STATE_FLAGS: &'static [&'static str] = &["reopen", "reason", "stage"];

    fn surfaces(&self) -> Vec<String> {
        [
            "log-out",
            "restart",
            "shut-down",
            "power-button",
            "interrupt",
        ]
        .map(String::from)
        .to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        if surface == "interrupt" {
            let mut states = Vec::new();
            for reason in ["cancelled", "failed", "not-responding"] {
                states.push(vec![
                    ("reason".to_owned(), reason.to_owned()),
                    ("stage".to_owned(), "ask".to_owned()),
                ]);
            }
            states.push(vec![
                ("reason".to_owned(), "failed".to_owned()),
                ("stage".to_owned(), "force-quit".to_owned()),
            ]);
            return states;
        }
        ["on", "off"]
            .map(|v| vec![("reopen".to_owned(), v.to_owned())])
            .to_vec()
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let panel = |kind| -> Result<Shot, String> {
            Ok(Shot::Panel {
                kind,
                reopen: pick(request, "reopen", &["on", "off"])? == 0,
            })
        };
        let shot = match request.surface {
            "log-out" => panel(Kind::LogOut)?,
            "restart" => panel(Kind::Restart)?,
            "shut-down" => panel(Kind::ShutDown)?,
            "power-button" => panel(Kind::PowerButton)?,
            _ => Shot::Interrupt {
                reason: [
                    Reason::Cancelled,
                    Reason::FailedToQuit,
                    Reason::NotResponding,
                ][pick(
                    request,
                    "reason",
                    &["cancelled", "failed", "not-responding"],
                )?],
                stage: [Stage::Ask, Stage::ConfirmForceQuit]
                    [pick(request, "stage", &["ask", "force-quit"])?],
            },
        };
        SHOT.set(Some(shot));
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
    TEXT.set(Some(Arc::clone(&options.text)));
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
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    system
        .configure(window, None, active)
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
        Ok(Agent)
    })
}
