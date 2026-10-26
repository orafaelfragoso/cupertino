use crate::animation::AnimationPresets;
use crate::appearance::Appearance;
use crate::error::Result;
use crate::metrics::{ShapeTokens, WindowShadowPreset};
use crate::recipe::{app_grid_background, switcher_panel, MaterialLibrary, MaterialOptions, Recipe, APP_GRID_MATERIAL, SWITCHER_MATERIAL};
use crate::tokens::DesignTokens;
use std::path::{Path, PathBuf};

/// Environment variable that overrides the theme data directory at run time.
pub const SPEC_DIR_ENV: &str = "CUPERTINO_SPEC_DIR";

/// Theme data directory: `$CUPERTINO_SPEC_DIR` if set, otherwise `../../data/theme` relative to this crate's manifest
/// directory.
pub fn spec_dir() -> PathBuf {
    match std::env::var_os(SPEC_DIR_ENV) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/theme"),
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

    pub fn recipe(&self, appearance: Appearance, material: &str, opts: &MaterialOptions) -> Result<Recipe> {
        if material == SWITCHER_MATERIAL {
            return Ok(switcher_panel(appearance, opts.reduce_transparency));
        }
        if material == APP_GRID_MATERIAL {
            return Ok(app_grid_background(appearance.is_dark()));
        }
        self.materials.recipe(appearance, material, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dir_holds_the_theme_data() {
        let d = spec_dir();
        assert!(d.join(TOKENS_FILE).is_file(), "{}", d.display());
    }

    #[test]
    fn from_strs_matches_from_dir() {
        let d = spec_dir();
        let read = |f: &str| std::fs::read_to_string(d.join(f)).unwrap();
        let a = Theme::from_strs(&read(TOKENS_FILE), &read(MATERIALS_FILE), &read(ANIMATION_FILE)).unwrap();
        let b = Theme::load().unwrap();
        assert_eq!(a.tokens.table(Appearance::Dark).len(), b.tokens.table(Appearance::Dark).len());
        assert_eq!(a.animation.timing("default"), b.animation.timing("default"));
    }

    #[test]
    fn missing_dir_is_an_error() {
        assert!(Theme::from_dir("/nonexistent/spec/dir").is_err());
        assert!(Theme::from_strs("{}", "{}", "{}").is_err());
    }
}
