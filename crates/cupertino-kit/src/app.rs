//! Running an application on the session's Wayland display: the installed theme, the asset packs' fonts and the
//! built-in Inter, the CPU renderer and the monotonic clock.

use cupertino_assets::{Assets, FontRole, FontSource, BUILTIN_FONT};
use cupertino_modifiers::Profile;
use cupertino_theme::Theme;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::{Application, MonotonicClock, Runtime, RuntimeOptions, UiError};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_platform::{PlatformError, WindowSystem};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_text::{FontIndex, FontStyle, FontWeight, GlyphRaster, TextSystem};
use cupertino_ui_theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_ui_wayland::WaylandWindowSystem;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
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

/// Opens `A`'s windows on the session's Wayland display, publishes them to assistive technology over AT-SPI, and runs
/// until the application exits or its last window without a close handler is closed.
pub fn run<A: Application>() -> Result<(), AppError> {
    let mut options = options()?;
    let system = WaylandWindowSystem::connect(Profile::cupertino()).map_err(AppError::Connect)?;
    options.accessibility = Some(Box::new(AtspiAdapter::new(system.waker())));
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
    let (fonts, builtin) = fonts(&assets);
    let preferred = [
        assets.font_family(FontRole::Ui),
        Some(&*theme.type_scale.family),
    ];
    let family = ui_family(&fonts, preferred.into_iter().flatten(), builtin.as_deref());
    let text = Arc::new(TextSystem::new(fonts, &family));
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

/// The largest font file read; the biggest real collections are a few tens of megabytes.
const MAX_FONT_BYTES: u64 = 64 << 20;

/// A regular file of at most [`MAX_FONT_BYTES`], so a pack entry that names a device, a pipe or a huge file never
/// stalls or exhausts an application at start.
fn read_font(path: &Path) -> io::Result<Vec<u8>> {
    let too_big = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file of at most 64 MiB",
        )
    };
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return Err(too_big());
    }
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return Err(too_big());
    }
    let mut data = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FONT_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_FONT_BYTES {
        return Err(too_big());
    }
    Ok(data)
}

/// The first of `preferred` that names a loaded face, else `builtin`: UI text never falls to whichever face happened
/// to load first.
fn ui_family<'a>(
    fonts: &FontIndex,
    mut preferred: impl Iterator<Item = &'a str>,
    builtin: Option<&'a str>,
) -> String {
    preferred
        .find(|family| {
            fonts
                .match_face(family, FontWeight::REGULAR, FontStyle::Normal)
                .is_some()
        })
        .or(builtin)
        .unwrap_or_default()
        .to_owned()
}

/// The asset packs' fonts, then the built-in Inter, with the family Inter registered under. A file that cannot be
/// read or parsed is skipped with a message.
fn fonts(assets: &Assets) -> (FontIndex, Option<String>) {
    let mut index = FontIndex::new();
    let mut builtin = None;
    for source in assets.fonts() {
        let data = match &source {
            FontSource::File(path) => match read_font(path) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("cupertino-kit: skipping the font {}: {e}", path.display());
                    continue;
                }
            },
            FontSource::Builtin => BUILTIN_FONT.to_vec(),
        };
        match index.add_font(Arc::new(data)) {
            Ok(faces) if matches!(source, FontSource::Builtin) => {
                builtin = faces
                    .first()
                    .and_then(|&face| index.family(face))
                    .map(str::to_owned);
            }
            Ok(_) => {}
            Err(e) => eprintln!("cupertino-kit: skipping the font {}: {e:?}", source.label()),
        }
    }
    (index, builtin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_regular_font_file_of_bounded_size_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join("a.ttf");
        std::fs::write(&font, b"font").unwrap();
        assert_eq!(read_font(&font).unwrap(), b"font");

        let huge = dir.path().join("huge.ttf");
        File::create(&huge)
            .unwrap()
            .set_len(MAX_FONT_BYTES + 1)
            .unwrap();
        assert!(read_font(&huge).is_err());

        let device = dir.path().join("zero.ttf");
        std::os::unix::fs::symlink("/dev/zero", &device).unwrap();
        assert!(read_font(&device).is_err());
        assert!(read_font(dir.path()).is_err());
    }

    #[test]
    fn ui_text_takes_a_preferred_family_that_is_loaded_else_the_built_in_one() {
        // a pack's face registered first, as Assets::fonts orders them, then the built-in Inter
        let mut fonts = FontIndex::new();
        let data = Arc::new(BUILTIN_FONT.to_vec());
        let pack = fonts
            .add_face(
                "Pack Sans",
                FontWeight::REGULAR,
                FontStyle::Normal,
                data.clone(),
                0,
            )
            .unwrap();
        let inter = fonts.add_font(data).unwrap()[0];
        let builtin = fonts.family(inter).unwrap().to_owned();
        let face = |family: &str| fonts.match_face(family, FontWeight::REGULAR, FontStyle::Normal);

        let family = ui_family(
            &fonts,
            ["Inter", "Missing Sans"].into_iter(),
            Some(&builtin),
        );
        assert_eq!(
            face(&family),
            Some(inter),
            "never the first registered face"
        );
        assert_ne!(face(&family), Some(pack));

        let family = ui_family(
            &fonts,
            ["Missing Sans", "pack sans"].into_iter(),
            Some(&builtin),
        );
        assert_eq!(
            face(&family),
            Some(pack),
            "a preferred family that is loaded wins"
        );
    }
}
