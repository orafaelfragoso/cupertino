//! The glass the dock's surfaces name: the bar the Dock's variant at the medium size, with Increase Contrast too, and
//! with Reduce Transparency its own body of another material; a label over a bar at the bottom and the stack's grid a
//! cartouche popover's, at the smallest and the medium size; a label beside a bar at a side the regular glass at the
//! smallest size; the application switcher its own variant at the medium size. None adapts to what is behind it.

use std::cell::Cell;
use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_dock::config::Orientation;
use cupertino_dock::stack::{StackEntry, StackPlacement};
use cupertino_dock::switcher_kit::{SwitcherEntry, SwitcherSurface};
use cupertino_dock::tiles::TileKind;
use cupertino_dock::view::bar::{self, BarView, TileView};
use cupertino_dock::view::label::{self, LabelView};
use cupertino_dock::view::stack::{self, StackView};
use cupertino_dock::view::{bar_key, Input};
use cupertino_kit::core::{
    Application, Command, GlassModifiers, GlassSize, GlassVariant, RuntimeOptions, Window,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{GlassStyle, Timestamp, WindowRequest};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};

/// The runtime turns between two looks at the windows.
const TURNS: u32 = 1000;
/// The bar's window and the bar in it.
const BAR_WINDOW: Size = Size::new(1440.0, 120.0);
const BAR: Rect = Rect::new(400.0, 50.0, 640.0, 64.0);
/// The bar's one tile.
const TILE: Rect = Rect::new(500.0, 50.0, 64.0, 64.0);

/// What the test shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Case {
    Bar,
    Label(Orientation),
    Stack,
    Switcher,
}

impl Case {
    /// How many windows the case shows: the bar and its popup, or the switcher alone.
    fn windows(self) -> usize {
        match self {
            Case::Bar | Case::Switcher => 1,
            Case::Label(_) | Case::Stack => 2,
        }
    }
}

thread_local! {
    /// The case the next driver's application shows.
    static CASE: Cell<Case> = const { Cell::new(Case::Bar) };
}

/// The bar's window with one tile, its bar at the bottom of the window.
fn bar_window(orientation: Orientation) -> Window<()> {
    bar::window(
        BarView {
            key: bar_key(1, 1),
            output: 1,
            size: BAR_WINDOW,
            orientation,
            tile_size: 64.0,
            bar: BAR,
            tiles: vec![TileView {
                id: 1,
                kind: TileKind::App,
                rect: TILE,
                rest: TILE,
                opacity: 1.0,
                indicator: 0.0,
                windowless: false,
                chevron: false,
                icon: Vec::new(),
                name: "Files".into(),
                description: String::new(),
                badge: None,
                picture: None,
            }],
            label_anchor: Some(Rect::new(500.0, 50.0, 64.0, 70.0)),
            fan_anchor: None,
            stack_anchor: Some(Rect::new(500.0, 50.0, 64.0, 70.0)),
            menu_anchor: None,
            input: vec![BAR],
            initial: None,
            ticking: false,
            surface_changes: 0,
        },
        |_: Input| (),
    )
}

/// The dock's surfaces of [`CASE`].
struct Dock;

impl Application for Dock {
    type Model = Case;
    type Message = ();

    fn init() -> (Case, Vec<Command<()>>) {
        (CASE.get(), Vec::new())
    }

