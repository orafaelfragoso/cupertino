//! The About window on CupertinoKit, run headless: what it reads to assistive technology and what it draws.

#![cfg(feature = "app")]

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::controls::{SIDEBAR_TOP, WINDOW_CORNER_RADIUS};
use cupertino_kit::core::WindowBackground;
use cupertino_kit::core::{AccessibilityUpdate, PlaceholderIcons, Role, RuntimeOptions};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, DecorationMode, Timestamp, WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{
    Appearance, ColorRole, StateVariant, Surface, ThemePreferences, ThemeTokens,
};
use cupertino_settings::about::{display_value, metrics, About, Display, Row};
use cupertino_settings::about_app::{self, launch, layout, AboutApp, Start};
use cupertino_theme::Theme;
use cupertino_ui_test::{semantic_gaps, ManualClock, TestDriver};

/// The start is the process's, so the tests take turns.
static WINDOW: Mutex<()> = Mutex::new(());

/// Kit names no window root.
const KIT_GAPS: [&str; 1] = ["Window #0: no name"];

fn about() -> About {
    let display = display_value(&Display {
        make: Some("Example"),
        model: Some("Panel"),
        mode: (2560, 1440),
        physical_size_mm: Some((597, 336)),
        scale: 2.0,
        ..Display::default()
    })
    .unwrap();
    let row = |label, value: &str| Row {
        label,
        value: value.into(),
    };
    About {
        version: "Version 0.1.0".into(),
        os: None,
        rows: vec![
            row("Processor", "Example CPU (8 threads)"),
            row("Graphics", "Card A"),
            row("Graphics", "Card B"),
            row("Displays", &display),
            row("Kernel", "6.17.0"),
        ],
    }
}

struct Run {
    driver: TestDriver<AboutApp>,
    window: WindowId,
    theme: Arc<ThemeTokens>,
}

fn start(appearance: Appearance, scale: f64) -> Run {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let theme =
        Arc::new(ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap());
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let about = about();
    let (_, shown) = async_channel::bounded(1);
    launch(Start {
        layout: layout(&about, &text, &theme),
        about,
        activation: String::new(),
        shown,
    });
    let glyphs = Arc::clone(&text);
    let options = RuntimeOptions {
        theme: Arc::clone(&theme),
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
    };
    let mut driver = TestDriver::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    );
    let budget = NonZeroU32::new(64).unwrap();
    driver.run_until_idle(budget).unwrap();
    let window = driver.runtime().windows()[0].1;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    let system = driver.window_system_mut();
    system.configure(window, None, active).unwrap();
    system
        .enqueue_window(window, WindowEvent::Scale(scale))
        .unwrap();
    for _ in 0..8 {
        driver.run_until_idle(budget).unwrap();
        let system = driver.window_system_mut();
        if !system.frame_requested(window).unwrap() {
            break;
        }
        system.frame_ready(window, Timestamp(0)).unwrap();
    }
    Run {
        driver,
        window,
        theme,
    }
}

#[test]
fn a_screen_reader_reads_every_field_in_order_and_the_more_info_button() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = start(Appearance::Light, 1.0);
    run.driver.look(run.window, "about").unwrap();
    let spec = run
        .driver
        .window_system_mut()
        .spec(run.window)
        .unwrap()
        .clone();
    assert_eq!(
        (spec.title.as_str(), spec.app_id.as_str()),
        ("About This Computer", "cupertino-settings")
    );
    assert_eq!(
        (spec.min_size, spec.max_size),
        (Some(spec.size), Some(spec.size)),
        "a fixed size"
    );
    assert_eq!(spec.size.width, f64::from(metrics::WIDTH));

    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let named = |role| -> Vec<String> {
        snapshot
            .accessibility
            .iter()
            .filter(|(_, node)| node.role == role)
            .filter_map(|(_, node)| node.name.clone())
            .collect()
    };
    let display = about().rows[3].value.clone();
    assert_eq!(
        named(Role::StaticText),
        [
            "Cupertino",
            "Version 0.1.0",
            "Processor",
            "Example CPU (8 threads)",
            "Graphics",
            "Card A",
            "Card B",
            "Displays",
            &display,
            "Kernel",
            "6.17.0",
        ],
        "a repeated label is read once"
    );
    assert_eq!(named(Role::Button), ["More Info…"]);

    let bounds: Vec<(String, cupertino_kit::geometry::Rect)> = snapshot
        .accessibility
        .iter()
        .filter(|(_, node)| matches!(node.role, Role::StaticText | Role::Button))
        .map(|(_, node)| (node.name.clone().unwrap(), node.bounds))
        .collect();
    let centre = f64::from(metrics::WIDTH) / 2.0;
    for (name, rect) in &bounds {
        assert!(
            rect.x() >= 0.0 && rect.max_x() <= spec.size.width && rect.max_y() <= spec.size.height,
            "{name} lies in the window: {rect:?}"
        );
    }
    for label in ["Processor", "Graphics", "Displays", "Kernel"] {
        let (_, rect) = bounds.iter().find(|(name, _)| name == label).unwrap();
        assert_eq!(
            rect.max_x(),
            centre - metrics::LABEL_END,
            "{label} ends just before the centre"
        );
    }
    for value in ["Example CPU (8 threads)", "Card A", "6.17.0"] {
        let (_, rect) = bounds.iter().find(|(name, _)| name == value).unwrap();
        assert_eq!(
            rect.x(),
            centre + metrics::VALUE_START,
            "{value} starts just after the centre"
        );
    }
    let (_, button) = bounds
        .iter()
        .find(|(name, _)| name == "More Info…")
        .unwrap();
    assert_eq!(
        (button.width(), button.height()),
        (metrics::BUTTON_WIDTH, metrics::BUTTON_HEIGHT)
    );
    assert_eq!(
        button.x(),
        centre - metrics::BUTTON_WIDTH / 2.0,
        "the button is centred"
    );
    for pair in bounds[3..].windows(2).filter(|p| p[0].1.x() > centre) {
        let ((above, a), (below, b)) = (&pair[0], &pair[1]);
        assert!(a.max_y() <= b.y(), "{above:?} ends before {below:?} starts");
    }
    let tree = AccessibilityUpdate {
        window: run.window,
        root: snapshot.accessibility[0].0,
        focus: snapshot.focus,
        nodes: snapshot.accessibility.clone(),
        removed: Vec::new(),
        full: true,
    };
    let mut gaps = semantic_gaps(&tree);
    gaps.retain(|gap| !KIT_GAPS.contains(&gap.as_str()));
    assert_eq!(gaps, Vec::<String>::new());
}

