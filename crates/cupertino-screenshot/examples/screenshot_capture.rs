//! The screenshot tool's surfaces drawn to PNGs for look checks:
//! `cargo run -p cupertino-screenshot --example screenshot_capture -- screenshot/SURFACE [flags]`
//! (crates/cupertino-kit/README.md).
//!
//! The surfaces, on a 1280 x 800 pt output:
//! - `overlay`: an output's overlay; `--overlay` is `drawn` (the toolbar's selection with its handles, the default),
//!   `dragging` (the direct shortcut's selection being drawn, with its size), `crosshair` (no selection yet) or
//!   `window` (the window pick's highlight).
//! - `toolbar`: the toolbar; `--mode` is `selection` (the default), `window` or `screen`.
//! - `options-menu`: the toolbar's Options menu, open.
//! - `thumbnail`: the floating thumbnail of a capture of the output, with the room for its shadow.
//! - `color-picker`: the colour picker's overlay on the output's copy; `--pointer` is `centre` (the default) or
//!   `edge`, where the loupe moves to the pointer's other side.
//! - `share-picker`: the choice of what a screen share shows, for an app that may be remembered, before it is armed.
//!
//! The overlay is transparent where it shows the screen, so it is drawn over a checkerboard of the capture's own. The
//! fonts, symbols and theme are the system's, as the program's, under a home of the capture's own
//! (`target/capture-home`); nothing moves, so a command gives the same pixels on every run.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_kit::core::next_resource_id;
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::platform::{SystemAppearance, Timestamp};
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_screenshot::color_picker::{self, Display, PickInput, Picking};
use cupertino_screenshot::controller::Controller;
use cupertino_screenshot::pick::PickWindow;
use cupertino_screenshot::session::Style;
use cupertino_screenshot::settings::{Mode, Settings};
use cupertino_screenshot::share_picker::{self, Ask, Picker, Source};
use cupertino_screenshot::view::{Input, PointerInput};

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames a window may ask for before it rests.
const FRAMES: usize = 16;
/// The rounds of windows opening, each after the frames of the last: a popup opens once its parent was drawn.
const ROUNDS: usize = 3;
/// The output the surfaces are drawn on, in points.
const OUTPUT: Rect = Rect::new(0.0, 0.0, 1280.0, 800.0);
/// The squares of the checkerboard the overlay is drawn over, in points, and their two greys.
const CHECKER: f32 = 16.0;
const CHECKER_TONES: [u8; 2] = [200, 150];

/// What a capture shows: a capture's run, a colour pick or a sharing choice.
enum Scene {
    Shot(Box<Controller>),
    Pick(Picking),
    Share(Picker),
}

thread_local! {
    /// The scene whose windows the next capture shows.
    static SHOWN: RefCell<Option<Scene>> = const { RefCell::new(None) };
}

/// The messages of the windows in [`SHOWN`], which change nothing.
#[derive(Clone, Debug)]
#[expect(
    dead_code,
    reason = "a capture acts on none but the sharing list's scrolling"
)]
enum Shot {
    Input(Input),
    Pick(PickInput),
    Share(share_picker::Input),
}

/// The windows of the scene in [`SHOWN`].
struct Shown;

impl Application for Shown {
    type Model = ();
    type Message = Shot;

    fn init() -> ((), Vec<Command<Shot>>) {
        ((), Vec::new())
    }

