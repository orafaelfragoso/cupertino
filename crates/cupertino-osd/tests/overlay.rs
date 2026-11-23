//! The volume and brightness overlay's body: the glass of a notification banner over its whole window with the window's
//! corners, faded with the overlay; with Reduce Transparency no glass, the body painted opaque in its place; hidden, no
//! glass at all.

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, GlassModifiers, GlassSize, GlassVariant, RuntimeOptions, Window,
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
use cupertino_osd::layout::{CORNER_RADIUS, WINDOW_SIZE};
use cupertino_osd::model::{Kind, Overlay, View};
use cupertino_osd::view;

/// The runtime turns between two looks at the window.
const TURNS: u32 = 1000;
/// The opacity of the fade the shown overlay is caught at.
const FADING: f64 = 0.5;

/// The overlay shown at half volume half way through its fade, or hidden.
struct Shown<const VISIBLE: bool>;

impl<const VISIBLE: bool> Application for Shown<VISIBLE> {
    type Model = Option<View>;
    type Message = ();

    fn init() -> (Option<View>, Vec<Command<()>>) {
        let view = VISIBLE.then(|| View {
            overlay: Overlay::new(Kind::Volume, 0.5).unwrap(),
            alpha: FADING,
        });
        (view, Vec::new())
    }

    fn update(_: &mut Option<View>, _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(view: &Option<View>) -> Vec<Window<()>> {
        vec![view::window(*view, None)]
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

/// What the overlay's window asked to have under it the last time it asked, and the alpha of its last frame's middle
/// pixel, drawn on a desktop that shows materials.
fn drawn<A: Application>(reduce_transparency: bool) -> (Vec<Region>, u8) {
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver = TestDriver::<A>::new(
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
    let middle = (frame.height / 2 * frame.width + frame.width / 2) as usize * 4;
    (regions, frame.pixels[middle + 3])
}

#[test]
fn the_overlay_lies_on_notification_banner_glass_with_its_corners_and_fade() {
    let (regions, alpha) = drawn::<Shown<true>>(false);
    let side = f64::from(WINDOW_SIZE);
    assert_eq!(regions.len(), 1, "one glass region: {regions:?}");
    let region = &regions[0];
    assert_eq!(
        region.glass,
        Some(GlassStyle::Card),
        "the body is banner glass"
    );
    assert_eq!(
        region.variant,
        Some((
            GlassVariant::NotificationCenter,
            GlassModifiers::DEFAULT.sized(GlassSize::Medium)
        )),
        "named as the notification centre variant at the medium size, not adaptive"
    );
    assert_eq!(region.rect, Rect::new(0.0, 0.0, side, side));
    assert_eq!(region.radius, CORNER_RADIUS);
    assert_eq!(region.alpha, FADING);
    // the glass is the body: the window paints none of its own under the glyph and the meter
    assert_eq!(alpha, 0, "no body painted over the glass");
}

#[test]
fn with_reduce_transparency_the_overlay_paints_an_opaque_body_and_asks_for_no_glass() {
    let (regions, alpha) = drawn::<Shown<true>>(true);
    assert!(regions.is_empty(), "no glass: {regions:?}");
    let expected = (FADING * 255.0).round();
    assert!(
        (f64::from(alpha) - expected).abs() <= 1.0,
        "the body at the fade's opacity: {alpha}"
    );
}

#[test]
fn the_hidden_overlay_has_no_glass() {
    let (regions, _) = drawn::<Shown<false>>(false);
    assert!(regions.is_empty(), "no glass while hidden: {regions:?}");
}
