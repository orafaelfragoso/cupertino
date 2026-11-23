//! The on-screen displays drawn to PNGs for look checks:
//! `cargo run -p cupertino-osd --example osd_capture -- osd/SURFACE [flags]` (crates/cupertino-kit/README.md).
//!
//! * `overlay`: the 200 x 200 pt window on banner glass, as the server shows it. `--kind` names the overlay
//!   (`volume`, the default, `mute`, `brightness`, `keyboard-brightness`, `microphone`, `microphone-mute`), `--level`
//!   its meter's level from 0 to 1 (default 0.5) and `--alpha` the opacity of the fade (default 1). The sheet shows each
//!   kind.
//! * `sources`: the input-source switcher's panel listing two sources, `us` and `de`, on the regular glass of an
//!   alert. `--selected` is the selected row (0 or 1, default 1) and `--alpha` the opacity of the fade (default 1). The
//!   sheet shows each selection.
//! * `accents`: the accent picker's panel with the seven candidates of `e` on the menu material. `--focused` is the
//!   focused candidate (`none`, the default, or 1 to 7). The sheet shows none and the second.
//!
//! The fonts, symbols and theme are the system's, as an application's; nothing moves, so a command gives the same
//! pixels on every run.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_kit_shell::keyboard_panels::{Source, Update};
use cupertino_osd::model::{Kind, Overlay, View};
use cupertino_osd::panels::{Panels, LABEL_SIZE};
use cupertino_osd::{panels_view, view};

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

/// The sources the switcher's panel lists, most recently used first, as the server names them.
const SOURCES: [(&str, &str); 2] = [("us", "English (Macintosh, ABC, ANSI)"), ("de", "German")];
/// The candidates of `e` the picker shows.
const CANDIDATES: [&str; 7] = ["è", "é", "ê", "ë", "ē", "ė", "ę"];
/// The output the picker is laid out on, in points, and the text cursor on it (from its top left corner).
const OUTPUT: Size = Size::new(800.0, 600.0);
const CARET: Rect = Rect::new(400.0, 300.0, 2.0, 18.0);

/// What a capture's window shows.
#[derive(Clone, Debug, Default)]
enum Shown {
    #[default]
    Nothing,
    Overlay(Option<View>),
    /// The keyboard's panels, with the alpha the switcher's panel is drawn at.
    Panels(Panels, f64),
}

thread_local! {
    /// What the next capture's window shows.
    static SHOWN: RefCell<Shown> = const { RefCell::new(Shown::Nothing) };
}

/// The window showing [`SHOWN`].
struct Showing;

impl Application for Showing {
    type Model = Shown;
    type Message = ();

    fn init() -> (Shown, Vec<Command<()>>) {
        (SHOWN.with_borrow(Clone::clone), Vec::new())
    }

    fn update(_: &mut Shown, (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(shown: &Shown) -> Vec<Window<()>> {
        match shown {
            Shown::Nothing => Vec::new(),
            Shown::Overlay(view) => vec![view::window(*view, None)],
            Shown::Panels(panels, alpha) => {
                let sources = panels.sources.as_ref().map(|panel| {
                    let widest = panel
                        .sources
                        .iter()
                        .map(|s| panels_view::text_width(&s.name, LABEL_SIZE, 400))
                        .fold(0.0, f64::max);
                    panels_view::sources_window(panel, panel.size(widest), *alpha, None)
                });
                let accents = panels
                    .accents
                    .as_ref()
                    .map(|panel| panels_view::accents_window(panel, &panel.layout(OUTPUT), |_| ()));
                sources.into_iter().chain(accents).collect()
            }
        }
    }
}

struct Osd;

/// The index a flag gives, below `count`; `none` is `None`.
fn index(
    request: &Request<'_>,
    flag: &str,
    default: Option<usize>,
    count: usize,
) -> Result<Option<usize>, String> {
    match request.state.iter().find(|(f, _)| f == flag) {
        None => Ok(default),
        Some((_, value)) if value == "none" => Ok(None),
        Some((_, value)) => value
            .parse::<usize>()
            .ok()
            .filter(|v| *v < count)
            .map(Some)
            .ok_or_else(|| format!("--{flag} {value}: none or a number below {count}")),
    }
}

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
    const STATE_FLAGS: &'static [&'static str] = &["kind", "level", "alpha", "selected", "focused"];

    fn surfaces(&self) -> Vec<String> {
        ["overlay", "sources", "accents"]
            .map(str::to_owned)
            .to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        let one = |flag: &str, value: &str| vec![(flag.to_owned(), value.to_owned())];
        match surface {
            "sources" => vec![one("selected", "0"), one("selected", "1")],
            "accents" => vec![one("focused", "none"), one("focused", "1")],
            _ => KINDS.map(|kind| one("kind", kind)).to_vec(),
        }
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        match request.surface {
            "sources" => {
                let selected = index(request, "selected", Some(1), SOURCES.len())?.unwrap_or(0);
                let mut panels = Panels::default();
                panels.apply(
                    Update::Sources {
                        output: None,
                        sources: SOURCES
                            .map(|(id, name)| Source {
                                id: id.to_owned(),
                                name: name.to_owned(),
                            })
                            .to_vec(),
                        selected,
                    },
                    0.0,
                );
                SHOWN.set(Shown::Panels(panels, fraction(request, "alpha", ALPHA)?));
                return draw(request.look, request.scale);
            }
            "accents" => {
                let mut panels = Panels::default();
                panels.apply(
                    Update::Accents {
                        serial: 1,
                        output: None,
                        anchor: CARET,
                        top: 0.0,
                        candidates: CANDIDATES.map(str::to_owned).to_vec(),
                        focused: index(request, "focused", None, CANDIDATES.len())?,
                    },
                    0.0,
                );
                SHOWN.set(Shown::Panels(panels, ALPHA));
                return draw(request.look, request.scale);
            }
            _ => {}
        }
        let kind = match request.state.iter().find(|(f, _)| f == "kind") {
            None => Kind::Volume,
            Some((_, name)) => Kind::from_name(name)
                .ok_or_else(|| format!("--kind {name}: one of {}", KINDS.join(", ")))?,
        };
        let overlay =
            Overlay::new(kind, fraction(request, "level", LEVEL)?).map_err(|e| e.to_string())?;
        let alpha = fraction(request, "alpha", ALPHA)?;
        SHOWN.set(Shown::Overlay(Some(View { overlay, alpha })));
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
    let mut driver = TestDriver::<Showing>::new(
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
