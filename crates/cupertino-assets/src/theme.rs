//! The theme data: where its files are, and the [`Theme`] loaded from them.

use cupertino_theme::{AnimationPresets, DesignTokens, Error, MaterialLibrary, Result, Theme};
use cupertino_theme::{ShapeTokens, WindowShadowPreset};
use std::path::{Path, PathBuf};

/// Environment variable that overrides the theme data directory at run time.
pub const SPEC_DIR_ENV: &str = "CUPERTINO_SPEC_DIR";

/// Default theme data directory, fixed at compile time: `$CUPERTINO_THEME_DIR` of the build (packages set the installed
/// path, `/usr/share/cupertino/theme`), otherwise `../../data/theme` relative to this crate's manifest directory, where
/// the checkout is now (`build.rs`). Only the chosen path is compiled in, so a packaged binary holds no build-tree path.
const DEFAULT_SPEC_DIR: &str = match option_env!("CUPERTINO_THEME_DIR") {
    Some(dir) => dir,
    None => env!("CUPERTINO_REPOSITORY_THEME_DIR"),
};

/// Theme data directory: `$CUPERTINO_SPEC_DIR` if set, otherwise [`DEFAULT_SPEC_DIR`].
pub fn spec_dir() -> PathBuf {
    spec_dir_from(std::env::var_os(SPEC_DIR_ENV))
}

/// [`spec_dir`] for a given value of `$CUPERTINO_SPEC_DIR`; an empty value counts as unset.
fn spec_dir_from(env: Option<std::ffi::OsString>) -> PathBuf {
    match env {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => Path::new(DEFAULT_SPEC_DIR).to_path_buf(),
    }
}

pub const TOKENS_FILE: &str = "tokens.json";
pub const MATERIALS_FILE: &str = "materials.json";
pub const ANIMATION_FILE: &str = "animations.json";

/// Load from [`spec_dir`].
pub fn load() -> Result<Theme> {
    from_dir(spec_dir())
}

/// Load from the theme data files in `dir`.
pub fn from_dir(dir: impl AsRef<Path>) -> Result<Theme> {
    let dir = dir.as_ref();
    Ok(Theme {
        tokens: DesignTokens::from_str(&read(&dir.join(TOKENS_FILE))?)?,
        materials: MaterialLibrary::from_str(&read(&dir.join(MATERIALS_FILE))?)?,
        animation: AnimationPresets::from_str(&read(&dir.join(ANIMATION_FILE))?)?,
        shapes: ShapeTokens::default(),
        window_shadow: WindowShadowPreset::default(),
    })
}

/// The text of the theme data file at `path`.
fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}
