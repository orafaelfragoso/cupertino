//! The control panel drawn to PNGs for look checks:
//! `cargo run -p cupertino-controlcenter --example controlcenter_capture -- controlcenter/panel [flags]`
//! (crates/cupertino-kit/README.md).
//!
//! One surface, `panel`: the panel's window with every control shown, as it opens below the menu bar, its shadow's
//! margin included. `--detail` names the detail view that is open (`none`, the default, `wifi`, `bluetooth`,
//! `display` or `sound`), `--edit on` shows the controls being edited, and `--moment moving` shows the detail view
//! half way through its opening instead of open. The sheet shows the grid, each detail view and the editing state. The
//! readings are fixed sample data; the fonts, symbols and theme are the system's, as an application's; nothing moves,
//! so a command gives the same pixels on every run.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_controlcenter::arrangement::Arrangement;
use cupertino_controlcenter::model::{
    Controls, Detail, Device, Media, Network, Output, Reading, VolumeState, WifiStatus,
};
use cupertino_controlcenter::view::{self, Editing, Input, Opened, Presentation};
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames the window may ask for before it rests.
const FRAMES: usize = 16;
/// The detail views a capture may show, by their flag values.
const DETAILS: [(&str, Option<Detail>); 5] = [
    ("none", None),
    ("wifi", Some(Detail::WiFi)),
    ("bluetooth", Some(Detail::Bluetooth)),
    ("display", Some(Detail::Display)),
    ("sound", Some(Detail::Sound)),
];
/// How far a detail view has opened at the `moving` moment.
const MOVING_PROGRESS: f64 = 0.5;

thread_local! {
    /// What the next capture's window shows.
    static SHOWN: RefCell<Option<(Controls, Presentation)>> = const { RefCell::new(None) };
}

/// The panel's window showing [`SHOWN`].
struct Shown;

impl Application for Shown {
    type Model = (Controls, Presentation);
    type Message = Input;

    fn init() -> ((Controls, Presentation), Vec<Command<Input>>) {
        let shown = SHOWN
            .with_borrow_mut(Option::take)
            .expect("a panel for the capture");
        (shown, Vec::new())
    }

    fn update(_: &mut (Controls, Presentation), _: Input) -> Vec<Command<Input>> {
        Vec::new()
    }

    fn view((controls, presentation): &(Controls, Presentation)) -> Vec<Window<Input>> {
        vec![view::arranged(
            controls,
            &Arrangement::default(),
            *presentation,
            std::convert::identity,
        )]
    }
}

/// The readings every capture shows: Wi-Fi on a known network with two more in range, Bluetooth on with two paired
/// devices, the microphone on, a track playing, the display at 0.6 and the sound at 0.4 on the first of two outputs.
fn controls() -> Controls {
    let network = |ssid: &str, bars, secured, known, connected| Network {
        ssid: ssid.to_owned(),
        bars: Some(bars),
        secured,
        known,
        connected,
    };
    let device = |id: &str, name: &str, connected| Device {
        id: id.to_owned(),
        name: name.to_owned(),
        connected,
    };
    let output = |id, name: &str, current| Output {
        id,
        name: name.to_owned(),
        current,
    };
    Controls {
        wifi: Reading::Ready(WifiStatus {
            enabled: true,
            network: Some("Home Network".to_owned()),
        }),
        bluetooth: Reading::Ready(true),
        microphone: Reading::Ready(true),
        dock_hiding: Reading::Ready(false),
        keyboard: Reading::Ready(false),
        focus_mode: Reading::Ready(false),
        brightness: Reading::Ready(0.6),
        volume: Reading::Ready(VolumeState {
            level: 0.4,
            muted: false,
        }),
        media: Reading::Ready(Some(Media {
            title: "Clair de Lune".to_owned(),
            artist: Some("Claude Debussy".to_owned()),
            playing: true,
            can_previous: true,
            can_next: true,
            can_play_pause: true,
        })),
        networks: Reading::Ready(vec![
            network("Home Network", 3, true, true, true),
            network("Guest", 2, false, false, false),
            network("Neighbours", 1, true, false, false),
        ]),
        devices: Reading::Ready(vec![
            device("00:11:22:33:44:55", "Headphones", true),
            device("66:77:88:99:AA:BB", "Keyboard", false),
        ]),
        outputs: Reading::Ready(vec![
            output(31, "Speakers", true),
            output(47, "Headphones", false),
        ]),
    }
}

/// The value of state flag `flag`, or the first of `values`; an error unless it is one of them.
fn flag<'a>(request: &'a Request<'_>, flag: &str, values: &[&'a str]) -> Result<&'a str, String> {
    match request.state.iter().find(|(f, _)| f == flag) {
        None => Ok(values[0]),
        Some((_, value)) if values.contains(&value.as_str()) => Ok(value),
        Some((_, value)) => Err(format!("--{flag} {value}: one of {}", values.join(", "))),
    }
}

struct Panel;

impl App for Panel {
    const NAME: &'static str = "controlcenter";
    const STATE_FLAGS: &'static [&'static str] = &["detail", "edit", "moment"];

    fn surfaces(&self) -> Vec<String> {
        vec!["panel".to_owned()]
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        let mut states: Vec<State> = DETAILS
            .iter()
            .map(|(name, _)| vec![("detail".to_owned(), (*name).to_owned())])
            .collect();
        states.push(vec![("edit".to_owned(), "on".to_owned())]);
        states
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let names: Vec<&str> = DETAILS.iter().map(|(name, _)| *name).collect();
        let detail = flag(request, "detail", &names)?;
        let detail = DETAILS
            .iter()
            .find(|(name, _)| *name == detail)
            .and_then(|(_, detail)| *detail);
        let editing = flag(request, "edit", &["off", "on"])? == "on";
        let moving = flag(request, "moment", &["rest", "moving"])? == "moving";
        if editing && detail.is_some() {
            return Err(
                "--edit on: the controls are edited on the grid, with no detail view".to_owned(),
            );
        }
        let presentation = Presentation {
            light: !request.look.dark,
            detail: detail.map(|detail| Opened {
                detail,
                progress: if moving { MOVING_PROGRESS } else { 1.0 },
                morph: true,
            }),
            edit: editing.then(Editing::default),
            ..Presentation::SHOWN
        };
        SHOWN.set(Some((controls(), presentation)));
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
    let window: WindowId = driver
        .runtime()
        .windows()
        .first()
        .ok_or("the panel shows no window")?
        .1;
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
        Ok(Panel)
    })
}
