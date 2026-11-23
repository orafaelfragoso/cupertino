//! The glass the banners and the notification centre's cards name: the notification centre variant at the medium size;
//! in a collapsed group the sheets behind the front card step back (subdued), the front card does not.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, GlassModifiers, GlassSize, GlassVariant, RuntimeOptions, Window,
};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{GlassStyle, Timestamp, WindowRequest};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_notifications::view::{self, CardMetrics, Input, Ui};
use cupertino_notifications::{CenterConfig, NotificationCenter, NotifyInput};

/// The runtime turns between two looks at the windows.
const TURNS: u32 = 10_000;
/// The time the windows are looked at on the model's clock, in seconds: a banner that arrived at 0 has slid in and is
/// not yet due to leave.
const NOW: f64 = 1.0;
/// The menu bar's height the banners and the centre stand below.
const MENU_BAR: f64 = 24.0;

thread_local! {
    /// The model the next driver's windows show, and the metrics they lay out with.
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
        let (center, ui, metrics) = NEXT.take().expect("a model");
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

/// Options that draw on the CPU with the built-in font, in the neutral dark theme.
fn options() -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance: Appearance::Dark,
                reduce_transparency: false,
                reduce_motion: true,
            })
            .unwrap(),
        ),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 10_000,
        renderers: Box::new(move || {
            Box::new(CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 23,
                    glyph_cache_bytes: 1 << 20,
                    image_cache_bytes: 1 << 20,
                    font_smoothing: true,
                },
                GlyphRaster::new(Arc::clone(&glyphs)),
            ))
        }),
        accessibility: None,
        clock: Arc::new(ManualClock::new(Timestamp(0))),
        on_window_error: None,
    }
}

/// A notification of `app`.
fn note(app: &str, summary: &str) -> NotifyInput {
    NotifyInput::new(app, summary, "A line of text under it.")
}

/// The style, variant and modifiers of every glass region the first window asked for the last time it asked, with the
/// centre open (`centre`, three notifications of one application in a collapsed group) or a banner on screen.
fn glass(centre: bool) -> Vec<(GlassStyle, Option<(GlassVariant, GlassModifiers)>)> {
    let options = options();
    let mut center = NotificationCenter::new(CenterConfig::below_menu_bar(MENU_BAR));
    let mut ui = Ui::default();
    if centre {
        for (at, summary) in [(-300.0, "One"), (-200.0, "Two"), (-100.0, "Three")] {
            center.notify(note("Mail", summary), at);
        }
        center.tick(NOW);
        ui.open_centre(NOW, true);
    } else {
        center.notify(note("Mail", "One"), 0.0);
        center.tick(NOW);
    }
    let metrics = CardMetrics::new(Arc::clone(&options.theme), Arc::clone(&options.text));
    NEXT.set(Some((center, ui, metrics)));
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver =
        TestDriver::<Shown>::new(capabilities, options, ManualClock::new(Timestamp(0)));
    let turns = NonZeroU32::new(TURNS).unwrap();
    driver.run_until_idle(turns).unwrap();
    let window = driver.runtime().windows().first().expect("a window").1;
    driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    driver.run_until_idle(turns).unwrap();
    driver
        .window_system_mut()
        .requests(window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(regions) => Some(
                regions
                    .iter()
                    .filter_map(|r| r.glass)
                    .map(|g| (g.style, g.variant))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// The notification centre variant at the medium size, adapting to what lies behind it, stepping back (`subdued`) or
/// not.
fn card(subdued: bool) -> Option<(GlassVariant, GlassModifiers)> {
    Some((
        GlassVariant::NotificationCenter,
        GlassModifiers {
            subdued,
            adaptive: true,
            ..GlassModifiers::DEFAULT.sized(GlassSize::Medium)
        },
    ))
}

#[test]
fn a_banner_is_the_notification_centres_glass_at_the_medium_size() {
    let glass = glass(false);
    assert_eq!(glass, [(GlassStyle::Card, card(false))]);
}

#[test]
fn the_sheets_behind_a_groups_front_card_step_back() {
    let glass = glass(true);
    let front = glass.iter().filter(|g| g.1 == card(false)).count();
    let behind = glass.iter().filter(|g| g.1 == card(true)).count();
    assert_eq!(front, 1, "one front card: {glass:?}");
    assert!(behind >= 1, "the sheets behind it: {glass:?}");
    assert_eq!(front + behind, glass.len(), "no other glass: {glass:?}");
    assert!(glass.iter().all(|g| g.0 == GlassStyle::Card), "{glass:?}");
}
