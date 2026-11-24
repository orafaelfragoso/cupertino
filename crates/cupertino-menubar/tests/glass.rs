//! The glass a bar's popups name: a menu and the Wi-Fi join prompt lie on the menu variant over their whole window with
//! their corners, at the menu's own size and without the variant's drop shadow, as the popup draws the menu's window
//! shadow apart; the bar's highlight under the open menu's title is no glass but the highlight colour painted, a flat
//! veil, on every desktop and with Reduce Transparency.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, GlassModifiers, GlassVariant, RuntimeOptions, Window, WindowKey,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{GlassStyle, Timestamp, WindowRequest};
use cupertino_kit::render::{CpuConfig, CpuRenderer, PixelFormat};
use cupertino_kit::test::{
    Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver, WindowStates,
};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_menubar::controller::MenuController;
use cupertino_menubar::extras::Passphrase;
use cupertino_menubar::feed::{BarFeed, Shared};
use cupertino_menubar::geom;
use cupertino_menubar::layout::{panel, BarMetrics, SlotKey};
use cupertino_menubar::tone::{BarTone, SlotTones};
use cupertino_menubar::view::{
    self, BarFont, BarId, BarScene, KitMeasure, PromptPhase, PromptScene, RowIcons,
};

/// The runtime turns between two looks at the windows.
const TURNS: u32 = 1000;
/// The output the bar is on.
const OUTPUT: Size = Size::new(1440.0, 900.0);
/// When the click that opens the system menu presses and releases, in seconds on the controller's clock.
const PRESS_AT: f64 = 1.0;
const RELEASE_AT: f64 = 1.05;

/// What the bar shows: its system menu open, or the join prompt.
struct Scene {
    controller: MenuController,
    shared: Shared,
    font: BarFont,
    icons: Rc<RowIcons>,
    prompt: Option<(geom::Point, Passphrase)>,
}

thread_local! {
    /// The scene the next driver's application shows.
    static NEXT: RefCell<Option<Scene>> = const { RefCell::new(None) };
}

/// A bar of [`NEXT`].
struct Bar;

impl Application for Bar {
    type Model = Scene;
    type Message = ();

    fn init() -> (Scene, Vec<Command<()>>) {
        (NEXT.take().expect("a scene"), Vec::new())
    }

    fn update(_: &mut Scene, _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(scene: &Scene) -> Vec<Window<()>> {
        view::windows(
            &BarScene {
                bar: BarId(0),
                controller: &scene.controller,
                shared: &scene.shared,
                font: scene.font,
                tone: BarTone::default(),
                slot_tones: &SlotTones::default(),
                solid: false,
                size: Size::new(OUTPUT.width, scene.controller.bar_metrics.height),
                open_panels: &[],
                icons: &scene.icons,
                prompt: scene.prompt.as_ref().map(|(at, text)| PromptScene {
                    ssid: "Harbour",
                    at: *at,
                    show: false,
                    text,
                    phase: PromptPhase::Editing,
                }),
            },
            |_| (),
        )
    }
}

/// The text system of the built-in font.
fn text() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter Variable"))
}

/// The bar's scene with its system menu open (`menu`) or the join prompt open.
fn scene(text: &Arc<TextSystem>, menu: bool) -> Scene {
    let design = cupertino_kit_shell::design::menu_bar();
    let font = BarFont::of(&design);
    let measure = KitMeasure {
        text: Arc::clone(text),
        font,
    };
    let output = geom::Rect::new(0.0, 0.0, OUTPUT.width, OUTPUT.height);
    let mut controller = MenuController::new(output, Box::new(measure));
    controller.bar_metrics = BarMetrics {
        height: design.height,
        selection_corner_radius: design.selection_radius,
        selection_inset_y: BarMetrics::item_inset_y(design.height),
        ..BarMetrics::default()
    };
    let shared = Shared::default();
    controller.set_content(BarFeed::new(0, false, false).content(&shared));
    let mut prompt = None;
    if menu {
        let slot = controller
            .layout()
            .slot(&SlotKey::System)
            .expect("a system menu")
            .rect;
        let at = geom::Point::new(slot.x + slot.w / 2.0, slot.y + slot.h / 2.0);
        controller.pointer_press(at, PRESS_AT);
        controller.pointer_release(at, RELEASE_AT);
        assert_eq!(
            controller.open_slot(),
            Some(&SlotKey::System),
            "the menu opens"
        );
    } else {
        let at = geom::Point::new(
            OUTPUT.width - panel::WIDTH - 8.0,
            design.height + panel::GAP,
        );
        prompt = Some((at, Passphrase::new("")));
    }
    Scene {
        controller,
        shared,
        font,
        icons: Rc::new(RowIcons::default()),
        prompt,
    }
}

/// A material region a window asked for: where, its corners, and its glass's style and variant when it is glass.
#[derive(Debug)]
struct Region {
    rect: Rect,
    radius: f64,
    style: Option<GlassStyle>,
    variant: Option<(GlassVariant, GlassModifiers)>,
}

/// One window as it was last drawn: its size, the material regions it asked for the last time it asked, and its last
/// frame's premultiplied pixels, `width` pixels a row.
#[derive(Debug)]
struct Drawn {
    size: Size,
    regions: Vec<Region>,
    width: u32,
    pixels: Vec<u8>,
}

