//! A confirmation panel's body names the glass window variant, the glass of an alert, with the panel's corners; the
//! compositor gives it the alert's drop shadow, so no modifier leaves the variant's out.

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, GlassModifiers, GlassVariant, Key, RuntimeOptions, Window, WindowKey,
};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{GlassStyle, Timestamp, WindowRequest};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_shell_agent::panel::Kind;
use cupertino_shell_agent::view::{panel, panel_frames};

/// The runtime turns between two looks at the window.
const TURNS: u32 = 1000;

/// The text system of the built-in font.
fn text() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter Variable"))
}

/// The log-out confirmation panel.
struct Panel;

impl Application for Panel {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let frames = panel_frames(Kind::LogOut, &text());
        vec![panel(
            WindowKey(Key::from("panel")),
            Kind::LogOut,
            Some(60),
            true,
            &frames,
            |_| (),
        )]
    }
}

/// Options that draw on the CPU with the built-in font, in the neutral dark theme.
fn options() -> RuntimeOptions {
    let text = text();
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
fn a_confirmation_panel_lies_on_the_glass_window_variant() {
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver =
        TestDriver::<Panel>::new(capabilities, options(), ManualClock::new(Timestamp(0)));
    let turns = NonZeroU32::new(TURNS).unwrap();
    driver.run_until_idle(turns).unwrap();
    let window = driver.runtime().windows()[0].1;
    driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    driver.run_until_idle(turns).unwrap();
    let glass: Vec<_> = driver
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
        .unwrap_or_default();
    // the panel's body; its buttons are Kit's controls and name their own glass
    assert!(
        glass.contains(&(
            GlassStyle::Regular,
            Some((GlassVariant::GlassWindow, GlassModifiers::DEFAULT))
        )),
        "{glass:?}"
    );
}