#[test]
fn the_mark_takes_the_label_colour_of_each_appearance_on_the_window_background() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    for appearance in [Appearance::Light, Appearance::Dark] {
        let mut run = start(appearance, 2.0);
        let color = |role| {
            let c = run.theme.colors.get(role, StateVariant::NORMAL).unwrap();
            <[u8; 3]>::try_from(&c.to_rgba8()[..3]).unwrap()
        };
        let (background, label) = (color(ColorRole::WindowBackground), color(ColorRole::Text));
        let spec_size: Size = run
            .driver
            .window_system_mut()
            .spec(run.window)
            .unwrap()
            .size;
        let frame = run
            .driver
            .window_system_mut()
            .frames(run.window)
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            (frame.width, frame.height),
            (
                (spec_size.width * 2.0) as u32,
                (spec_size.height * 2.0) as u32
            ),
            "{appearance:?}: drawn at 2x"
        );
        let pixel = |x: f64, y: f64| {
            let at = ((y * 2.0) as usize * frame.width as usize + (x * 2.0) as usize) * 4;
            [frame.pixels[at], frame.pixels[at + 1], frame.pixels[at + 2]]
        };
        assert_eq!(
            pixel(f64::from(metrics::WIDTH) / 2.0, 2.0),
            background,
            "{appearance:?}: the top edge"
        );
        let left = (f64::from(metrics::WIDTH) - metrics::MARK) / 2.0;
        let side = (metrics::MARK * 2.0) as u32;
        let ink = run
            .theme
            .colors
            .get(ColorRole::Text, StateVariant::NORMAL)
            .unwrap();
        let reference = cupertino_icons::system_mark(side, [ink.r, ink.g, ink.b, ink.a]);
        let mut inked = 0;
        for y in 0..side {
            for x in 0..side {
                let source = reference.pixel(x, y).unwrap();
                let alpha = u16::from(source.alpha());
                let over = |s: u8, b: u8| {
                    (u16::from(s) + (u16::from(b) * (255 - alpha) + 127) / 255) as u8
                };
                let expected = [
                    over(source.red(), background[0]),
                    over(source.green(), background[1]),
                    over(source.blue(), background[2]),
                ];
                let drawn = pixel(left + f64::from(x) / 2.0, metrics::TOP + f64::from(y) / 2.0);
                assert!(
                    (0..3).all(|i| drawn[i].abs_diff(expected[i]) <= 2),
                    "{appearance:?}: the mark at ({x}, {y}) is {drawn:?}, not {expected:?}"
                );
                inked += usize::from(drawn == label);
            }
        }
        assert!(inked > 0, "{appearance:?}: the mark is in the label colour");
        assert_eq!(
            pixel(left - 4.0, metrics::TOP + metrics::MARK / 2.0),
            background,
            "{appearance:?}: beside the mark"
        );
    }
}

#[test]
fn the_window_is_a_glass_panel_whose_title_bar_is_only_its_buttons() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = start(Appearance::Dark, 1.0);
    let spec = run
        .driver
        .window_system_mut()
        .spec(run.window)
        .unwrap()
        .clone();
    assert_eq!(spec.decorations, DecorationMode::Unified);
    assert_eq!(about_app::DECORATIONS, DecorationMode::Unified);
    assert_eq!(
        about_app::BACKGROUND,
        WindowBackground::Surface {
            surface: Surface::Sidebar,
            radius: WINDOW_CORNER_RADIUS,
            alpha: 1.0,
        },
        "the sidebar's glass under the whole window, with the window's corners"
    );
    assert_eq!(
        metrics::TOP,
        SIDEBAR_TOP,
        "the mark starts under the row of the window's buttons"
    );
}
