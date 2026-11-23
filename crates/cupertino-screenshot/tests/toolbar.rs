//! The capture toolbar's body: regular glass over its whole window with the toolbar's corners; with Reduce
//! Transparency no glass, the body painted opaque in its place.

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, GlassModifiers, GlassVariant, RuntimeOptions, Window,
};
use cupertino_kit::geometry::Rect;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{GlassStyle, Timestamp, WindowId, WindowRequest};
use cupertino_kit::render::{CpuConfig, CpuRenderer, PixelFormat};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_screenshot::settings::Mode;
use cupertino_screenshot::values::{TOOLBAR_RADIUS, TOOLBAR_SIZE};
use cupertino_screenshot::view::{self, Input, ToolbarView};

/// The runtime turns between two looks at the window.
const TURNS: u32 = 1000;

/// The toolbar in selection mode, no timer running.
struct Toolbar;

impl Application for Toolbar {
    type Model = ();
    type Message = Input;

    fn init() -> ((), Vec<Command<Input>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: Input) -> Vec<Command<Input>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<Input>> {
        let shown = ToolbarView {
            mode: Mode::Selection,
            countdown: None,
        };
        vec![view::toolbar(shown, |input| input)]
    }
}

/// Options that draw on the CPU with the built-in font, in the neutral light theme, with or without Reduce
/// Transparency.
fn options(reduce_transparency: bool) -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance: Appearance::Light,
                reduce_transparency,
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

/// A material region the window asked for under part of it: its glass's style when it is glass, where, its corners and
/// its opacity.
#[derive(Debug)]
struct Region {
    glass: Option<GlassStyle>,
    variant: Option<(GlassVariant, GlassModifiers)>,
    rect: Rect,
    radius: f64,
    alpha: f64,
}

/// What the toolbar's window asked to have under it the last time it asked, and the alpha of its last frame's pixel in
/// the middle of its top row, above the controls, drawn on a desktop that shows materials.
fn drawn(reduce_transparency: bool) -> (Vec<Region>, u8) {
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver = TestDriver::<Toolbar>::new(
        capabilities,
        options(reduce_transparency),
        ManualClock::new(Timestamp(0)),
    );
    let turns = NonZeroU32::new(TURNS).unwrap();
    driver.run_until_idle(turns).unwrap();
    let window: WindowId = driver.runtime().windows()[0].1;
    driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    driver.run_until_idle(turns).unwrap();
    let system = driver.window_system_mut();
    let regions = system
        .requests(window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(regions) => Some(
                regions
                    .iter()
                    .map(|r| Region {
                        glass: r.glass.map(|g| g.style),
                        variant: r.glass.and_then(|g| g.variant),
                        rect: r.rect,
                        radius: r.radius,
                        alpha: r.alpha,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    let frame = system.frames(window).unwrap().last().expect("a frame");
    assert_eq!(frame.format, PixelFormat::Rgba8Premultiplied);
    let top = (frame.width + frame.width / 2) as usize * 4;
    (regions, frame.pixels[top + 3])
}

#[test]
fn the_toolbar_lies_on_regular_glass_with_its_corners() {
    let (regions, alpha) = drawn(false);
    assert_eq!(regions.len(), 1, "one glass region: {regions:?}");
    let region = &regions[0];
    assert_eq!(region.glass, Some(GlassStyle::Regular), "regular glass");
    assert_eq!(
        region.variant,
        Some((GlassVariant::Regular, GlassModifiers::DEFAULT)),
        "named as the regular variant at its own size, not adaptive"
    );
    assert_eq!(
        region.rect,
        Rect::new(0.0, 0.0, TOOLBAR_SIZE.0, TOOLBAR_SIZE.1)
    );
    assert_eq!(region.radius, TOOLBAR_RADIUS);
    assert_eq!(region.alpha, 1.0);
    // the glass is the body: the window paints none of its own around the controls
    assert_eq!(alpha, 0, "no body painted over the glass");
}

#[test]
fn with_reduce_transparency_the_toolbar_paints_an_opaque_body_and_asks_for_no_glass() {
    let (regions, alpha) = drawn(true);
    assert!(regions.is_empty(), "no glass: {regions:?}");
    assert_eq!(alpha, 255, "an opaque body");
}
