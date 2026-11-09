//! Design tokens: named colours per appearance and the raw typography tables.

use crate::appearance::Appearance;
use crate::color::{AccentColor, Color, SemanticColor};
use crate::error::{Error, Result};
use crate::typography::Typography;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

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

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_str(&text)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    fn tokens() -> DesignTokens {
        Theme::load().unwrap().tokens
    }

    #[test]
    fn documented_values_match() {
        let t = tokens();
        let hex = |a, c| t.semantic(a, c).unwrap().hex();
        assert_eq!(
            hex(Appearance::Light, SemanticColor::WindowBackground),
            "#FFFFFF"
        );
        assert_eq!(
            hex(Appearance::Dark, SemanticColor::WindowBackground),
            "#1F2025"
        );
        assert_eq!(
            hex(Appearance::Light, SemanticColor::ControlBackground),
            "#FFFFFF"
        );
        assert_eq!(
            hex(Appearance::Dark, SemanticColor::ControlBackground),
            "#1E1E1E"
        );
        assert_eq!(hex(Appearance::Light, SemanticColor::SystemBlue), "#007AFF");
        assert_eq!(hex(Appearance::Dark, SemanticColor::SystemBlue), "#0A84FF");
        assert_eq!(hex(Appearance::Light, SemanticColor::SystemRed), "#FF3B30");
        assert_eq!(hex(Appearance::Dark, SemanticColor::SystemRed), "#FF453A");
        assert_eq!(hex(Appearance::Light, SemanticColor::Grid), "#E6E6E6");
        assert_eq!(hex(Appearance::Dark, SemanticColor::Grid), "#1A1A1A");
        assert_eq!(
            hex(Appearance::Light, SemanticColor::KeyboardFocusIndicator),
            "#3B99FC"
        );
    }

    #[test]
    fn alpha_is_preserved() {
        let t = tokens();
        let label = t.semantic(Appearance::Light, SemanticColor::Label).unwrap();
        assert_eq!(label.to_rgba8(), [0, 0, 0, 216]);
        assert!((label.a - 0.847).abs() < 0.001);
        let rim = t
            .semantic(Appearance::Light, SemanticColor::WindowRim)
            .unwrap();
        assert!((rim.a - 0.349).abs() < 0.002);
        let dark_label = t.semantic(Appearance::Dark, SemanticColor::Label).unwrap();
        assert_eq!(dark_label.hex(), "#FFFFFF");
    }

    #[test]
    fn catalog_sizes() {
        let t = tokens();
        assert_eq!(t.table(Appearance::Light).len(), 88);
        assert_eq!(t.table(Appearance::Dark).len(), 87);
        assert!(t.table(Appearance::LightHighContrast).len() < t.table(Appearance::Light).len());
    }

    #[test]
    fn high_contrast_falls_back_to_base() {
        let t = tokens();
        let name = SemanticColor::KeyboardFocusIndicator.name();
        assert!(t.table(Appearance::DarkHighContrast).get(name).is_none());
        assert_eq!(
            t.color(Appearance::DarkHighContrast, name),
            t.table(Appearance::Dark).get(name)
        );
        assert!(t.color(Appearance::Light, "no-such-colour").is_none());
    }

    #[test]
    fn both_high_contrast_rims_are_dark_strokes_of_their_own() {
        let t = tokens();
        for a in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
            for rim in [SemanticColor::WindowRim, SemanticColor::WindowRim2x] {
                assert!(
                    t.table(a).get(rim.name()).is_some(),
                    "{rim:?} missing in {a:?}"
                );
                assert_eq!(t.semantic(a, rim).unwrap().to_rgba8(), [0, 0, 0, 127]);
            }
        }
    }

    #[test]
    fn every_listed_name_resolves_in_light_and_dark() {
        let t = tokens();
        for a in [Appearance::Light, Appearance::Dark] {
            for c in SemanticColor::ALL {
                assert!(t.semantic(a, *c).is_some(), "{:?} missing in {:?}", c, a);
            }
            for c in AccentColor::ALL {
                assert!(t.accent(a, *c).is_some(), "{:?} missing in {:?}", c, a);
            }
        }
    }

    /// Increase Contrast keeps the default backgrounds: it strengthens borders and text, not the surfaces under them. A
    /// light background is never darker than the default one, so content under Increase Contrast never looks dimmer
    /// than without it; in dark the window stays within a few levels of the default and the wells for text and lists
    /// are no lighter than the default ones, so they stand further apart from the window.
    #[test]
    fn high_contrast_backgrounds_keep_the_default_ones() {
        const WINDOW_SPREAD: u8 = 4;
        let t = tokens();
        let backgrounds = [
            "window-background",
            "control-background",
            "text-background",
            "text-background-disabled",
            "under-page-background",
            "tool-tip",
        ];
        for (contrast, base) in [
            (Appearance::LightHighContrast, Appearance::Light),
            (Appearance::DarkHighContrast, Appearance::Dark),
        ] {
            for name in backgrounds {
                let ic = t.color(contrast, name).unwrap().to_rgba8();
                let default = t.color(base, name).unwrap().to_rgba8();
                for c in 0..3 {
                    if !base.is_dark() {
                        assert!(
                            ic[c] >= default[c],
                            "{name}: {ic:?} darker than {default:?}"
                        );
                    } else if name == "window-background" {
                        assert!(
                            ic[c].abs_diff(default[c]) <= WINDOW_SPREAD,
                            "{name}: {ic:?} {default:?}"
                        );
                    } else {
                        assert!(
                            ic[c] <= default[c],
                            "{name}: {ic:?} lighter than {default:?}"
                        );
                    }
                }
                assert!(
                    ic[3] >= default[3],
                    "{name}: {ic:?} less opaque than {default:?}"
                );
            }
        }
    }

    #[test]
    fn from_str_rejects_garbage() {
        assert!(DesignTokens::from_str("{").is_err());
        assert!(DesignTokens::from_str("{\"appearances\":{},\"typography\":{}}").is_err());
        let light_only = "{\"appearances\":{\"light\":{}},\"typography\":{\"sizes\":{},\"system_font\":{\"units_per_em\":1,\"ascender\":1,\"descender\":0,\"line_gap\":0,\"cap_height\":1,\"x_height\":1,\"tracking\":{}}}}";
        assert!(matches!(
            DesignTokens::from_str(light_only),
            Err(Error::NotFound(_))
        ));
    }
}
