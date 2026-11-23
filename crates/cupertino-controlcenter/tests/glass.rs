//! The panel's glass: faint clear glass of its own behind the tiles, in the outline of the opaque panel of Reduce
//! Transparency, faded with the panel and never in the tiles' container; the tiles' module glass over it in one
//! container; with Reduce Transparency no glass at all. The tiles' labels over that glass.

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_controlcenter::arrangement::Arrangement;
use cupertino_controlcenter::layout::{GROUP_RADIUS, INSET, WINDOW, WINDOW_ORIGIN};
use cupertino_controlcenter::model::Controls;
use cupertino_controlcenter::view::{self, BandPictures, Input, Presentation};
use cupertino_kit::core::{Application, Command, RuntimeOptions, Window};
use cupertino_kit::geometry::Rect;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{GlassStyle, Timestamp, WindowId, WindowRequest};
use cupertino_kit::render::{CpuConfig, CpuRenderer, PaintOp};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, Color, ThemePreferences, ThemeTokens};

/// The runtime turns between two looks at the window.
const TURNS: u32 = 10_000;
/// The panel's opacity the capture catches it at, part way through its fade.
const FADING: f64 = 0.75;

/// The panel with nothing read yet, part way through its fade in.
struct Panel;

impl Application for Panel {
    type Model = ();
    type Message = Input;

    fn init() -> ((), Vec<Command<Input>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: Input) -> Vec<Command<Input>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<Input>> {
        let presentation = Presentation {
            alpha: FADING,
            ..Presentation::SHOWN
        };
        vec![view::arranged(
            &Controls::unread(),
            &Arrangement::default(),
            presentation,
            &BandPictures::default(),
            |input| input,
        )]
    }
}

/// Options that draw on the CPU with the built-in font, in the neutral dark theme, with or without Reduce
/// Transparency.
fn options(reduce_transparency: bool) -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance: Appearance::Dark,
                reduce_transparency,
                reduce_motion: true,
            })
            .unwrap(),
        ),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 4000,
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

/// A glass region the window asked for: its style, container, where, its corners and its opacity.
#[derive(Debug)]
struct Region {
    style: GlassStyle,
    container: u32,
    rect: Rect,
    radius: f64,
    alpha: f64,
}

/// The panel shown and drawn on a desktop that shows materials, and its window.
fn shown(reduce_transparency: bool) -> (TestDriver<Panel>, WindowId) {
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver = TestDriver::<Panel>::new(
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
    (driver, window)
}

/// The glass regions the panel's window asked for the last time it asked, on a desktop that shows materials.
fn glass(reduce_transparency: bool) -> Vec<Region> {
    let (mut driver, window) = shown(reduce_transparency);
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
                    .filter_map(|r| {
                        r.glass.map(|g| Region {
                            style: g.style,
                            container: g.container,
                            rect: r.rect,
                            radius: r.radius,
                            alpha: r.alpha,
                        })
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn the_tiles_lie_on_a_clear_glass_panel_of_its_own() {
    let regions = glass(false);
    let panels: Vec<&Region> = regions
        .iter()
        .filter(|r| r.style == GlassStyle::Clear)
        .collect();
    assert_eq!(panels.len(), 1, "one clear panel: {regions:?}");
    let panel = panels[0];
    assert_eq!(panel.container, 0, "the panel is glass on its own");
    assert_eq!(panel.radius, GROUP_RADIUS);
    assert!(
        panel.alpha > 0.0 && panel.alpha <= 0.5 * FADING,
        "the clear panel shows faintly, never as a plate: {panel:?}"
    );
    assert!(
        (panel.alpha - FADING * view::PANEL_GLASS).abs() < 1e-9,
        "the panel fades with the card at its faint strength: {panel:?}"
    );
    assert_eq!(
        (panel.rect.x(), panel.rect.y(), panel.rect.width()),
        (WINDOW_ORIGIN.x, WINDOW_ORIGIN.y, WINDOW.width)
    );
    let tiles: Vec<&Region> = regions
        .iter()
        .filter(|r| r.style == GlassStyle::Module)
        .collect();
    assert!(!tiles.is_empty(), "module tiles: {regions:?}");
    for tile in &tiles {
        assert_ne!(tile.container, 0, "the tiles join a container");
        assert_ne!(tile.container, panel.container);
        let (outer, inner) = (panel.rect, tile.rect);
        let tolerance = 1e-9;
        assert!(
            inner.min_x() >= outer.min_x() + INSET - tolerance
                && inner.min_y() >= outer.min_y() + INSET - tolerance
                && inner.max_x() <= outer.max_x() - INSET + tolerance
                && inner.max_y() <= outer.max_y() - INSET + tolerance,
            "{tile:?} lies inside the panel's inset {panel:?}"
        );
    }
    let first = regions
        .iter()
        .position(|r| r.style == GlassStyle::Module)
        .unwrap();
    let behind = regions
        .iter()
        .position(|r| r.style == GlassStyle::Clear)
        .unwrap();
    assert!(behind < first, "the panel lies behind the tiles");
}

#[test]
fn with_reduce_transparency_the_panel_asks_for_no_glass() {
    let regions = glass(true);
    assert!(regions.is_empty(), "no glass: {regions:?}");
}

#[test]
fn a_tiles_second_line_is_drawn_in_its_titles_colour() {
    let (driver, window) = shown(false);
    let snapshot = driver.runtime().snapshot(window).unwrap();
    // every text run of the panel at rest by its size: the titles in the body size, the tiles' second lines smaller
    let mut runs: Vec<(f32, Color)> = Vec::new();
    for node in &snapshot.display_list.nodes {
        for op in &node.ops {
            if let PaintOp::Glyphs { run, color, .. } = op {
                runs.push((run.size, *color));
            }
        }
    }
    let largest = runs.iter().map(|r| r.0).fold(0.0, f32::max);
    let titles: Vec<Color> = runs
        .iter()
        .filter(|r| r.0 == largest)
        .map(|r| r.1)
        .collect();
    let lines: Vec<Color> = runs.iter().filter(|r| r.0 < largest).map(|r| r.1).collect();
    assert!(
        !titles.is_empty() && !lines.is_empty(),
        "titles and second lines: {runs:?}"
    );
    for line in &lines {
        assert_eq!(
            Some(line),
            titles.first(),
            "a second line reads as well as its title over a bright backdrop: {runs:?}"
        );
    }
}
