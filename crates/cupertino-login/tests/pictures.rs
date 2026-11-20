//! The screens' pictures (`view::ScreenPictures`): what a screen wants comes back as commands that make it on
//! the CPU pool, the pictures made make the screen ready, a second screen of the same size, scale and wallpaper takes the
//! first one's, and no more than a few pictures are kept besides those shown.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, Environment, Key, RuntimeOptions, Window, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::Timestamp;
use cupertino_kit::test::{Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver};
use cupertino_kit::text::TextSystem;
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_login::user::User;
use cupertino_login::view::{
    Access, Clock, Field, Made, Metrics, Screen, ScreenPictures, Status, Wants,
};
use cupertino_login::wallpaper::Source;

/// How long the test waits for the CPU pool.
const PATIENCE: Duration = Duration::from_secs(60);
/// The screens' size, points: small, so the pictures are quick to make.
const SIZE: Size = Size::new(400.0, 300.0);
/// The most pictures the screens keep besides those shown.
const KEPT: usize = 8;
/// Screens of wallpapers of their own, more than are kept.
const MANY: usize = 12;

fn environment() -> Environment {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    Environment {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance: Appearance::Dark,
                reduce_transparency: false,
                reduce_motion: true,
            })
            .unwrap(),
        ),
        text: Arc::new(TextSystem::new(fonts, "Inter Variable")),
        direction: LayoutDirection::Ltr,
        scale: 1.0,
        icons: Arc::new(PlaceholderIcons::default()),
    }
}

fn key(name: &str) -> WindowKey {
    WindowKey(Key::from(name.to_owned()))
}

/// What a lock screen on \`wallpaper\` with its dimming in wants at \`scale\`.
fn wants(env: &Environment, wallpaper: &Source, scale: f64) -> Wants {
    let pictures = ScreenPictures::new();
    let user = User::typed("ada".into());
    let clock = Clock {
        date: "Mon Nov 16".into(),
        time: "10:24".into(),
    };
    Screen {
        size: SIZE,
        metrics: Metrics::of(&env.text),
        clock: &clock,
        user: Some(&user),
        field: Field::Password {
            generation: 0,
            enabled: true,
            busy: false,
        },
        name: None,
        typed: false,
        caps_lock: false,
        spinner: None,
        denied: false,
        denied_said: false,
        notice: None,
        help: false,
        help_open: false,
        opacity: 1.0,
        block: 1.0,
        shake: 0.0,
        hidden: false,
        lock_message: None,
        back: false,
        locked: false,
        hide_user: false,
        access: Access::default(),
        large_clock: true,
        avatar_from: None,
        power: None,
        status: Status::NONE,
        notices: &[],
        wallpaper,
        dim: 1.0,
        pictures: pictures.shown(&key("none")),
    }
    .wants(scale)
}

/// The steps the test takes, each once the pictures of the one before are in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Screen \`a\` alone.
    One,
    /// Screen \`b\` of the same size, scale and wallpaper beside it.
    Two,
    /// [\`MANY\`] screens, each of a wallpaper of its own.
    Many,
    Done,
}

struct Model {
    pictures: ScreenPictures,
    env: Environment,
    step: Step,
    /// How many commands each step's [\`ScreenPictures::want\`] gave.
    issued: Vec<usize>,
    /// Whether \`b\` was ready as soon as it was named.
    b_ready_at_once: bool,
    /// The pictures kept after each arrival, at most.
    most_kept: usize,
}

fn screens(model: &Model) -> Vec<(WindowKey, Wants)> {
    let solid = Source::Solid([40, 60, 120]);
    match model.step {
        Step::One => vec![(key("a"), wants(&model.env, &solid, 1.0))],
        Step::Two => vec![
            (key("a"), wants(&model.env, &solid, 1.0)),
            (key("b"), wants(&model.env, &solid, 1.0)),
        ],
        Step::Many | Step::Done => (0..MANY)
            .map(|i| {
                let source = Source::Solid([i as u8 * 10, 20, 30]);
                (key(&format!("many-{i}")), wants(&model.env, &source, 1.0))
            })
            .collect(),
    }
}

fn want(model: &mut Model) -> Vec<Command<Made>> {
    let screens = screens(model);
    let commands = model.pictures.want(&model.env, screens, |made| made);
    model.issued.push(commands.len());
    commands
}

fn all_ready(model: &Model) -> bool {
    screens(model)
        .iter()
        .all(|(key, _)| model.pictures.ready(key))
}

struct Pictures;

impl Application for Pictures {
    type Model = Model;
    type Message = Made;
    const RESIDENT: bool = true;

    fn init() -> (Model, Vec<Command<Made>>) {
        let mut model = Model {
            pictures: ScreenPictures::new(),
            env: environment(),
            step: Step::One,
            issued: Vec::new(),
            b_ready_at_once: false,
            most_kept: 0,
        };
        let commands = want(&mut model);
        (model, commands)
    }

    fn update(model: &mut Model, made: Made) -> Vec<Command<Made>> {
        let mut commands = model.pictures.made(&model.env, made, |made| made);
        model.most_kept = model.most_kept.max(model.pictures.kept());
        if commands.is_empty() && all_ready(model) {
            match model.step {
                Step::One => {
                    model.step = Step::Two;
                    commands = want(model);
                    model.b_ready_at_once = model.pictures.ready(&key("b"));
                    // the second screen waits for nothing: on to the many at once
                    model.step = Step::Many;
                    commands.extend(want(model));
                }
                Step::Many => model.step = Step::Done,
                Step::Two | Step::Done => {}
            }
        }
        commands
    }

    fn view(_: &Model) -> Vec<Window<Made>> {
        Vec::new()
    }
}

fn driver() -> TestDriver<Pictures> {
    let env = environment();
    let options = RuntimeOptions {
        theme: env.theme,
        text: env.text,
        icons: env.icons,
        direction: LayoutDirection::Ltr,
        node_limit: 1000,
        // the application has no window
        renderers: Box::new(|| unreachable!("no window is drawn")),
        accessibility: None,
        clock: Arc::new(ManualClock::new(Timestamp(0))),
        on_window_error: None,
    };
    TestDriver::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    )
}

#[test]
fn wanted_pictures_are_made_shared_by_screens_of_the_same_key_and_kept_within_bounds() {
    let mut driver = driver();
    let end = Instant::now() + PATIENCE;
    loop {
        driver
            .run_until_idle(NonZeroU32::new(10_000).unwrap())
            .unwrap();
        if driver.runtime().model().step == Step::Done {
            break;
        }
        let left = end.saturating_duration_since(Instant::now());
        assert!(
            driver.window_system_mut().wait_for_wake(left),
            "the pictures were not made within {PATIENCE:?}"
        );
    }
    let model = driver.runtime().model();
    // the first screen's picture, its materials and its glass letters were asked for
    assert!(model.issued[0] > 0, "{:?}", model.issued);
    // the second screen of the same key asked for nothing and was ready at once
    assert_eq!(model.issued[1], 0, "{:?}", model.issued);
    assert!(model.b_ready_at_once);
    // the screens of wallpapers of their own each asked for theirs
    assert!(model.issued[2] > 0, "{:?}", model.issued);
    assert!(all_ready(model));
    assert!(model.most_kept <= KEPT, "{} kept", model.most_kept);
    // one whole picture made for a and b, and one for each of the many
    assert_eq!(model.pictures.pictures_made(), 1 + MANY as u64);
    // the screens not named in the last step are forgotten
    assert!(!model.pictures.ready(&key("a")));
    assert!(model.pictures.shown(&key("many-0")).ready());
}
