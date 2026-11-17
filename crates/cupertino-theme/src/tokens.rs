//! Design tokens: named colours per appearance and the raw typography tables.

use crate::appearance::Appearance;
use crate::color::{AccentColor, Color, SemanticColor};
use crate::error::{Error, Result};
use crate::typography::Typography;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Default)]
struct RawAppearance(HashMap<String, [u8; 4]>);

#[derive(Deserialize)]
pub(crate) struct RawTypography {
    pub sizes: HashMap<String, f64>,
    pub system_font: RawFont,
}

#[derive(Deserialize)]
pub(crate) struct RawFont {
    pub units_per_em: f64,
    pub ascender: f64,
    pub descender: f64,
    pub line_gap: f64,
    pub cap_height: f64,
    pub x_height: f64,
    /// Track → point size → value in font units.
    pub tracking: HashMap<String, HashMap<String, f64>>,
}

#[derive(Deserialize)]
struct RawTokens {
    appearances: HashMap<String, RawAppearance>,
    typography: RawTypography,
}

/// Named colours of one appearance catalog.
#[derive(Clone, Debug, Default)]
pub struct ColorTable {
    map: HashMap<String, Color>,
}

impl ColorTable {
    pub fn get(&self, name: &str) -> Option<Color> {
        self.map.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }
}

#[derive(Clone, Debug)]
pub struct DesignTokens {
    colors: HashMap<Appearance, ColorTable>,
    typography: Typography,
}

impl DesignTokens {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(json: &str) -> Result<Self> {
        let raw: RawTokens = serde_json::from_str(json)?;
        let mut colors = HashMap::new();
        for a in Appearance::ALL {
            let cat = raw
                .appearances
                .get(a.as_str())
                .ok_or_else(|| Error::NotFound(format!("colour catalog for {}", a.as_str())))?;
            let map = cat
                .0
                .iter()
                .map(|(k, v)| (k.clone(), Color::from_rgba8(*v)))
                .collect();
            colors.insert(a, ColorTable { map });
        }
        Ok(Self {
            colors,
            typography: Typography::from_raw(&raw.typography),
        })
    }

    /// The catalog of exactly this appearance (high-contrast catalogs are partial).
    pub fn table(&self, a: Appearance) -> &ColorTable {
        &self.colors[&a]
    }

    /// Colour by name. High-contrast catalogs only override part of the palette, so a missing entry falls back to the
    /// matching normal-contrast appearance.
    pub fn color(&self, a: Appearance, name: &str) -> Option<Color> {
        self.colors[&a].get(name).or_else(|| {
            if a.is_high_contrast() {
                self.colors[&a.base()].get(name)
            } else {
                None
            }
        })
    }

    pub fn semantic(&self, a: Appearance, c: SemanticColor) -> Option<Color> {
        self.color(a, c.name())
    }

    pub fn accent(&self, a: Appearance, c: AccentColor) -> Option<Color> {
        self.color(a, c.name())
    }

    pub fn typography(&self) -> &Typography {
        &self.typography
    }
}
