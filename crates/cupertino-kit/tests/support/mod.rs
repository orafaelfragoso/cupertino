//! A Kit application on the headless window system, run as its UI thread would run it: tasks finish on other threads
//! and wake it.

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{Application, Environment, RuntimeOptions};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::Timestamp;
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::test::{Capabilities, FontIndex, ManualClock, PlaceholderIcons, TestDriver};
use cupertino_kit::text::{GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a test waits for work on other threads before it fails.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// The built-in font, shared by whoever is given it.
pub fn text_system() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter Variable"))
}

/// Options that draw on the CPU with `text`, in the neutral light theme.
pub fn options(text: Arc<TextSystem>) -> RuntimeOptions {
    let theme = ThemeTokens::neutral(ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: true,
    })
    .unwrap();
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(theme),
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

/// A driver drawing with `environment`'s text system and icons, in the neutral light theme.
pub fn driver_in<A: Application>(environment: Environment) -> TestDriver<A> {
    let mut options = options(environment.text);
    options.icons = environment.icons;
    TestDriver::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    )
}

pub fn driver<A: Application>(text: Arc<TextSystem>) -> TestDriver<A> {
    TestDriver::new(
        Capabilities::default(),
        options(text),
        ManualClock::new(Timestamp(0)),
    )
}

/// Runs `driver` until `done` says so of its model, waiting for its tasks' wakes in between.
pub fn run_until<A: Application>(driver: &mut TestDriver<A>, done: impl Fn(&A::Model) -> bool) {
    let end = Instant::now() + PATIENCE;
    loop {
        driver
            .run_until_idle(NonZeroU32::new(1000).unwrap())
            .unwrap();
        if done(driver.runtime().model()) {
            return;
        }
        let left = end.saturating_duration_since(Instant::now());
        assert!(
            driver.window_system_mut().wait_for_wake(left),
            "nothing woke the runtime within {PATIENCE:?}"
        );
    }
}
