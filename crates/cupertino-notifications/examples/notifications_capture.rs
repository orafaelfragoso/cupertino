//! The banners and the notification list drawn to PNGs for look checks:
//! `cargo run -p cupertino-notifications --example notifications_capture -- notifications/SURFACE [flags]`
//! (crates/cupertino-kit/README.md).
//!
//! Two surfaces: `banner`, the window of the banner on screen, and `centre`, the open notification list.
//! `--kind` names the banner (`plain`, the default, `actions`, `long` or `critical`); `--expanded` names the list's
//! group that is expanded (`none`, the default, or `mail`); `--moment` names the time of the picture (`rest`, the
//! default, every motion at rest, or `moving`, the banner, the list and the expanding group part way through their
//! motion). The notifications, their times and the clock are fixed, so a command gives the same pixels on every run. The fonts, symbols and theme are the
//! system's, as an application's. Each picture is written as one PNG; there is no contact sheet.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::render::ImageResource;
use cupertino_kit::test::capture::{self, Look, Options, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_notifications::view::{self, CardMetrics, Input, Ui};
use cupertino_notifications::{CenterConfig, NotificationCenter, NotifyInput, Urgency};

/// The app's part of the surface and file names.
const NAME: &str = "notifications";
/// The state flags, without their dashes.
const STATE_FLAGS: &[&str] = &["kind", "expanded", "moment"];
const SURFACES: [&str; 2] = ["banner", "centre"];
const KINDS: [&str; 4] = ["plain", "actions", "long", "critical"];
const MOMENTS: [&str; 2] = ["rest", "moving"];
/// How long before the picture a motion started at the `moving` moment, in seconds: part way through each of them.
const MOVING_FOR: f64 = 0.08;
/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames a window may ask for before it rests.
const FRAMES: usize = 16;
/// The time of the picture on the model's clock, in seconds: a banner that arrived at 0 has slid in and is not yet due
/// to leave.
const NOW: f64 = 1.0;

thread_local! {
    /// The model the next capture's windows show, and the metrics they lay out with.
    static NEXT: RefCell<Option<(NotificationCenter, Ui, CardMetrics)>> = const { RefCell::new(None) };
}

/// The windows of [`NEXT`].
struct Shown;

struct Model {
    center: RefCell<NotificationCenter>,
    ui: Ui,
    metrics: CardMetrics,
}

impl Application for Shown {
    type Model = Model;
    type Message = Input;

    fn init() -> (Model, Vec<Command<Input>>) {
        let (center, ui, metrics) = NEXT
            .with_borrow_mut(Option::take)
            .expect("a model for the capture");
        let model = Model {
            center: RefCell::new(center),
            ui,
            metrics,
        };
        (model, Vec::new())
    }

    fn update(_: &mut Model, _: Input) -> Vec<Command<Input>> {
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Input>> {
        let mut center = model.center.borrow_mut();
        view::windows(
            &mut center,
            &model.ui,
            &model.metrics,
            NOW,
            std::convert::identity,
        )
    }
}

/// The value of state flag `flag`, or `default`; an error unless it is one of `values`.
fn flag<'a>(state: &'a State, flag: &str, values: &[&'a str]) -> Result<&'a str, String> {
    match state.iter().find(|(f, _)| f == flag) {
        None => Ok(values[0]),
        Some((_, value)) if values.contains(&value.as_str()) => Ok(value),
        Some((_, value)) => Err(format!("--{flag} {value}: one of {}", values.join(", "))),
    }
}

/// The banner of `kind`.
fn banner(kind: &str) -> NotifyInput {
    let mut input = NotifyInput::new(
        "Mail",
        "Lunch on Friday",
        "Are you free at noon? We could try the new place by the river.",
    );
    input.icon = "mail".to_owned();
    match kind {
        "actions" => input
            .with_action("reply", "Reply")
            .with_action("later", "Later"),
        "long" => {
            input.summary =
                "The minutes of this morning's planning meeting, with every decision".to_owned();
            input.body = "We went through the whole of the quarter's plan. The release moves a week, the \
                          design review stays on Tuesday, and the new hires start on the first of the month. \
                          Please read the notes before Thursday and add what is missing."
                .to_owned();
            input
        }
        "critical" => {
            input.app_name = "Battery".to_owned();
            input.summary = "Your battery is low".to_owned();
            input.body = "Connect your computer to power soon.".to_owned();
            input.with_urgency(Urgency::Critical)
        }
        _ => input,
    }
}

/// The list's notifications: three of Mail, which stack in one group, one of Calendar and one of Messages with a
/// reply action, newest last.
fn list() -> Vec<(f64, NotifyInput)> {
    let mail = |summary: &str, body: &str| {
        let mut input = NotifyInput::new("Mail", summary, body);
        input.icon = "mail".to_owned();
        input
    };
    let mut calendar = NotifyInput::new("Calendar", "Design review", "Tomorrow at 10:00, room 4");
    calendar.icon = "calendar".to_owned();
    let mut messages = NotifyInput::new("Messages", "Ana", "See you at the station at six?")
        .with_action("reply", "Reply");
    messages.icon = "message".to_owned();
    vec![
        (
            -3600.0,
            mail("Invoice 2041", "Your invoice for October is ready."),
        ),
        (-1800.0, calendar),
        (-900.0, mail("Lunch on Friday", "Are you free at noon?")),
        (-300.0, messages),
        (
            -60.0,
            mail(
                "Re: the plan",
                "Looks good to me, thanks for writing it up.",
            ),
        ),
    ]
}

/// The model of a capture of `surface` in `state`.
fn model(
    surface: &str,
    state: &State,
    config: CenterConfig,
) -> Result<(NotificationCenter, Ui), String> {
    let mut center = NotificationCenter::new(config);
    let mut ui = Ui::default();
    let moving = flag(state, "moment", &MOMENTS)? == "moving";
    match surface {
        "banner" => {
            // a banner at rest arrived at 0, long enough ago for its slide to have ended
            let arrived = if moving { NOW - MOVING_FOR } else { 0.0 };
            center.notify(banner(flag(state, "kind", &KINDS)?), arrived);
            center.tick(NOW);
            Ok((center, ui))
        }
        "centre" => {
            for (at, input) in list() {
                center.notify(input, at);
            }
            center.tick(NOW);
            // the list as it stands once it has slid in, or part way in
            if moving {
                ui.open_centre(NOW - MOVING_FOR, false);
            } else {
                ui.open_centre(NOW, true);
            }
            if flag(state, "expanded", &["none", "mail"])? == "mail" {
                let began = if moving {
                    NOW - MOVING_FOR
                } else {
                    f64::NEG_INFINITY
                };
                ui.expanded.insert("Mail".to_owned(), began);
            }
            Ok((center, ui))
        }
        _ => Err(format!(
            "{NAME}/{surface}: not a surface (`capture {NAME}` lists them)"
        )),
    }
}

/// One picture of `surface` in `state` and `look`, at `scale` pixels a point.
// a capture loads the theme of the appearance it draws, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn draw(surface: &str, state: &State, look: Look, scale: f32) -> Result<ImageResource, String> {
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
    let (center, ui) = model(surface, state, CenterConfig::from_theme(&theme))?;
    let metrics = CardMetrics::new(Arc::clone(&options.theme), Arc::clone(&options.text));
    NEXT.set(Some((center, ui, metrics)));
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
        .ok_or("the model shows no window")?
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
    let system = driver.runtime().window_system();
    let frame = system
        .frames(window)
        .map_err(|e| fail(&e))?
        .last()
        .ok_or("the window presented no frame")?;
    let pixels = match system.composited(window) {
        Ok(Some(composited)) => composited,
        _ => frame.pixels.clone(),
    };
    Ok(ImageResource {
        revision: 1,
        width: frame.width,
        height: frame.height,
        stride: frame.width * 4,
        format: frame.format,
        pixels: Arc::from(pixels),
    })
}

/// Writes `image` as the PNG `name` in `out`.
// the example writes its pictures itself, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn save(image: &ImageResource, out: &Path, name: &str) -> Result<(), String> {
    let png = cupertino_kit::image::encode_png(image).map_err(|e| format!("{name}: {e:?}"))?;
    std::fs::create_dir_all(out).map_err(|e| format!("creating {}: {e}", out.display()))?;
    let path = out.join(name);
    std::fs::write(&path, png).map_err(|e| format!("writing {}: {e}", path.display()))?;
    println!("{}", path.display());
    Ok(())
}

/// Carries out `options`.
fn capture(options: &Options) -> Result<(), String> {
    let Some(surface) = options.surface.as_deref() else {
        for surface in SURFACES {
            println!("{NAME}/{surface}");
        }
        return Ok(());
    };
    if options.sheet {
        return Err(
            "--sheet: this example writes one PNG per picture and no contact sheet".to_owned(),
        );
    }
    if options.size.is_some() {
        return Err("--size: the windows take the size of what they show".to_owned());
    }
    for &dark in &options.dark {
        let look = Look {
            dark,
            increase_contrast: options.increase_contrast,
            reduce_transparency: options.reduce_transparency,
        };
        let image = draw(surface, &options.state, look, options.scale)?;
        let name = capture::file_name(NAME, surface, &options.state, look, options.scale);
        save(&image, &options.out, &name)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let result = capture::assets()
        .and_then(|_| capture::parse(NAME, STATE_FLAGS, std::env::args().skip(1)))
        .and_then(|options| capture(&options));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("capture: {e}");
            ExitCode::from(2)
        }
    }
}
