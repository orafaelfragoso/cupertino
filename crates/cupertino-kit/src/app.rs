//! Running an application on the session's Wayland display: the installed theme, the asset packs' fonts and the
//! built-in Inter, the CPU renderer and the monotonic clock.

use cupertino_assets::{Assets, FontRole, FontSource, BUILTIN_FONT};
use cupertino_modifiers::Profile;
use cupertino_theme::Theme;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::{
    Application, MonotonicClock, Runtime, RuntimeOptions, ThemeForAppearance, UiError,
};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_platform::{PlatformError, SystemAppearance, WindowSystem};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_text::{FontIndex, FontStyle, FontWeight, GlyphRaster, TextSystem};
use cupertino_ui_theme::{Appearance, ColorTokens, ThemePreferences, ThemeTokens};
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

/// Opens `A`'s windows on the session's Wayland display, publishes them to assistive technology over AT-SPI, follows
/// the session's appearance and display options, and runs until the application exits or its last window without a
/// close handler is closed.
pub fn run<A: Application>() -> Result<(), AppError> {
    let system = WaylandWindowSystem::connect(Profile::cupertino()).map_err(AppError::Connect)?;
    // the first frame already has the session's appearance
    let (mut options, theme) = options_and_theme(start_preferences(system.appearance()))?;
    let follow = theme_for(theme, Arc::clone(&options.theme));
    options.accessibility = Some(Box::new(AtspiAdapter::muted(system.waker())));
    Runtime::<A, _>::new(system, options)
        .with_theme_for(follow)
        .run()
        .map_err(AppError::Runtime)
}

/// The runtime options of an application on this system, in the light appearance with motion on. [`run`] starts in the
/// session's appearance and display options instead and follows them.
pub fn options() -> Result<RuntimeOptions, AppError> {
    options_and_theme(start_preferences(None)).map(|(options, _)| options)
}

/// The theme preferences an application starts with: the session's when the window system knows them, else light with
/// motion on.
pub fn start_preferences(appearance: Option<SystemAppearance>) -> ThemePreferences {
    preferences(&appearance.unwrap_or_default())
}

/// The theme preferences of the session's appearance and display options.
pub fn preferences(appearance: &SystemAppearance) -> ThemePreferences {
    ThemePreferences {
        appearance: match (appearance.dark, appearance.increase_contrast) {
            (false, false) => Appearance::Light,
            (true, false) => Appearance::Dark,
            (false, true) => Appearance::LightHighContrast,
            (true, true) => Appearance::DarkHighContrast,
        },
        reduce_transparency: appearance.reduce_transparency,
        reduce_motion: appearance.reduce_motion,
    }
}

/// Makes the Cupertino theme for each appearance from `theme`. Where the theme data lacks something the theme needs,
/// the built-in neutral colours take the place of its colours, on `start`'s metrics and type.
fn theme_for(theme: Theme, start: Arc<ThemeTokens>) -> ThemeForAppearance {
    Box::new(move |appearance| {
        let preferences = preferences(appearance);
        let tokens = ThemeTokens::cupertino(&theme, preferences).unwrap_or_else(|e| {
            eprintln!("cupertino-kit: the theme cannot follow the session ({e}); using the neutral colours");
            let mut tokens = (*start).clone();
            tokens.preferences = preferences;
            tokens.colors = ColorTokens::neutral(preferences.appearance);
            tokens.motion.reduce = preferences.reduce_motion;
            tokens
        });
        Arc::new(tokens)
    })
}

fn options_and_theme(preferences: ThemePreferences) -> Result<(RuntimeOptions, Theme), AppError> {
    let data = Theme::load().map_err(AppError::Theme)?;
    let theme = ThemeTokens::cupertino(&data, preferences).map_err(AppError::Theme)?;
    let assets = Assets::discover();
    let (fonts, builtin) = fonts(&assets);
    let preferred = [
        assets.font_family(FontRole::Ui),
        Some(&*theme.type_scale.family),
    ];
    let family = ui_family(&fonts, preferred.into_iter().flatten(), builtin.as_deref());
    let text = Arc::new(TextSystem::new(fonts, &family));
    let glyphs = Arc::clone(&text);
    let options = RuntimeOptions {
        theme: Arc::new(theme),
        text,
        icons: Arc::new(crate::icons::PackIcons::new(&assets)),
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
    };
    Ok((options, data))
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
    fn the_session_appearance_picks_the_theme_appearance_and_its_preferences() {
        let session =
            |dark, increase_contrast, reduce_transparency, reduce_motion| SystemAppearance {
                dark,
                increase_contrast,
                reduce_transparency,
                reduce_motion,
            };
        assert_eq!(
            preferences(&session(false, false, false, false)).appearance,
            Appearance::Light
        );
        assert_eq!(
            preferences(&session(true, false, false, false)).appearance,
            Appearance::Dark
        );
        assert_eq!(
            preferences(&session(false, true, false, false)).appearance,
            Appearance::LightHighContrast
        );
        let all = preferences(&session(true, true, true, true));
        assert_eq!(all.appearance, Appearance::DarkHighContrast);
        assert!(all.reduce_transparency && all.reduce_motion);
    }

    #[test]
    fn an_application_starts_in_the_session_appearance_so_its_first_frame_has_it() {
        assert_eq!(
            start_preferences(None),
            preferences(&SystemAppearance::default())
        );
        assert_eq!(start_preferences(None).appearance, Appearance::Light);
        let dark = SystemAppearance {
            dark: true,
            reduce_transparency: true,
            ..SystemAppearance::default()
        };
        let (options, theme) = options_and_theme(start_preferences(Some(dark))).unwrap();
        assert_eq!(
            *options.theme,
            ThemeTokens::cupertino(&theme, preferences(&dark)).unwrap(),
            "every window opens with this theme"
        );
        assert_eq!(options.theme.preferences.appearance, Appearance::Dark);
    }

    #[test]
    fn following_the_session_makes_its_cupertino_theme() {
        let theme = Theme::load().unwrap();
        let start = Arc::new(
            ThemeTokens::cupertino(&theme, preferences(&SystemAppearance::default())).unwrap(),
        );
        let mut follow = theme_for(theme, start);
        let dark = SystemAppearance {
            dark: true,
            reduce_motion: true,
            ..SystemAppearance::default()
        };
        let tokens = follow(&dark);
        let expected = ThemeTokens::cupertino(&Theme::load().unwrap(), preferences(&dark)).unwrap();
        assert_eq!(*tokens, expected);
        assert!(tokens.motion.reduce);
    }

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
