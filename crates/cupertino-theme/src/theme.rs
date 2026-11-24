use crate::animation::AnimationPresets;
use crate::appearance::Appearance;
use crate::error::Result;
use crate::metrics::{ShapeTokens, WindowShadowPreset};
use crate::recipe::{
    app_grid_background, dock_reduced, opaque_background, overview_band, overview_band_selection,
    running_indicator, switcher_panel, window_background, MaterialLibrary, MaterialOptions, Recipe,
    APP_GRID_MATERIAL, DOCK_MATERIAL, OVERVIEW_BAND_MATERIAL, OVERVIEW_BAND_SELECTION_MATERIAL,
    RUNNING_INDICATOR_MATERIAL, SWITCHER_MATERIAL, TEXT_BACKGROUND_MATERIAL,
    WINDOW_BACKGROUND_MATERIAL,
};
use crate::tokens::DesignTokens;

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
        if material == OVERVIEW_BAND_MATERIAL {
            return Ok(overview_band(appearance, opts.reduce_transparency));
        }
        if material == OVERVIEW_BAND_SELECTION_MATERIAL {
            return Ok(overview_band_selection(appearance));
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
