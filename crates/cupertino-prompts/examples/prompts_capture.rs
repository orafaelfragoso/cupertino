//! The prompts drawn to PNGs for look checks:
//! `cargo run -p cupertino-prompts --example prompts_capture -- prompts/SURFACE [flags]` (crates/cupertino-kit/README.md).
//!
//! Surfaces: `authentication` (`--details shown` opens its details, `--users two` offers a second user), `keyring`
//! (`--kind password`, the default, unlocks a keyring; `new` asks for a new password and its confirmation; `confirm`
//! asks a question with no field), and the portal's consent questions `screenshot` and `shortcuts` (armed, with the
//! applications symbol for the app's icon). Each is the alert's surface as the server shows it over the scrim, without the
//! scrim. The fonts, symbols and theme are the system's, as an application's; nothing moves, so a command gives the same
//! pixels on every run.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_kit::core::{Application, Command, Key, Window, WindowKey};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::{SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_prompts::consent::{self, Question, Row};
use cupertino_prompts::program::Message;
use cupertino_prompts::view;

/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames the window may ask for before it rests.
const FRAMES: usize = 16;

/// What a capture shows.
#[derive(Clone)]
enum Shown {
    Authentication(view::Authentication),
    Keyring(view::Keyring),
    Consent(view::consent::Consent),
}

thread_local! {
    /// What the next capture's window shows.
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

/// The alert's window showing [`SHOWN`].
struct Alert;

impl Application for Alert {
    type Model = Option<Shown>;
    type Message = Message;

    fn init() -> (Option<Shown>, Vec<Command<Message>>) {
        (SHOWN.with(|s| s.borrow().clone()), Vec::new())
    }

    fn update(_: &mut Option<Shown>, _: Message) -> Vec<Command<Message>> {
        Vec::new()
    }

    fn view(shown: &Option<Shown>) -> Vec<Window<Message>> {
        let key = WindowKey(Key::from("prompt"));
        match shown {
            Some(Shown::Authentication(prompt)) => view::authentication(key, prompt)
                .into_iter()
                .take(1)
                .collect(),
            Some(Shown::Keyring(prompt)) => vec![view::keyring(key, prompt)],
            Some(Shown::Consent(prompt)) => vec![view::consent::consent(key, prompt)],
            None => Vec::new(),
        }
    }
}

/// An authentication of a package manager's action, as polkitd asks it.
fn authentication(details: bool, two_users: bool) -> view::Authentication {
    let mut users = vec!["Alex Appleseed".to_owned()];
    if two_users {
        users.push("Administrator".to_owned());
    }
    view::Authentication {
        message: "Authentication is required to install software.".to_owned(),
        users,
        user: 0,
        menu: false,
        question: view::field_label("Password: "),
        echo: None,
        asked: 1,
        asking: true,
        can_submit: false,
        checking: false,
        failed: false,
        status: Vec::new(),
        details: vec![
            ("Application", "/usr/bin/pkexec".to_owned()),
            (
                "Action",
                "org.freedesktop.packagekit.package-install".to_owned(),
            ),
        ],
        details_shown: details,
    }
}

/// A keyring prompt of `kind`, as the keyring daemon asks it.
fn keyring(kind: &str) -> Result<view::Keyring, String> {
    let (heading, description, password, confirm) = match kind {
        "password" => (
            "Unlock Login Keyring",
            "The login keyring did not get unlocked when you logged into your computer.",
            true,
            false,
        ),
        "new" => (
            "Choose a password for the new keyring",
            "An application wants to create a new keyring called \u{201c}Work\u{201d}.",
            true,
            true,
        ),
        "confirm" => (
            "Move the items of \u{201c}Default keyring\u{201d} to the login keyring?",
            "Logging in unlocks the login keyring.",
            false,
            false,
        ),
        other => return Err(format!("--kind {other}: one of password, new, confirm")),
    };
    Ok(view::Keyring {
        heading: heading.to_owned(),
        description: description.to_owned(),
        password,
        confirm,
        choice: None,
        status: Vec::new(),
        continue_label: if password { "Continue" } else { "Move Items" }.to_owned(),
        cancel_label: if password {
            "Cancel"
        } else {
            "Don\u{2019}t Move"
        }
        .to_owned(),
        asked: 1,
        checking: false,
        mismatch: false,
    })
}

/// A consent question of an app named `name`, armed.
fn consent_question(
    name: &str,
    question: impl FnOnce(consent::App) -> Question,
) -> view::consent::Consent {
    let app = consent::App {
        id: format!("org.example.{name}"),
        name: Some(name.to_owned()),
        icon: None,
    };
    view::consent::Consent {
        question: question(app),
        icon: None,
        armed: true,
        scroll: ScrollState {
            offset: Point::ZERO,
            content: Size::ZERO,
            viewport: Size::ZERO,
        },
    }
}

/// The shortcuts of a call app, as the portal words them: two that get their keys and one that does not.
fn shortcut_rows() -> Vec<Row> {
    let row = |keys: &str, description: &str, note: &str| Row {
        description: description.to_owned(),
        keys: keys.to_owned(),
        note: note.to_owned(),
    };
    vec![
        row("\u{2303}\u{2325}T", "Push to talk", ""),
        row("\u{2303}\u{2318}M", "Mute the microphone", ""),
        row(
            "H",
            "Raise a hand",
            "not available: it needs \u{2303}, \u{2325} or \u{2318}",
        ),
    ]
}

fn flag<'a>(request: &'a Request<'_>, name: &str) -> Option<&'a str> {
    request
        .state
        .iter()
        .find(|(f, _)| f == name)
        .map(|(_, v)| v.as_str())
}

struct Prompts;

impl App for Prompts {
    const NAME: &'static str = "prompts";
    const STATE_FLAGS: &'static [&'static str] = &["details", "users", "kind"];

    fn surfaces(&self) -> Vec<String> {
        ["authentication", "keyring", "screenshot", "shortcuts"]
            .map(str::to_owned)
            .to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        let state = |flag: &str, value: &str| vec![(flag.to_owned(), value.to_owned())];
        match surface {
            "authentication" => vec![state("details", "hidden"), state("details", "shown")],
            "screenshot" | "shortcuts" => vec![Vec::new()],
            _ => ["password", "new", "confirm"]
                .map(|kind| state("kind", kind))
                .to_vec(),
        }
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let shown = match request.surface {
            "authentication" => {
                let details = match flag(request, "details") {
                    None | Some("hidden") => false,
                    Some("shown") => true,
                    Some(other) => return Err(format!("--details {other}: hidden or shown")),
                };
                let two = match flag(request, "users") {
                    None | Some("one") => false,
                    Some("two") => true,
                    Some(other) => return Err(format!("--users {other}: one or two")),
                };
                Shown::Authentication(authentication(details, two))
            }
            "keyring" => Shown::Keyring(keyring(flag(request, "kind").unwrap_or("password"))?),
            "screenshot" => Shown::Consent(consent_question("Notes", |app| Question::Screenshot {
                app,
            })),
            "shortcuts" => Shown::Consent(consent_question("Talk", |app| Question::Shortcuts {
                app,
                rows: shortcut_rows(),
                allowed: 0,
            })),
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
    let mut driver = TestDriver::<Alert>::new(
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
        Ok(Prompts)
    })
}
