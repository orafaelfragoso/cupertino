//! Running an application on the session's Wayland display: the installed theme, the asset packs' fonts with the
//! built-in Inter last, the CPU renderer and the monotonic clock.

use cupertino_assets::{Assets, FontRole, FontSource, BUILTIN_FONT};
use cupertino_modifiers::Profile;
use cupertino_theme::Theme;
use cupertino_ui_core::{Application, MonotonicClock, Runtime, RuntimeOptions, UiError};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_platform::PlatformError;
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_ui_theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_ui_wayland::WaylandWindowSystem;
use std::fmt;
use std::sync::Arc;

/// The most nodes one application's tree holds.
const NODE_LIMIT: usize = 100_000;

#[derive(Debug)]
pub enum AppError {
    /// The theme data could not be read: a broken installation.
    Theme(cupertino_theme::Error),
    /// The Wayland display could not be reached.
    Connect(PlatformError),
    Runtime(UiError),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Theme(e) => write!(f, "cannot read the theme: {e}"),
            AppError::Connect(e) => write!(f, "cannot connect to the Wayland display: {e:?}"),
            AppError::Runtime(e) => write!(f, "the application stopped: {e:?}"),
        }
    }
}

impl std::error::Error for AppError {}

/// Opens `A`'s windows on the session's Wayland display and runs until the application exits or its last window
/// without a close handler is closed.
pub fn run<A: Application>() -> Result<(), AppError> {
    let options = options()?;
    let system = WaylandWindowSystem::connect(Profile::cupertino()).map_err(AppError::Connect)?;
    Runtime::<A, _>::new(system, options)
        .run()
        .map_err(AppError::Runtime)
}

/// The runtime options of an application on this system. Appearance follows Light and motion is on until the
/// system preferences reach applications.
pub fn options() -> Result<RuntimeOptions, AppError> {
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let theme = Theme::load()
        .and_then(|theme| ThemeTokens::cupertino(&theme, preferences))
        .map_err(AppError::Theme)?;
    let assets = Assets::discover();
    let family = assets
        .font_family(FontRole::Ui)
        .unwrap_or(&theme.type_scale.family)
        .to_owned();
    let text = Arc::new(TextSystem::new(fonts(&assets), &family));
    let glyphs = Arc::clone(&text);
    Ok(RuntimeOptions {
        theme: Arc::new(theme),
        text,
        direction: LayoutDirection::Ltr,
        node_limit: NODE_LIMIT,
        renderers: Box::new(move || {
            Box::new(CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 26,
                    glyph_cache_bytes: 8 << 20,
                    image_cache_bytes: 32 << 20,
                    font_smoothing: true,
                },
                GlyphRaster::new(Arc::clone(&glyphs)),
            ))
        }),
        accessibility: None,
        clock: Arc::new(MonotonicClock),
        on_window_error: Some(Box::new(|window, error| {
            eprintln!("cupertino-kit: window {window:?}: {error:?}");
        })),
    })
}

/// The asset packs' fonts, then the built-in Inter, which serves what they lack. A file that cannot be read or parsed
/// is skipped with a message.
fn fonts(assets: &Assets) -> FontIndex {
    let mut index = FontIndex::new();
    for source in assets.fonts() {
        let data = match &source {
            FontSource::File(path) => match std::fs::read(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("cupertino-kit: skipping the font {}: {e}", path.display());
                    continue;
                }
            },
            FontSource::Builtin => BUILTIN_FONT.to_vec(),
        };
        if let Err(e) = index.add_font(Arc::new(data)) {
            eprintln!("cupertino-kit: skipping the font {}: {e:?}", source.label());
        }
    }
    index
}
