//! An alert's body names the glass window variant, the glass of an alert, with its corners; the compositor gives it the
//! alert's drop shadow, so no modifier leaves the variant's out.

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, CommandRegistry, GlassModifiers, GlassVariant, Key, RuntimeOptions,
    Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{DecorationMode, GlassStyle, Timestamp, WindowRequest, WindowSpec};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_prompts::view::widgets::AlertBody;

/// The runtime turns between two looks at the window.
const TURNS: u32 = 1000;
/// The alert's width and corner radius.
const WIDTH: f64 = 260.0;
const RADIUS: f64 = 16.0;

/// One window whose content is an empty alert body.
struct Alert;

impl Application for Alert {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        vec![Window {
            key: WindowKey(Key::from("alert")),
            spec: WindowSpec {
                title: "Alert".to_owned(),
                app_id: String::new(),
                size: Size::new(WIDTH, 120.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::None,
            content: cupertino_kit::core::Element::new(AlertBody {
                width: WIDTH,
                radius: RADIUS,
                padding: Insets::ZERO,
                children: Vec::new(),
            }),
            popup: None,
            menus: None,
        }]
    }
}

/// Options that draw on the CPU with the built-in font, in the neutral light theme.
fn options() -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance: Appearance::Light,
                reduce_transparency: false,
                reduce_motion: true,
            })
            .unwrap(),
        ),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 1000,
        renderers: Box::new(move || {
            Box::new(CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 22,
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

#[test]
fn an_alert_lies_on_the_glass_window_variant_with_its_corners() {
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver =
        TestDriver::<Alert>::new(capabilities, options(), ManualClock::new(Timestamp(0)));
    let turns = NonZeroU32::new(TURNS).unwrap();
    driver.run_until_idle(turns).unwrap();
    let window = driver.runtime().windows()[0].1;
    driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    driver.run_until_idle(turns).unwrap();
    let regions: Vec<_> = driver
        .window_system_mut()
        .requests(window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(regions) => Some(
                regions
                    .iter()
                    .map(|r| (r.radius, r.glass.map(|g| (g.style, g.variant))))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    assert_eq!(
        regions,
        [(
            RADIUS,
            Some((
                GlassStyle::Regular,
                Some((GlassVariant::GlassWindow, GlassModifiers::DEFAULT))
            ))
        )]
    );
}