    fn update(_: &mut Case, _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(case: &Case) -> Vec<Window<()>> {
        match *case {
            Case::Bar => vec![bar_window(Orientation::Bottom)],
            Case::Label(orientation) => vec![
                bar_window(orientation),
                label::window(
                    LabelView {
                        parent: bar_key(1, 1),
                        tile: 1,
                        name: "Files",
                        status: None,
                        orientation,
                        opacity: 1.0,
                    },
                    |_| (),
                ),
            ],
            Case::Stack => {
                let entries = [StackEntry {
                    name: "notes.txt".into(),
                    path: "/tmp/notes.txt".into(),
                    icon: "document".into(),
                }];
                let icons = |_: &str| Vec::new();
                vec![
                    bar_window(Orientation::Bottom),
                    stack::window(
                        StackView {
                            parent: bar_key(1, 1),
                            placement: StackPlacement {
                                window: cupertino_wm::geometry::Rect::new(
                                    300.0, -400.0, 400.0, 400.0,
                                ),
                                anchor: cupertino_wm::geometry::Rect::new(300.0, 50.0, 400.0, 70.0),
                                pointer: Some(200.0),
                            },
                            orientation: Orientation::Bottom,
                            name: "Downloads",
                            entries: &entries,
                            icons: &icons,
                            since: 10.0,
                            reduce_motion: true,
                        },
                        |_| (),
                    ),
                ]
            }
            Case::Switcher => {
                let mut switcher = SwitcherSurface::new();
                let entries = ["files", "settings"]
                    .map(|a| SwitcherEntry {
                        app_id: a.into(),
                        name: a.into(),
                    })
                    .to_vec();
                switcher
                    .changed(1, (1440.0, 900.0), entries, 1, 0.0)
                    .unwrap();
                switcher
                    .window(10.0, &|_| Vec::new(), |_| ())
                    .into_iter()
                    .collect()
            }
        }
    }
}

/// Options that draw on the CPU with the built-in font, in the neutral theme of `appearance`, with or without Reduce
/// Transparency.
fn options(appearance: Appearance, reduce_transparency: bool) -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(
            ThemeTokens::neutral(ThemePreferences {
                appearance,
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

/// A material region a window asked for: its material, where, its corners, and its glass's style and variant when it
/// is glass.
#[derive(Debug)]
struct Region {
    material: String,
    rect: Rect,
    radius: f64,
    style: Option<GlassStyle>,
    variant: Option<(GlassVariant, GlassModifiers)>,
}

/// The material regions the last window of `case` (its popup, where it has one) asked for the last time it asked, on a
/// desktop that shows materials.
fn regions(case: Case, appearance: Appearance, reduce_transparency: bool) -> Vec<Region> {
    CASE.set(case);
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver = TestDriver::<Dock>::new(
        capabilities,
        options(appearance, reduce_transparency),
        ManualClock::new(Timestamp(0)),
    );
    let turns = NonZeroU32::new(TURNS).unwrap();
    driver.run_until_idle(turns).unwrap();
    for (_, window) in driver.runtime().windows() {
        driver
            .window_system_mut()
            .configure(window, None, WindowStates::default())
            .unwrap();
        driver.run_until_idle(turns).unwrap();
    }
    let windows = driver.runtime().windows();
    assert_eq!(
        windows.len(),
        case.windows(),
        "{case:?}: every window shows"
    );
    let window = windows.last().expect("a window").1;
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
                    .map(|r| Region {
                        material: r.material.clone(),
                        rect: r.rect,
                        radius: r.radius,
                        style: r.glass.map(|g| g.style),
                        variant: r.glass.and_then(|g| g.variant),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// The variants and modifiers of the glass among `regions`.
fn variants(regions: &[Region]) -> Vec<(GlassVariant, GlassModifiers)> {
    regions.iter().filter_map(|r| r.variant).collect()
}

/// `variant` at `size`, with no other modifier.
fn named(variant: GlassVariant, size: GlassSize) -> (GlassVariant, GlassModifiers) {
    (variant, GlassModifiers::DEFAULT.sized(size))
}

#[test]
fn the_bar_is_the_docks_glass_at_the_medium_size() {
    let regions = regions(Case::Bar, Appearance::Light, false);
    assert_eq!(regions.len(), 1, "one region under the bar: {regions:?}");
    assert_eq!(regions[0].rect, BAR);
    assert_eq!(
        regions[0].style,
        Some(GlassStyle::Bar),
        "the bar glass where the window system has no variants"
    );
    assert_eq!(
        variants(&regions),
        [named(GlassVariant::Dock, GlassSize::Medium)]
    );
}

#[test]
fn with_increase_contrast_the_bar_keeps_the_docks_glass() {
    for appearance in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
        let regions = regions(Case::Bar, appearance, false);
        assert_eq!(
            variants(&regions),
            [named(GlassVariant::Dock, GlassSize::Medium)],
            "{appearance:?}"
        );
    }
}

#[test]
fn with_reduce_transparency_the_bar_is_its_own_body_and_no_glass() {
    let regions = regions(Case::Bar, Appearance::Dark, true);
    assert_eq!(regions.len(), 1, "one region under the bar: {regions:?}");
    assert_eq!(regions[0].style, None, "no glass: {regions:?}");
    assert_eq!(regions[0].material, "Dock");
}

#[test]
fn a_label_over_a_bar_at_the_bottom_is_a_small_cartouche() {
    let regions = regions(Case::Label(Orientation::Bottom), Appearance::Light, false);
    let small = named(GlassVariant::CartouchePopover, GlassSize::Minimum);
    // the capsule and the slices of its pointer, one shape
    let found = variants(&regions);
    assert!(found.len() > 1, "the capsule and its pointer: {regions:?}");
    assert!(found.iter().all(|v| *v == small), "{found:?}");
    assert_eq!(regions[0].radius, label::RADIUS);
}

#[test]
fn a_label_beside_a_bar_at_a_side_is_a_small_regular_capsule() {
    for side in [Orientation::Left, Orientation::Right] {
        let regions = regions(Case::Label(side), Appearance::Dark, false);
        assert_eq!(
            variants(&regions),
            [named(GlassVariant::Regular, GlassSize::Minimum)],
            "{side:?}: {regions:?}"
        );
        assert_eq!(regions[0].radius, label::RADIUS);
    }
}

#[test]
fn the_stack_is_a_cartouche_at_the_medium_size() {
    let regions = regions(Case::Stack, Appearance::Light, false);
    let medium = named(GlassVariant::CartouchePopover, GlassSize::Medium);
    let found = variants(&regions);
    assert!(!found.is_empty(), "glass: {regions:?}");
    assert!(found.iter().all(|v| *v == medium), "{found:?}");
}

#[test]
fn the_switcher_is_its_own_glass_at_the_medium_size() {
    let regions = regions(Case::Switcher, Appearance::Dark, false);
    assert_eq!(
        variants(&regions),
        [named(GlassVariant::AppSwitcher, GlassSize::Medium)],
        "{regions:?}"
    );
}

#[test]
fn with_reduce_transparency_the_switcher_shows_no_glass() {
    let regions = regions(Case::Switcher, Appearance::Light, true);
    assert!(variants(&regions).is_empty(), "{regions:?}");
}
