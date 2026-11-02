//! Pictures of a window for a person to compare with a reference: one PNG per appearance, at a backing scale.

use crate::{DriverError, TestDriver};
use cupertino_theme::Theme;
use cupertino_ui_core::{Application, Clock};
use cupertino_ui_platform::{PlatformError, WindowEvent, WindowId};
use cupertino_ui_render::PixelFormat;
use cupertino_ui_theme::{Appearance, ThemeTokens};
use std::fmt;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The folder [`TestDriver::look`] writes to. Unset, it writes nothing, so an ordinary test run skips it.
pub const LOOK_DIR: &str = "CUPERTINO_LOOK_DIR";
/// The backing scale of the pictures; 2 when unset.
pub const LOOK_SCALE: &str = "CUPERTINO_LOOK_SCALE";
/// The appearances, comma-separated from `light`, `dark`, `light-hc` and `dark-hc`; `light,dark` when unset.
pub const LOOK_APPEARANCES: &str = "CUPERTINO_LOOK_APPEARANCES";

/// The workspace's theme data, which the pictures' themes are made from.
const THEME_DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
const TURNS: u32 = 64;
const FRAMES: usize = 8;

#[derive(Debug)]
pub enum LookError {
    Driver(DriverError),
    Platform(PlatformError),
    /// An environment variable holds a value the helper doesn't know.
    Setting {
        name: &'static str,
        value: String,
    },
    /// The theme data can't make the appearance.
    Theme(String),
    /// The window presented no frame at the scale.
    NoFrame(WindowId),
    Write {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for LookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookError::Driver(e) => write!(f, "{e:?}"),
            LookError::Platform(e) => write!(f, "{e:?}"),
            LookError::Setting { name, value } => write!(f, "{name}={value:?} is not understood"),
            LookError::Theme(message) => write!(f, "the theme data: {message}"),
            LookError::NoFrame(window) => write!(f, "{window:?} presented no frame"),
            LookError::Write { path, message } => write!(f, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for LookError {}

impl From<DriverError> for LookError {
    fn from(e: DriverError) -> Self {
        LookError::Driver(e)
    }
}

impl From<PlatformError> for LookError {
    fn from(e: PlatformError) -> Self {
        LookError::Platform(e)
    }
}

/// The name an appearance has in [`LOOK_APPEARANCES`] and in the file names.
pub fn appearance_name(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Light => "light",
        Appearance::Dark => "dark",
        Appearance::LightHighContrast => "light-hc",
        Appearance::DarkHighContrast => "dark-hc",
    }
}

fn parse_appearances(value: &str) -> Result<Vec<Appearance>, LookError> {
    value
        .split(',')
        .map(str::trim)
        .map(|name| {
            Appearance::ALL
                .into_iter()
                .find(|a| appearance_name(*a) == name)
                .ok_or_else(|| LookError::Setting {
                    name: LOOK_APPEARANCES,
                    value: value.to_owned(),
                })
        })
        .collect()
}

fn parse_scale(value: &str) -> Result<f64, LookError> {
    value
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s > 0.0)
        .ok_or_else(|| LookError::Setting {
            name: LOOK_SCALE,
            value: value.to_owned(),
        })
}

impl<A: Application> TestDriver<A> {
    /// When [`LOOK_DIR`] names a folder, [`look_into`](Self::look_into) it with the scale and appearances the
    /// environment asks for; otherwise nothing, and no file.
    pub fn look(&mut self, window: WindowId, name: &str) -> Result<Vec<PathBuf>, LookError> {
        let Some(dir) = std::env::var_os(LOOK_DIR) else {
            return Ok(Vec::new());
        };
        let scale = match std::env::var(LOOK_SCALE) {
            Ok(value) => parse_scale(&value)?,
            Err(_) => 2.0,
        };
        let appearances = match std::env::var(LOOK_APPEARANCES) {
            Ok(value) => parse_appearances(&value)?,
            Err(_) => vec![Appearance::Light, Appearance::Dark],
        };
        self.look_into(Path::new(&dir), window, name, scale, &appearances)
    }

    /// Writes `window` as `<name>-<appearance>@<scale>x.png` in `dir` for each of `appearances` at `scale`, and
    /// returns the paths. The window is drawn as it is now, in the Cupertino theme of the workspace's `data/theme`
    /// with the window's preferences in the appearance; afterwards it has its own theme back, at scale 1.
    pub fn look_into(
        &mut self,
        dir: &Path,
        window: WindowId,
        name: &str,
        scale: f64,
        appearances: &[Appearance],
    ) -> Result<Vec<PathBuf>, LookError> {
        std::fs::create_dir_all(dir).map_err(|e| LookError::Write {
            path: dir.to_path_buf(),
            message: e.to_string(),
        })?;
        let theme = Arc::clone(self.runtime.theme());
        let data = Theme::from_dir(THEME_DATA).map_err(|e| LookError::Theme(e.to_string()))?;
        self.window_system_mut()
            .enqueue_window(window, WindowEvent::Scale(scale))?;
        let mut written = Vec::new();
        for &appearance in appearances {
            let mut preferences = theme.preferences;
            preferences.appearance = appearance;
            let look = ThemeTokens::cupertino(&data, preferences)
                .map_err(|e| LookError::Theme(e.to_string()))?;
            self.runtime.set_theme(Arc::new(look));
            let path = dir.join(format!(
                "{name}-{}@{scale}x.png",
                appearance_name(appearance)
            ));
            self.present(window)?;
            self.write(window, &path)?;
            written.push(path);
        }
        self.runtime.set_theme(theme);
        self.window_system_mut()
            .enqueue_window(window, WindowEvent::Scale(1.0))?;
        self.present(window)?;
        Ok(written)
    }

    /// Runs until the window has presented what it shows now.
    fn present(&mut self, window: WindowId) -> Result<(), LookError> {
        let turns = NonZeroU32::new(TURNS).expect("a non-zero budget");
        for _ in 0..FRAMES {
            self.run_until_idle(turns)?;
            let now = self.clock.now();
            let system = self.window_system_mut();
            if !system.frame_requested(window)? {
                return Ok(());
            }
            system.frame_ready(window, now)?;
        }
        Ok(())
    }

    fn write(&mut self, window: WindowId, path: &Path) -> Result<(), LookError> {
        let frame = self
            .window_system_mut()
            .frames(window)?
            .last()
            .ok_or(LookError::NoFrame(window))?;
        let mut pixels = frame.pixels.clone();
        if frame.format == PixelFormat::Rgba8 {
            for px in pixels.chunks_exact_mut(4) {
                let a = u16::from(px[3]);
                for c in &mut px[..3] {
                    *c = ((u16::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        let failed = |message: String| LookError::Write {
            path: path.to_path_buf(),
            message,
        };
        let size = tiny_skia::IntSize::from_wh(frame.width, frame.height)
            .ok_or_else(|| failed("the frame is empty".into()))?;
        tiny_skia::Pixmap::from_vec(pixels, size)
            .ok_or_else(|| failed("the frame's pixels don't fill its size".into()))?
            .save_png(path)
            .map_err(|e| failed(e.to_string()))
    }
}

#[cfg(test)]
#[path = "look-tests.rs"]
mod tests;