    /// A sharing choice follows its list's scrolling, which sets the rows it shows; nothing else changes.
    fn update(_: &mut (), message: Shot) -> Vec<Command<Shot>> {
        if let Shot::Share(input @ share_picker::Input::Scroll(..)) = message {
            SHOWN.with_borrow_mut(|scene| {
                if let Some(Scene::Share(picker)) = scene {
                    picker.input(input);
                }
            });
        }
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<Shot>> {
        SHOWN.with_borrow(|scene| match scene {
            Some(Scene::Shot(c)) => c.windows(0.0, Shot::Input),
            Some(Scene::Pick(p)) => p
                .view(0, true)
                .map(|v| color_picker::overlay(color_picker::overlay_key(0), v, Shot::Pick))
                .into_iter()
                .collect(),
            Some(Scene::Share(p)) => vec![share_picker::window(p, Shot::Share)],
            None => Vec::new(),
        })
    }
}

fn press(controller: &mut Controller, at: Point, pressed: bool) {
    controller.input(
        Input::Pointer(PointerInput::Button {
            pressed,
            at,
            control: false,
        }),
        0.0,
    );
}

fn moved(controller: &mut Controller, at: Point) {
    controller.input(Input::Pointer(PointerInput::Moved(at)), 0.0);
}

/// A run in `style` with the default settings, the toolbar in `mode`, and no window remembered.
fn run(style: Option<Style>, mode: Mode) -> Controller {
    let settings = Settings {
        mode,
        selection: None,
        ..Settings::default()
    };
    Controller::new(style, settings, vec![OUTPUT], false).0
}

/// The overlay in the state `--overlay` names.
fn overlay(state: &str) -> Result<Controller, String> {
    let (from, to) = (Point::new(320.0, 200.0), Point::new(960.0, 600.0));
    Ok(match state {
        "drawn" => {
            let mut c = run(Some(Style::Interactive), Mode::Selection);
            moved(&mut c, from);
            press(&mut c, from, true);
            moved(&mut c, to);
            press(&mut c, to, false);
            moved(&mut c, Point::new(640.0, 700.0));
            c
        }
        "dragging" => {
            let mut c = run(Some(Style::Simple), Mode::Selection);
            moved(&mut c, from);
            press(&mut c, from, true);
            moved(&mut c, to);
            c
        }
        "crosshair" => {
            let mut c = run(Some(Style::Simple), Mode::Selection);
            moved(&mut c, Point::new(640.0, 400.0));
            c
        }
        "window" => {
            let mut c = run(Some(Style::Interactive), Mode::Window);
            c.set_windows(vec![
                PickWindow {
                    identifier: "front".into(),
                    frame: Rect::new(240.0, 160.0, 560.0, 380.0),
                },
                PickWindow {
                    identifier: "back".into(),
                    frame: Rect::new(520.0, 300.0, 600.0, 400.0),
                },
            ]);
            moved(&mut c, Point::new(400.0, 300.0));
            c
        }
        other => {
            return Err(format!(
                "--overlay {other}: drawn, dragging, crosshair or window"
            ))
        }
    })
}

/// The picture of the output a thumbnail shows: a gradient, so its scaling shows.
fn picture() -> ImageResource {
    let (w, h) = (OUTPUT.width() as u32, OUTPUT.height() as u32);
    let mut pixels = vec![0; (w * h * 4) as usize];
    for (i, px) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        px.copy_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, 160, 255]);
    }
    ImageResource {
        revision: 1,
        width: w,
        height: h,
        stride: w * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: pixels.into(),
    }
}

fn flag<'a>(request: &'a Request<'_>, name: &str, default: &'a str) -> &'a str {
    request
        .state
        .iter()
        .find(|(f, _)| f == name)
        .map_or(default, |(_, v)| v.as_str())
}

struct Screenshot;

impl App for Screenshot {
    const NAME: &'static str = "screenshot";
    const STATE_FLAGS: &'static [&'static str] = &["overlay", "mode", "pointer"];

    fn surfaces(&self) -> Vec<String> {
        [
            "overlay",
            "toolbar",
            "options-menu",
            "thumbnail",
            "color-picker",
            "share-picker",
        ]
        .map(String::from)
        .to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        let states: &[(&str, &[&str])] = match surface {
            "overlay" => &[("overlay", &["drawn", "dragging", "crosshair", "window"])],
            "toolbar" => &[("mode", &["selection", "window", "screen"])],
            "color-picker" => &[("pointer", &["centre", "edge"])],
            _ => &[],
        };
        match states.first() {
            Some((flag, values)) => values
                .iter()
                .map(|v| vec![((*flag).to_owned(), (*v).to_owned())])
                .collect(),
            None => vec![Vec::new()],
        }
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let (controller, key, backdrop) = match request.surface {
            "overlay" => (
                overlay(flag(request, "overlay", "drawn"))?,
                "overlay/0",
                true,
            ),
            "toolbar" => {
                let mode = flag(request, "mode", "selection");
                let mode = Mode::from_name(mode)
                    .ok_or_else(|| format!("--mode {mode}: selection, window or screen"))?;
                (run(Some(Style::Interactive), mode), "toolbar", false)
            }
            "options-menu" => {
                let mut c = run(Some(Style::Interactive), Mode::Selection);
                c.input(Input::Options(true), 0.0);
                (c, "options-menu", false)
            }
            "thumbnail" => {
                let mut c = run(None, Mode::Selection);
                let picture = picture();
                let (w, h) = c.thumbnail_pixels((picture.width, picture.height));
                let small =
                    cupertino_kit::image::resize(&picture, w, h).map_err(|e| e.to_string())?;
                c.shots(Ok(vec![picture]), Some(small), 0.0);
                (c, "thumbnail", true)
            }
            "color-picker" => {
                let pointer = match flag(request, "pointer", "centre") {
                    "centre" => Point::new(640.0, 400.0),
                    "edge" => Point::new(1250.0, 780.0),
                    other => return Err(format!("--pointer {other}: centre or edge")),
                };
                let mut picking = Picking::new(vec![Display {
                    rect: OUTPUT,
                    image: picture(),
                    id: next_resource_id(),
                }]);
                picking.moved(pointer);
                SHOWN.set(Some(Scene::Pick(picking)));
                return draw("pick/0", request.look, request.scale);
            }
            "share-picker" => {
                SHOWN.set(Some(Scene::Share(Picker::new(0, share_ask()))));
                return draw("share/0", request.look, request.scale);
            }
            other => return Err(format!("no surface {other}")),
        };
        SHOWN.set(Some(Scene::Shot(Box::new(controller))));
        let picture = draw(key, request.look, request.scale)?;
        Ok(if backdrop {
            over_checkerboard(&picture, request.scale)
        } else {
            picture
        })
    }
}

