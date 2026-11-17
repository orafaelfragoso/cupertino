use crate::animation::AnimationPresets;
use crate::appearance::Appearance;
use crate::error::Result;
use crate::metrics::{ShapeTokens, WindowShadowPreset};
use crate::recipe::{
    app_grid_background, dock_reduced, opaque_background, running_indicator, switcher_panel,
    window_background, MaterialLibrary, MaterialOptions, Recipe, APP_GRID_MATERIAL, DOCK_MATERIAL,
    RUNNING_INDICATOR_MATERIAL, SWITCHER_MATERIAL, TEXT_BACKGROUND_MATERIAL,
    WINDOW_BACKGROUND_MATERIAL,
};
use crate::tokens::DesignTokens;
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

#[derive(Clone, Debug)]
pub struct Theme {
    pub tokens: DesignTokens,
    pub materials: MaterialLibrary,
    pub animation: AnimationPresets,
    pub shapes: ShapeTokens,
    pub window_shadow: WindowShadowPreset,
}

/// The colour token of a window's background, which the window background material draws.
const WINDOW_BACKGROUND_TOKEN: &str = "window-background";
/// The colour token of the text background material.
const TEXT_BACKGROUND_TOKEN: &str = "text-background";

impl Theme {
    /// Load from [`spec_dir`].
    pub fn load() -> Result<Self> {
        Self::from_dir(spec_dir())
    }

    pub fn from_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        Ok(Self {
            tokens: DesignTokens::from_path(dir.join(TOKENS_FILE))?,
            materials: MaterialLibrary::from_path(dir.join(MATERIALS_FILE))?,
            animation: AnimationPresets::from_path(dir.join(ANIMATION_FILE))?,
            shapes: ShapeTokens::default(),
            window_shadow: WindowShadowPreset::default(),
        })
    }

    /// Build from in-memory JSON documents (tokens, materials, animations).
    pub fn from_strs(tokens: &str, materials: &str, animation: &str) -> Result<Self> {
        Ok(Self {
            tokens: DesignTokens::from_str(tokens)?,
            materials: MaterialLibrary::from_str(materials)?,
            animation: AnimationPresets::from_str(animation)?,
            shapes: ShapeTokens::default(),
            window_shadow: WindowShadowPreset::default(),
        })
    }

    pub fn recipe(
        &self,
        appearance: Appearance,
        material: &str,
        opts: &MaterialOptions,
    ) -> Result<Recipe> {
        if material == SWITCHER_MATERIAL {
            return Ok(switcher_panel(appearance, opts.reduce_transparency));
        }
        if material == APP_GRID_MATERIAL {
            return Ok(app_grid_background(appearance, opts.reduce_transparency));
        }
        if material == DOCK_MATERIAL {
            return Ok(dock_reduced(appearance));
        }
        if material == RUNNING_INDICATOR_MATERIAL {
            return Ok(running_indicator(appearance));
        }
        if material == WINDOW_BACKGROUND_MATERIAL {
            if let Some(color) = self.tokens.color(appearance, WINDOW_BACKGROUND_TOKEN) {
                return Ok(window_background(color));
            }
        }
        if material == TEXT_BACKGROUND_MATERIAL {
            if let Some(color) = self.tokens.color(appearance, TEXT_BACKGROUND_TOKEN) {
                return Ok(opaque_background(TEXT_BACKGROUND_MATERIAL, color));
            }
        }
        self.materials.recipe(appearance, material, opts)
    }
}
