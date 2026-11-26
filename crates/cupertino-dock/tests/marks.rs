//! The marks under the bar's tiles: the running indicator and the separator's line, each a material the compositor
//! lays over the bar (plus-darker in the light appearance, plus-lighter in the dark), placed as the design has them,
//! and painted by the dock itself in high contrast.

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_dock::config::Orientation;
use cupertino_dock::tiles::TileKind;
use cupertino_dock::view::bar::{self, BarView, TileView};
use cupertino_dock::view::{bar_key, Input};
use cupertino_kit::core::{Application, Command, RuntimeOptions, Window};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{Timestamp, WindowRequest};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};

/// The runtime turns between two looks at the window.
const TURNS: u32 = 1000;
/// The bar's window, the bar in it, and its tiles at 64 pt: an application, a separator's 26 pt slot and another
/// application, with the 4 pt gaps between them.
const WINDOW: Size = Size::new(1440.0, 140.0);
const BAR: Rect = Rect::new(400.0, 40.0, 150.0, 84.0);
const FIRST: Rect = Rect::new(410.0, 50.0, 64.0, 64.0);
const SLOT: Rect = Rect::new(478.0, 50.0, 26.0, 64.0);
const SECOND: Rect = Rect::new(508.0, 50.0, 64.0, 64.0);

fn tile(id: u64, kind: TileKind, rest: Rect, indicator: f64) -> TileView {
    TileView {
        id,
        kind,
        rect: rest,
        rest,
        opacity: 1.0,
        indicator,
        windowless: false,
        chevron: false,
        icon: Vec::new(),
        name: String::new(),
        description: String::new(),
        badge: None,
        picture: None,
    }
}

struct Dock;

impl Application for Dock {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        vec![bar::window(
            BarView {
                key: bar_key(1, 1),
                output: 1,
                size: WINDOW,
                orientation: Orientation::Bottom,
                tile_size: 64.0,
                bar: BAR,
                tiles: vec![
                    tile(1, TileKind::App, FIRST, 1.0),
                    tile(2, TileKind::Separator, SLOT, 0.0),
                    tile(3, TileKind::App, SECOND, 0.0),
                ],
                label_anchor: None,
                fan_anchor: None,
                stack_anchor: None,
                menu_anchor: None,
                input: vec![BAR],
                initial: None,
                ticking: false,
                surface_changes: 0,
            },
            |_: Input| (),
        )]
    }
}

fn options(appearance: Appearance) -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance,
                reduce_transparency: false,
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

/// The materials (name, rectangle, corner radius) the bar's window asked for the last time it asked.
fn materials(appearance: Appearance) -> Vec<(String, Rect, f64)> {
    let mut driver = TestDriver::<Dock>::new(
        Capabilities {
            app_effects: true,
            ..Capabilities::default()
        },
        options(appearance),
        ManualClock::new(Timestamp(0)),
    );
    let turns = NonZeroU32::new(TURNS).unwrap();
    driver.run_until_idle(turns).unwrap();
    let window = driver.runtime().windows()[0].1;
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
                    .map(|r| (r.material.clone(), r.rect, r.radius))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn named<'a>(all: &'a [(String, Rect, f64)], name: &str) -> &'a (String, Rect, f64) {
    all.iter()
        .find(|(n, ..)| n == name)
        .unwrap_or_else(|| panic!("no {name} among {all:?}"))
}

#[test]
fn the_separator_stands_17_pt_after_one_tile_and_16_before_the_next_inset_11_pt() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let all = materials(appearance);
        let (_, line, radius) = named(&all, "DockSeparator");
        assert_eq!(line.width(), 1.0, "{appearance:?}");
        assert_eq!(
            line.x() - (FIRST.x() + FIRST.width()),
            17.0,
            "{appearance:?}"
        );
        assert_eq!(
            SECOND.x() - (line.x() + line.width()),
            16.0,
            "{appearance:?}"
        );
        assert_eq!(line.y() - BAR.y(), 11.0, "{appearance:?}");
        assert_eq!(BAR.y() + BAR.height() - (line.y() + line.height()), 11.0);
        assert_eq!(line.height(), 62.0, "{appearance:?}");
        assert_eq!(*radius, 0.5);
    }
}

#[test]
fn the_running_indicator_is_a_4_pt_dot_centred_under_its_tile_6_pt_above_the_bars_edge() {
    let all = materials(Appearance::Light);
    let (_, dot, radius) = named(&all, "RunningIndicator");
    assert_eq!((dot.width(), dot.height(), *radius), (4.0, 4.0, 2.0));
    assert_eq!(dot.x() + 2.0, FIRST.x() + 32.0);
    assert_eq!(BAR.y() + BAR.height() - (dot.y() + 2.0), 6.0);
}

#[test]
fn in_high_contrast_the_marks_paint_themselves() {
    for appearance in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
        let all = materials(appearance);
        assert!(
            all.iter()
                .all(|(n, ..)| n != "DockSeparator" && n != "RunningIndicator"),
            "{appearance:?}: {all:?}"
        );
    }
}