/// A request of an app that names itself, for a display and two windows, one without its app.
fn share_ask() -> Ask {
    Ask {
        app_name: Some("Viewer".into()),
        app_icon: None,
        parent_window: String::new(),
        sources: vec![
            Source::display("DP-1", "Dell P2723QE"),
            Source::window("Notes", "org.example.Notes"),
            Source::window("Terminal", ""),
        ],
        offer_remember: true,
    }
}

/// `picture` over a checkerboard, so what it leaves clear shows.
fn over_checkerboard(picture: &Pixmap, scale: f32) -> Pixmap {
    let mut out = Pixmap::new(picture.width(), picture.height()).expect("a picture");
    let side = (CHECKER * scale).max(1.0) as u32;
    let w = picture.width();
    for (i, (dst, src)) in out
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(picture.data().as_chunks::<4>().0)
        .enumerate()
    {
        let (x, y) = (i as u32 % w, i as u32 / w);
        let tone = CHECKER_TONES[((x / side + y / side) % 2) as usize];
        let a = u32::from(src[3]);
        for c in 0..3 {
            dst[c] = (u32::from(src[c]) + u32::from(tone) * (255 - a) / 255) as u8;
        }
        dst[3] = 255;
    }
    out
}

// a capture loads the theme of the appearance it draws, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn draw(key: &str, look: Look, scale: f32) -> Result<Pixmap, String> {
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
    let output = Size::new(OUTPUT.width(), OUTPUT.height());
    // a popup opens once its parent showed a frame: each round configures and draws the windows that opened since
    let mut configured = Vec::new();
    let mut window = None;
    for _ in 0..ROUNDS {
        for (k, id) in driver.runtime().windows() {
            if configured.contains(&id) {
                continue;
            }
            configured.push(id);
            let name = format!("{:?}", k.0);
            // an overlay takes its output's size
            let size = (name.contains("overlay/") || name.contains("pick/")).then_some(output);
            let system = driver.window_system_mut();
            system
                .enqueue_window(
                    id,
                    cupertino_kit::test::WindowEvent::Scale(f64::from(scale)),
                )
                .map_err(|e| fail(&e))?;
            system
                .configure(id, size, WindowStates::default())
                .map_err(|e| fail(&e))?;
            // a menu's panels are keyed under the menu's key
            if window.is_none() && format!("{:?}", k.0).contains(&format!("{key:?}")) {
                window = Some(id);
            }
        }
        for _ in 0..FRAMES {
            driver.run_until_idle(turns).map_err(|e| fail(&e))?;
            let system = driver.window_system_mut();
            let mut asked = false;
            for id in &configured {
                if system.frame_requested(*id).unwrap_or(false) {
                    asked = true;
                    system
                        .frame_ready(*id, Timestamp(0))
                        .map_err(|e| fail(&e))?;
                }
            }
            if !asked {
                break;
            }
        }
        if window.is_some() {
            break;
        }
    }
    let window = window.ok_or_else(|| {
        let shown: Vec<String> = driver
            .runtime()
            .windows()
            .iter()
            .map(|(k, _)| format!("{:?}", k.0))
            .collect();
        format!("the run shows no window {key}, only {}", shown.join(", "))
    })?;
    presented(driver.runtime().window_system(), window)
}

/// The home a capture runs with, never the person's own: their settings and asset packs would change the pixels.
fn private_home() {
    let home = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/capture-home");
    std::env::set_var("HOME", &home);
    for (name, dir) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_CACHE_HOME", ".cache"),
        ("XDG_STATE_HOME", ".local/state"),
    ] {
        std::env::set_var(name, home.join(dir));
    }
}

fn main() -> ExitCode {
    private_home();
    cupertino_kit::test::capture::run(|| {
        cupertino_kit::test::capture::assets()?;
        Ok(Screenshot)
    })
}
