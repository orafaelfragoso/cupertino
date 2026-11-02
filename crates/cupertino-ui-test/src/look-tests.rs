use super::*;
use crate::ManualClock;
use cupertino_geometry::Size;
use cupertino_ui_core::{
    Change, Command, CommandRegistry, Element, Event, EventCx, MeasureCx, PaintCx, Propagation,
    RuntimeOptions, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};
use cupertino_ui_platform::{Capabilities, DecorationMode, Timestamp, WindowSpec, WindowStates};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_text::{FontIndex, GlyphKey, GlyphRasterizer, RasterGlyph, TextError, TextSystem};
use cupertino_ui_theme::ThemePreferences;

struct Plain;

impl Application for Plain {
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
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Plain".into(),
                app_id: "test.plain".into(),
                size: Size::new(40.0, 30.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Client,
                foreign_parent: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Nothing),
            menus: None,
        }]
    }
}

struct Nothing;

impl Widget<()> for Nothing {
    type State = ();
    fn state(&self) {}
    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }
    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }
    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(40.0, 30.0),
            baseline: None,
        }
    }
    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, ()>) -> Propagation {
        Propagation::Continue
    }
    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}
    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

struct NoGlyphs;
impl GlyphRasterizer for NoGlyphs {
    fn rasterize(&mut self, _: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        panic!("the plain window paints no glyphs")
    }
}

fn start() -> (TestDriver<Plain>, WindowId) {
    let data = Theme::from_dir(THEME_DATA).unwrap();
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let options = RuntimeOptions {
        theme: Arc::new(ThemeTokens::cupertino(&data, preferences).unwrap()),
        text: Arc::new(TextSystem::new(FontIndex::new(), "none")),
        icons: Arc::new(cupertino_ui_core::PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 8,
        renderers: Box::new(|| {
            Box::new(CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 16,
                    glyph_cache_bytes: 0,
                    image_cache_bytes: 0,
                    font_smoothing: true,
                },
                NoGlyphs,
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
    driver.run_until_idle(NonZeroU32::new(16).unwrap()).unwrap();
    let window = driver.runtime().windows()[0].1;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    driver
        .window_system_mut()
        .configure(window, None, active)
        .unwrap();
    driver.present(window).unwrap();
    (driver, window)
}

fn read(path: &Path) -> tiny_skia::Pixmap {
    tiny_skia::Pixmap::load_png(path).unwrap()
}

#[test]
fn each_appearance_is_written_at_the_scale_and_the_window_gets_its_theme_and_scale_back() {
    let (mut driver, window) = start();
    let before = Arc::clone(driver.runtime().theme());
    let dir = std::env::temp_dir().join(format!("cupertino-look-{}", std::process::id()));

    let written = driver
        .look_into(
            &dir,
            window,
            "plain",
            2.0,
            &[Appearance::Light, Appearance::Dark],
        )
        .unwrap();

    assert_eq!(
        written,
        vec![
            dir.join("plain-light@2x.png"),
            dir.join("plain-dark@2x.png")
        ]
    );
    let (light, dark) = (read(&written[0]), read(&written[1]));
    assert_eq!(
        (light.width(), light.height()),
        (80, 60),
        "40 × 30 pt at 2x"
    );
    let (l, d) = (light.pixel(40, 30).unwrap(), dark.pixel(40, 30).unwrap());
    assert!(
        l.red() > 200 && d.red() < 100,
        "the light window background is light and the dark one dark: {l:?} {d:?}"
    );
    assert_eq!(driver.runtime().theme(), &before);
    let last = driver
        .window_system_mut()
        .frames(window)
        .unwrap()
        .last()
        .unwrap();
    assert_eq!((last.width, last.height), (40, 30), "back at scale 1");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_unknown_appearance_or_scale_names_its_setting() {
    assert!(matches!(
        parse_appearances("light,sepia"),
        Err(LookError::Setting {
            name: LOOK_APPEARANCES,
            ..
        })
    ));
    assert_eq!(
        parse_appearances("dark, light-hc").unwrap(),
        vec![Appearance::Dark, Appearance::LightHighContrast]
    );
    assert!(matches!(
        parse_scale("0"),
        Err(LookError::Setting {
            name: LOOK_SCALE,
            ..
        })
    ));
    assert_eq!(parse_scale("1.5").unwrap(), 1.5);
}
