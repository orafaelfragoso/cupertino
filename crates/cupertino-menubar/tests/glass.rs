//! The glass a bar's popups name: a menu and the Wi-Fi join prompt lie on the menu variant over their whole window with
//! their corners, at the menu's own size and without the variant's drop shadow, as the popup draws the menu's window
//! shadow apart; the bar's highlight under the open menu's title is clear glass tinted with the highlight colour, and
//! is painted under Reduce Transparency and on a desktop that does not tint glass.

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
use cupertino_kit::render::{CpuConfig, CpuRenderer};
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
    tint: Option<[f32; 4]>,
}

/// The windows of the bar's scene (`menu` or the prompt) in the `appearance` with or without Reduce Transparency, and
/// the material regions each asked for the last time it asked, on a desktop that shows materials and, with `variants`,
/// takes glass named by its variant and tinted.
fn regions(
    menu: bool,
    appearance: Appearance,
    reduce_transparency: bool,
    variants: bool,
) -> Vec<(WindowKey, Size, Vec<Region>)> {
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
                                tint: r.glass.map(|g| g.tint),
                            })
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            (key, size, regions)
        })
        .collect()
}

/// The menu variant at the menu's own size, its shadow left out.
const MENU: (GlassVariant, GlassModifiers) = (
    GlassVariant::Menu,
    GlassModifiers {
        exclude_shadow: true,
        ..GlassModifiers::DEFAULT
    },
);

/// Asserts that the popup of `size` lies on one region of the menu's glass over its whole window with corners of
/// `radius`.
fn lies_on_menu_glass(size: Size, regions: &[Region], radius: f64) {
    assert_eq!(regions.len(), 1, "one region: {regions:?}");
    let region = &regions[0];
    assert_eq!(region.rect, Rect::new(0.0, 0.0, size.width, size.height));
    assert_eq!(region.radius, radius);
    assert_eq!(
        region.style,
        Some(GlassStyle::Menu),
        "the menu glass where the window system has no variants"
    );
    assert_eq!(region.variant, Some(MENU));
}

#[test]
fn a_menu_lies_on_the_menu_variant_without_its_shadow() {
    for (appearance, reduce_transparency) in [
        (Appearance::Light, false),
        (Appearance::Dark, false),
        (Appearance::DarkHighContrast, false),
        (Appearance::Light, true),
    ] {
        let windows = regions(true, appearance, reduce_transparency, true);
        assert_eq!(windows.len(), 2, "the bar and its menu: {windows:?}");
        let (_, _, bar) = &windows[0];
        if reduce_transparency {
            assert!(bar.is_empty(), "the bar paints its highlight: {bar:?}");
        } else {
            lies_on_highlight_glass(bar, appearance);
        }
        let (_, size, menu) = &windows[1];
        lies_on_menu_glass(*size, menu, view::POPUP_RADIUS);
    }
}

#[test]
fn a_desktop_that_does_not_tint_glass_has_the_bar_paint_its_highlight() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let windows = regions(true, appearance, false, false);
        assert_eq!(windows.len(), 2, "the bar and its menu: {windows:?}");
        let (_, _, bar) = &windows[0];
        assert!(
            bar.is_empty(),
            "untinted clear glass would show no highlight, so the bar paints it: {bar:?}"
        );
        let (_, size, menu) = &windows[1];
        lies_on_menu_glass(*size, menu, view::POPUP_RADIUS);
    }
}

#[test]
fn the_join_prompt_lies_on_the_menu_variant_with_the_panels_corners() {
    let windows = regions(false, Appearance::Dark, false, true);
    assert_eq!(windows.len(), 2, "the bar and the prompt: {windows:?}");
    let (_, size, prompt) = &windows[1];
    lies_on_menu_glass(*size, prompt, panel::RADIUS);
}

/// Asserts that the bar's one region is its highlight under the system menu's title: clear glass where the bar painted
/// its highlight, tinted with the highlight colour of the `appearance`.
fn lies_on_highlight_glass(bar: &[Region], appearance: Appearance) {
    let text = text();
    let scene = scene(&text, true);
    let metrics = &scene.controller.bar_metrics;
    let slot = scene
        .controller
        .layout()
        .slot(&SlotKey::System)
        .expect("a system menu")
        .rect;
    let (x, width) = metrics.highlight_span(&SlotKey::System, slot.w);
    let height = slot.h - 2.0 * metrics.selection_inset_y;
    let dark = matches!(appearance, Appearance::Dark | Appearance::DarkHighContrast);
    let palette = BarTone::default().palette(dark, appearance.is_high_contrast());
    assert_eq!(bar.len(), 1, "the highlight alone: {bar:?}");
    let region = &bar[0];
    assert_eq!(
        region.rect,
        Rect::new(
            slot.x + x,
            slot.y + metrics.selection_inset_y,
            width,
            height
        )
    );
    assert_eq!(
        region.radius,
        metrics.selection_corner_radius.min(height / 2.0)
    );
    assert_eq!(region.style, Some(GlassStyle::Clear));
    assert_eq!(
        region.variant,
        Some((GlassVariant::Clear, GlassModifiers::DEFAULT))
    );
    assert_eq!(region.tint, Some(palette.bar_highlight));
}