impl Drawn {
    /// The alpha of the pixel at `at`, in the window's points.
    fn alpha(&self, at: (f64, f64)) -> u8 {
        let scale = f64::from(self.width) / self.size.width;
        let (x, y) = ((at.0 * scale) as u32, (at.1 * scale) as u32);
        self.pixels[(y * self.width + x) as usize * 4 + 3]
    }
}

/// The windows of the bar's scene (`menu` or the prompt) in the `appearance` with or without Reduce Transparency, as
/// last drawn on a desktop that shows materials and, with `variants`, takes glass named by its variant and tinted.
fn regions(
    menu: bool,
    appearance: Appearance,
    reduce_transparency: bool,
    variants: bool,
) -> Vec<(WindowKey, Drawn)> {
    let text = text();
    NEXT.set(Some(scene(&text, menu)));
    let glyphs = Arc::clone(&text);
    let options = RuntimeOptions {
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
    };
    let capabilities = Capabilities {
        app_effects: true,
        glass_variants: variants,
        glass_tint: variants,
        glass_scale: variants,
        ..Capabilities::default()
    };
    let mut driver = TestDriver::<Bar>::new(capabilities, options, ManualClock::new(Timestamp(0)));
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
    let system = driver.window_system_mut();
    windows
        .into_iter()
        .map(|(key, window)| {
            let size = system.spec(window).unwrap().size;
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
                                rect: r.rect,
                                radius: r.radius,
                                style: r.glass.map(|g| g.style),
                                variant: r.glass.and_then(|g| g.variant),
                            })
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            let frame = system.frames(window).unwrap().last().expect("a frame");
            assert_eq!(frame.format, PixelFormat::Rgba8Premultiplied);
            let drawn = Drawn {
                size,
                regions,
                width: frame.width,
                pixels: frame.pixels.clone(),
            };
            (key, drawn)
        })
        .collect()
}

/// The menu variant at the menu's own size, its shadow left out; an application's menu adapts to what lies behind it,
/// a status panel's and the join prompt's do not.
const MENU: (GlassVariant, GlassModifiers) = (
    GlassVariant::Menu,
    GlassModifiers {
        exclude_shadow: true,
        ..GlassModifiers::DEFAULT
    },
);

/// Asserts that the popup of `size` lies on one region of the menu's glass over its whole window with corners of
/// `radius`, adapting to what lies behind it or not.
fn lies_on_menu_glass(size: Size, regions: &[Region], radius: f64, adaptive: bool) {
    assert_eq!(regions.len(), 1, "one region: {regions:?}");
    let region = &regions[0];
    assert_eq!(region.rect, Rect::new(0.0, 0.0, size.width, size.height));
    assert_eq!(region.radius, radius);
    assert_eq!(
        region.style,
        Some(GlassStyle::Menu),
        "the menu glass where the window system has no variants"
    );
    let (variant, modifiers) = MENU;
    assert_eq!(
        region.variant,
        Some((
            variant,
            GlassModifiers {
                adaptive,
                ..modifiers
            }
        ))
    );
}

#[test]
fn a_menu_lies_on_the_menu_variant_without_its_shadow_and_the_bar_paints_its_highlight() {
    for (appearance, reduce_transparency, variants) in [
        (Appearance::Light, false, true),
        (Appearance::Dark, false, true),
        (Appearance::DarkHighContrast, false, true),
        (Appearance::Light, true, true),
        (Appearance::Light, false, false),
        (Appearance::Dark, false, false),
    ] {
        let windows = regions(true, appearance, reduce_transparency, variants);
        assert_eq!(windows.len(), 2, "the bar and its menu: {windows:?}");
        paints_its_highlight(&windows[0].1, appearance);
        let menu = &windows[1].1;
        lies_on_menu_glass(menu.size, &menu.regions, view::POPUP_RADIUS, true);
    }
}

#[test]
fn the_join_prompt_lies_on_the_menu_variant_with_the_panels_corners() {
    let windows = regions(false, Appearance::Dark, false, true);
    assert_eq!(windows.len(), 2, "the bar and the prompt: {windows:?}");
    let prompt = &windows[1].1;
    lies_on_menu_glass(prompt.size, &prompt.regions, panel::RADIUS, false);
}

/// Asserts that the bar asks for no glass and paints its highlight under the system menu's title: the highlight colour
/// of the `appearance` inside the capsule's end, nothing above the capsule.
fn paints_its_highlight(bar: &Drawn, appearance: Appearance) {
    assert!(
        bar.regions.is_empty(),
        "no glass under the highlight: {bar:?}"
    );
    let text = text();
    let scene = scene(&text, true);
    let metrics = &scene.controller.bar_metrics;
    let slot = scene
        .controller
        .layout()
        .slot(&SlotKey::System)
        .expect("a system menu")
        .rect;
    let (x, _) = metrics.highlight_span(&SlotKey::System, slot.w);
    let (left, top) = (slot.x + x, slot.y + metrics.selection_inset_y);
    let dark = matches!(appearance, Appearance::Dark | Appearance::DarkHighContrast);
    let palette = BarTone::default().palette(dark, appearance.is_high_contrast());
    let veil = (palette.bar_highlight[3] * 255.0).round();
    let inside = bar.alpha((left + 2.0, slot.y + slot.h / 2.0));
    assert!(
        (f32::from(inside) - veil).abs() <= 1.0,
        "the highlight colour inside the capsule's end: alpha {inside} for {veil}"
    );
    assert_eq!(bar.alpha((left + 2.0, top - 2.0)), 0, "nothing above it");
}
