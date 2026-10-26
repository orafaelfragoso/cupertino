//! Material recipes of the theme data (`materials.json`): per appearance catalog and material, a ready recipe (backdrop
//! filter chain followed by fill layers) for each window state and interaction.
//!
//! * the backdrop's `scale` is the capture resolution factor and `sigma_pt` the Gaussian σ in layer points;
//! * `ColorOp::LumCurve` is a cubic Bézier of Rec.709 luma mixed by an amount, `ColorOp::Saturate` the Rec.709 saturation
//!   matrix, `ColorOp::Monochrome` a tint towards a colour;
//! * plus-lighter / plus-darker compositing and the darken / lighten / soft-light blend modes follow the PDF / W3C
//!   blend-mode definitions;
//! * a tint fill takes the desktop tint (the wallpaper's average colour) at its opacity;
//! * reduce-transparency uses the `-Opaque` variants, and the high-contrast catalogs only ship opaque variants.
//!
//! A material has six variants, keyed `<window>.<interaction>` with window `active` or `inactive` and interaction `normal`,
//! `rollover` or `pressed`; a variant the material does not draw differently names the variant it shares.

use crate::appearance::Appearance;
use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum ColorOp {
    Saturate(f32),
    /// Bézier control values v0..v3 and mix amount.
    LumCurve([f32; 4], f32),
    Monochrome { color: [f32; 3], amount: f32 },
    /// Display colour filter: row-major 3x3 matrix applied to linear light (`out = M . in`), the
    /// input decoded from and the result encoded back to sRGB.
    LinearMatrix([[f32; 3]; 3]),
    /// Display inversion: `out = 1 - in` on the encoded values.
    Invert,
    /// Display contrast `c` (0 to 1): the encoded range `[c/2, 1 - c/2]` stretched to `[0, 1]`.
    Contrast(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Blend {
    Normal = 0,
    PlusLighter = 1,
    PlusDarker = 2,
    Darken = 3,
    Lighten = 4,
    SoftLight = 5,
    Copy = 6,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CornerCurve {
    #[default]
    Circular,
    Continuous,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    /// Straight (non-premultiplied) colour; alpha already includes the layer opacity.
    pub rgba: [f32; 4],
    pub blend: Blend,
    /// Fill whose colour comes from the wallpaper average (`rgba[3]` is its opacity).
    pub tint_layer: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    pub scale: f32,
    pub sigma_pt: f32,
    pub ops: Vec<ColorOp>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recipe {
    pub name: String,
    pub material: String,
    /// Variant of the material data the recipe comes from (`<window>.<interaction>`).
    pub rendition_key: String,
    pub backdrop: Option<Backdrop>,
    pub fills: Vec<Fill>,
    pub corner_radius: Option<f32>,
    pub corner_curve: CornerCurve,
    pub notes: Vec<String>,
    /// `true` when the recipe is an `-Opaque` variant.
    pub opaque: bool,
}

impl Recipe {
    pub fn has_backdrop(&self) -> bool {
        self.backdrop.is_some()
    }

    /// Fills that need the wallpaper average colour.
    pub fn uses_tint_layer(&self) -> bool {
        self.fills.iter().any(|f| f.tint_layer)
    }
}

/// Name of the application switcher's panel material. Unlike the catalog materials it is a code
/// recipe of the switcher itself (see [`switcher_panel`]).
pub const SWITCHER_MATERIAL: &str = "Switcher";

/// The application switcher's panel material: a backdrop captured at 1/8
/// scale, saturated by 1.8 (light) or 1.6 (dark) and blurred with σ 30, a fill of gray 0.9646 at
/// opacity 0.36 (light) or gray 0.1569 at 0.40 (dark) and the wallpaper tint at opacity 0.05, on a
/// 24 pt radius. The design's (−0.01 light, +0.015 dark) has no recipe
/// operation and is left out, as for the notification platter.
///
/// A high-contrast appearance and `reduce_transparency` select the solid styles instead: gray 0.7843 (light) or 0.05882 (dark) with Increase Contrast, gray 0.8548 or 0.1255 with
/// Reduce Transparency, opaque and without backdrop. Which solid wins with both options on is not
/// recorded; the contrast one is taken (approximate).
pub fn switcher_panel(appearance: Appearance, reduce_transparency: bool) -> Recipe {
    let dark = appearance.is_dark();
    let solid = |gray: f32, style: &str| Recipe {
        name: format!("{SWITCHER_MATERIAL}-{style}"),
        material: SWITCHER_MATERIAL.into(),
        fills: vec![Fill { rgba: [gray, gray, gray, 1.0], blend: Blend::Normal, tint_layer: false }],
        corner_radius: Some(24.0),
        opaque: true,
        ..Recipe::default()
    };
    if appearance.is_high_contrast() {
        return solid(if dark { 0.05882 } else { 0.7843 }, "IncreaseContrast");
    }
    if reduce_transparency {
        return solid(if dark { 0.1255 } else { 0.8548 }, "ReduceTransparency");
    }
    let (saturation, gray, opacity) = if dark { (1.6, 0.1569, 0.40) } else { (1.8, 0.9646, 0.36) };
    Recipe {
        name: SWITCHER_MATERIAL.into(),
        material: SWITCHER_MATERIAL.into(),
        backdrop: Some(Backdrop { scale: 0.125, sigma_pt: 30.0, ops: vec![ColorOp::Saturate(saturation)] }),
        fills: vec![
            Fill { rgba: [gray, gray, gray, opacity], blend: Blend::Normal, tint_layer: false },
            Fill { rgba: [0.0, 0.0, 0.0, 0.05], blend: Blend::Normal, tint_layer: true },
        ],
        corner_radius: Some(24.0),
        notes: vec!["colorBrightness has no recipe operation and is left out".into()],
        ..Recipe::default()
    }
}

/// Name of the application grid's background material, a code recipe like the switcher's (see
/// [`app_grid_background`]).
pub const APP_GRID_MATERIAL: &str = "AppGrid";

/// The application grid's background: the desktop behind it blurred with
/// radius 30 (the design blurs with hard edges, drawn here as σ 30), saturated by 1.8 and put
/// through a colour matrix with R/G/B gain 0.55 and bias 0.05292. The matrix has no recipe
/// operation; a normal fill of gray `g` at opacity `a` computes `c·(1 − a) + g·a`, so `a = 0.45`
/// and `g = 0.05292 / 0.45` reproduce it exactly. The backdrop is captured at 1/8
/// scale as the switcher's is (approximate; the design rasterises at display scale). The same
/// in both appearances: the design has one filter chain.
pub fn app_grid_background(_dark: bool) -> Recipe {
    let gray = 0.05292 / 0.45;
    Recipe {
        name: APP_GRID_MATERIAL.into(),
        material: APP_GRID_MATERIAL.into(),
        backdrop: Some(Backdrop { scale: 0.125, sigma_pt: 30.0, ops: vec![ColorOp::Saturate(1.8)] }),
        fills: vec![Fill { rgba: [gray, gray, gray, 0.45], blend: Blend::Normal, tint_layer: false }],
        corner_radius: None,
        notes: vec!["the gain 0.55 / bias 0.05292 colour matrix is the fill gray 0.1176 at opacity 0.45".into()],
        ..Recipe::default()
    }
}

/// Bézier control values → Horner coefficients `[a, b, c, d].
pub fn curve_coeffs(v: [f32; 4]) -> [f32; 4] {
    let [v0, v1, v2, v3] = v;
    [v3 - v0 + 3.0 * (v1 - v2), 3.0 * (v0 - 2.0 * v1 + v2), 3.0 * (v1 - v0), v0]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowState {
    #[default]
    Active,
    Inactive,
}

impl WindowState {
    fn key(self) -> &'static str {
        match self {
            WindowState::Active => "active",
            WindowState::Inactive => "inactive",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interaction {
    #[default]
    Normal,
    Rollover,
    Pressed,
}

impl Interaction {
    fn key(self) -> &'static str {
        match self {
            Interaction::Normal => "normal",
            Interaction::Rollover => "rollover",
            Interaction::Pressed => "pressed",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MaterialOptions {
    pub window: WindowState,
    pub interaction: Interaction,
    pub reduce_transparency: bool,
    /// Explicit variant key (`active.normal` … `inactive.pressed`) instead of `window` and `interaction`.
    pub rendition_key: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Catalog {
    Appearance(Appearance),
}

impl Catalog {
    fn label(self) -> &'static str {
        match self {
            Catalog::Appearance(a) => a.as_str(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawOp {
    Saturate(f32),
    LuminanceCurve([f32; 4]),
    Monochrome([f32; 3]),
}

#[derive(Deserialize)]
struct RawColorOp {
    #[serde(flatten)]
    op: RawOp,
    amount: Option<f32>,
}

#[derive(Deserialize)]
struct RawBackdrop {
    scale: f32,
    sigma: f32,
    ops: Vec<RawColorOp>,
}

#[derive(Deserialize)]
struct RawFill {
    rgba: [f32; 4],
    blend: String,
    tint: bool,
}

#[derive(Deserialize)]
struct RawRecipe {
    backdrop: Option<RawBackdrop>,
    fills: Vec<RawFill>,
    corner_radius: Option<f32>,
    corner_curve: String,
    notes: Vec<String>,
    variant: String,
}

#[derive(Deserialize)]
struct RawMaterial {
    variants: BTreeMap<String, usize>,
    recipes: Vec<RawRecipe>,
}

#[derive(Deserialize)]
struct RawMaterials {
    catalogs: BTreeMap<String, BTreeMap<String, RawMaterial>>,
}

const VARIANTS: [&str; 6] = ["active.normal", "active.rollover", "active.pressed", "inactive.normal", "inactive.rollover", "inactive.pressed"];

fn blend(name: &str) -> Result<Blend> {
    Ok(match name {
        "normal" => Blend::Normal,
        "plus-lighter" => Blend::PlusLighter,
        "plus-darker" => Blend::PlusDarker,
        "darken" => Blend::Darken,
        "lighten" => Blend::Lighten,
        "soft-light" => Blend::SoftLight,
        "copy" => Blend::Copy,
        other => return Err(Error::Invalid(format!("unknown blend {other}"))),
    })
}

fn convert(raw: RawRecipe, what: &str) -> Result<Recipe> {
    let backdrop = raw.backdrop.map(|b| {
        let ops = b
            .ops
            .into_iter()
            .map(|o| {
                let amount = o.amount.unwrap_or(1.0);
                match o.op {
                    RawOp::Saturate(s) => ColorOp::Saturate(s),
                    RawOp::LuminanceCurve(v) => ColorOp::LumCurve(v, amount),
                    RawOp::Monochrome(color) => ColorOp::Monochrome { color, amount },
                }
            })
            .collect();
        Backdrop { scale: b.scale, sigma_pt: b.sigma, ops }
    });
    let fills = raw.fills.into_iter().map(|f| Ok(Fill { rgba: f.rgba, blend: blend(&f.blend)?, tint_layer: f.tint })).collect::<Result<Vec<_>>>()?;
    let corner_curve = match raw.corner_curve.as_str() {
        "circular" => CornerCurve::Circular,
        "continuous" => CornerCurve::Continuous,
        other => return Err(Error::Invalid(format!("unknown corner curve {other} in {what}"))),
    };
    Ok(Recipe { rendition_key: raw.variant, backdrop, fills, corner_radius: raw.corner_radius, corner_curve, notes: raw.notes, ..Recipe::default() })
}

/// Catalog name → material name → variant key → recipe.
#[derive(Clone, Debug)]
pub struct MaterialLibrary {
    catalogs: BTreeMap<String, BTreeMap<String, BTreeMap<String, Recipe>>>,
}

impl MaterialLibrary {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(json: &str) -> Result<Self> {
        let raw: RawMaterials = serde_json::from_str(json)?;
        let mut catalogs = BTreeMap::new();
        for (cat, materials) in raw.catalogs {
            let mut out = BTreeMap::new();
            for (name, m) in materials {
                let what = format!("{cat}/{name}");
                let recipes = m.recipes.into_iter().map(|r| convert(r, &what)).collect::<Result<Vec<_>>>()?;
                let mut variants = BTreeMap::new();
                for v in VARIANTS {
                    let i = *m.variants.get(v).ok_or_else(|| Error::NotFound(format!("variant {v} of {what}")))?;
                    let r = recipes.get(i).ok_or_else(|| Error::Invalid(format!("variant {v} of {what} names recipe {i} of {}", recipes.len())))?;
                    variants.insert(v.to_string(), r.clone());
                }
                out.insert(name, variants);
            }
            catalogs.insert(cat, out);
        }
        if catalogs.is_empty() {
            return Err(Error::NotFound("catalogs".into()));
        }
        Ok(Self { catalogs })
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io { path: path.to_path_buf(), source })?;
        Self::from_str(&text)
    }

    pub fn materials(&self, catalog: Catalog) -> Vec<&str> {
        self.catalogs.get(catalog.label()).map(|c| c.keys().map(String::as_str).collect()).unwrap_or_default()
    }

    pub fn has_material(&self, catalog: Catalog, material: &str) -> bool {
        self.catalogs.get(catalog.label()).is_some_and(|c| c.contains_key(material))
    }

    pub fn recipe(&self, appearance: Appearance, material: &str, opts: &MaterialOptions) -> Result<Recipe> {
        let want_opaque = opts.reduce_transparency || appearance.is_high_contrast();
        let mut catalogs = vec![Catalog::Appearance(appearance)];
        if appearance.is_high_contrast() {
            catalogs.push(Catalog::Appearance(appearance.base()));
        }
        let plain = material.strip_suffix("-Opaque").unwrap_or(material);
        let opaque = format!("{plain}-Opaque");
        for cat in catalogs {
            let names: Vec<&str> = if want_opaque { vec![opaque.as_str(), plain] } else { vec![material] };
            for name in names {
                if self.has_material(cat, name) {
                    return self.recipe_in(cat, name, opts);
                }
            }
        }
        Err(Error::NotFound(format!("material {material} for {}", appearance.as_str())))
    }

    pub fn recipe_in(&self, catalog: Catalog, material: &str, opts: &MaterialOptions) -> Result<Recipe> {
        let variants = self
            .catalogs
            .get(catalog.label())
            .and_then(|c| c.get(material))
            .ok_or_else(|| Error::NotFound(format!("material {}/{material}", catalog.label())))?;
        let key = opts.rendition_key.clone().unwrap_or_else(|| format!("{}.{}", opts.window.key(), opts.interaction.key()));
        let mut r = variants.get(&key).cloned().ok_or_else(|| Error::NotFound(format!("variant {key} of {}/{material}", catalog.label())))?;
        r.name = format!("{}/{material}", catalog.label());
        r.material = material.to_string();
        r.opaque = material.ends_with("-Opaque");
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    fn lib() -> MaterialLibrary {
        Theme::load().unwrap().materials
    }

    fn rec(l: &MaterialLibrary, a: Appearance, m: &str) -> Recipe {
        l.recipe(a, m, &MaterialOptions::default()).unwrap()
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn menu_bar_light() {
        let r = rec(&lib(), Appearance::Light, "MenuBar");
        let b = r.backdrop.as_ref().unwrap();
        assert_eq!((b.scale, b.sigma_pt), (0.5, 180.0));
        assert!(r.fills.is_empty());
        assert_eq!(r.rendition_key, "active.normal");
        match &b.ops[..] {
            [ColorOp::LumCurve(v, amt), ColorOp::Saturate(s)] => {
                assert!(near(v[0], 0.477) && near(v[1], 0.484) && near(v[2], 0.538) && near(v[3], 0.877));
                assert!(near(*amt, 0.08));
                assert!(near(*s, 1.2));
            }
            other => panic!("unexpected ops {other:?}"),
        }
    }

    #[test]
    fn menu_bar_dark_has_larger_sigma() {
        let r = rec(&lib(), Appearance::Dark, "MenuBar");
        assert_eq!(r.backdrop.unwrap().sigma_pt, 360.0);
    }

    #[test]
    fn sidebar_light_active_layers() {
        let r = rec(&lib(), Appearance::Light, "Sidebar");
        let b = r.backdrop.as_ref().unwrap();
        assert_eq!(b.sigma_pt, 30.0);
        assert!(matches!(b.ops[..], [ColorOp::Saturate(s)] if near(s, 2.2)));
        assert_eq!(r.fills.len(), 3);
        assert!(near(r.fills[0].rgba[0], 0.9647) && near(r.fills[0].rgba[3], 0.84));
        assert_eq!(r.fills[0].blend, Blend::Normal);
        assert_eq!(r.fills[1].blend, Blend::Darken);
        assert!(near(r.fills[1].rgba[0], 0.915));
        assert!(r.fills[2].tint_layer && near(r.fills[2].rgba[3], 0.05));
        assert!(r.uses_tint_layer());
        assert!(!r.opaque);
    }

    #[test]
    fn dark_sidebar_uses_lighten() {
        let r = rec(&lib(), Appearance::Dark, "Sidebar");
        assert_eq!(r.fills[1].blend, Blend::Lighten);
        assert!(matches!(r.backdrop.unwrap().ops[..], [ColorOp::Saturate(s)] if near(s, 2.4)));
    }

    #[test]
    fn inactive_window_is_opaque_fill() {
        let l = lib();
        let r = l.recipe(Appearance::Light, "Sidebar", &MaterialOptions { window: WindowState::Inactive, ..Default::default() }).unwrap();
        assert!(r.backdrop.is_none());
        assert_eq!(r.rendition_key, "inactive.normal");
        assert!(near(r.fills[0].rgba[0], 0.941) && near(r.fills[0].rgba[3], 1.0));
    }

    #[test]
    fn opaque_variants_for_reduce_transparency() {
        let l = lib();
        let o = MaterialOptions { reduce_transparency: true, ..Default::default() };
        for m in ["Sidebar", "Menu", "MenuBar", "Titlebar", "Selection"] {
            let r = l.recipe(Appearance::Light, m, &o).unwrap();
            assert!(r.opaque && r.backdrop.is_none(), "{m}");
            assert!(!r.fills.is_empty(), "{m}");
            assert_eq!(r.name, format!("light/{m}-Opaque"));
        }
        let r = l.recipe(Appearance::Light, "Sidebar-Opaque", &MaterialOptions::default()).unwrap();
        assert!(r.opaque);
        assert!(near(r.fills[0].rgba[0], 0.965));
    }

    #[test]
    fn high_contrast_catalogs_are_opaque_with_fallback() {
        let l = lib();
        for a in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
            for m in ["Sidebar", "Titlebar", "MenuBar", "Menu", "Sheet"] {
                let r = l.recipe(a, m, &MaterialOptions::default()).unwrap();
                assert!(r.opaque && r.backdrop.is_none(), "{a:?} {m}");
            }
        }
        let own = l.recipe(Appearance::DarkHighContrast, "Sidebar", &MaterialOptions::default()).unwrap();
        assert!(own.name.starts_with("dark-high-contrast/"));
        assert!(!l.has_material(Catalog::Appearance(Appearance::DarkHighContrast), "Menu-Opaque"));
        let fb = l.recipe(Appearance::DarkHighContrast, "Menu", &MaterialOptions::default()).unwrap();
        assert!(fb.name.starts_with("dark/"), "{}", fb.name);
    }

    #[test]
    fn selection_states_and_blend_modes() {
        let l = lib();
        let sel = |i| l.recipe(Appearance::Light, "MenuSelection", &MaterialOptions { interaction: i, ..Default::default() }).unwrap();
        let normal = sel(Interaction::Normal);
        let pressed = sel(Interaction::Pressed);
        assert_eq!(normal.rendition_key, "active.normal");
        assert_eq!(pressed.rendition_key, "active.pressed");
        assert_ne!(normal.fills.len(), pressed.fills.len());
        let bar = rec(&l, Appearance::Light, "MenuBarSelection");
        assert_eq!(bar.fills.len(), 1);
        assert_eq!(bar.fills[0].blend, Blend::PlusLighter);
        assert_eq!(bar.corner_radius, Some(4.0));
        assert_eq!(bar.corner_curve, CornerCurve::Continuous);
        let inactive = l.recipe(Appearance::Light, "MenuBarSelection", &MaterialOptions { window: WindowState::Inactive, ..Default::default() }).unwrap();
        assert_eq!(inactive.fills[0].blend, Blend::PlusDarker);
    }

    #[test]
    fn explicit_rendition_key() {
        let l = lib();
        let r = l.recipe(Appearance::Light, "MenuBar", &MaterialOptions { rendition_key: Some("inactive.normal".into()), ..Default::default() }).unwrap();
        assert_eq!(r.rendition_key, "inactive.normal");
        assert!(l.recipe(Appearance::Light, "MenuBar", &MaterialOptions { rendition_key: Some("nope".into()), ..Default::default() }).is_err());
        assert!(l.recipe(Appearance::Light, "NoSuchMaterial", &MaterialOptions::default()).is_err());
    }

    #[test]
    fn the_switcher_panel_follows_the_dock_code_values() {
        let light = switcher_panel(Appearance::Light, false);
        assert_eq!(light.backdrop, Some(Backdrop { scale: 0.125, sigma_pt: 30.0, ops: vec![ColorOp::Saturate(1.8)] }));
        assert_eq!(light.fills[0].rgba, [0.9646, 0.9646, 0.9646, 0.36]);
        let dark = switcher_panel(Appearance::Dark, false);
        assert_eq!(dark.backdrop.clone().unwrap().ops, vec![ColorOp::Saturate(1.6)]);
        assert_eq!(dark.fills[0].rgba, [0.1569, 0.1569, 0.1569, 0.40]);
        assert_eq!((dark.fills[1].tint_layer, dark.fills[1].rgba[3]), (true, 0.05));
        assert_eq!(dark.corner_radius, Some(24.0));
    }

    #[test]
    fn the_switcher_panel_is_solid_under_reduce_transparency_and_increase_contrast() {
        let gray = |r: &Recipe| {
            assert!(r.opaque && r.backdrop.is_none() && r.fills.len() == 1, "{}", r.name);
            assert_eq!(r.corner_radius, Some(24.0));
            r.fills[0].rgba
        };
        assert_eq!(gray(&switcher_panel(Appearance::Light, true)), [0.8548, 0.8548, 0.8548, 1.0]);
        assert_eq!(gray(&switcher_panel(Appearance::Dark, true)), [0.1255, 0.1255, 0.1255, 1.0]);
        assert_eq!(gray(&switcher_panel(Appearance::LightHighContrast, false)), [0.7843, 0.7843, 0.7843, 1.0]);
        assert_eq!(gray(&switcher_panel(Appearance::DarkHighContrast, true)), [0.05882, 0.05882, 0.05882, 1.0]);
    }

    #[test]
    fn the_app_grid_background_is_the_colour_matrix_as_a_fill() {
        let recipe = app_grid_background(false);
        assert_eq!(recipe.backdrop, Some(Backdrop { scale: 0.125, sigma_pt: 30.0, ops: vec![ColorOp::Saturate(1.8)] }));
        let [r, g, b, a] = recipe.fills[0].rgba;
        assert_eq!((r, g, b), (g, g, g));
        // a backdrop gray c comes out as c·0.55 + 0.05292 for every c
        for c in [0.0f32, 0.3, 1.0] {
            let out = c * (1.0 - a) + g * a;
            assert!((out - (c * 0.55 + 0.05292)).abs() < 1e-6, "c = {c}: {out}");
        }
        assert_eq!(recipe.corner_radius, None);
        assert_eq!(app_grid_background(true), recipe, "one filter chain for both appearances");
    }

    #[test]
    fn every_material_parses_in_every_state() {
        let l = lib();
        let mut count = 0;
        let cats: Vec<Catalog> = Appearance::ALL.iter().map(|a| Catalog::Appearance(*a)).collect();
        for cat in cats {
            let names = l.materials(cat);
            assert!(!names.is_empty());
            for m in names {
                for window in [WindowState::Active, WindowState::Inactive] {
                    for interaction in [Interaction::Normal, Interaction::Rollover, Interaction::Pressed] {
                        let r = l.recipe_in(cat, m, &MaterialOptions { window, interaction, ..Default::default() }).unwrap_or_else(|e| panic!("{cat:?}/{m}: {e}"));
                        assert!(r.backdrop.is_some() || !r.fills.is_empty() || !r.notes.is_empty(), "{cat:?}/{m} {window:?} {interaction:?} produced an empty recipe");
                        if let Some(b) = &r.backdrop {
                            assert!(b.scale > 0.0 && b.scale <= 1.0 && b.sigma_pt >= 0.0);
                        }
                        for f in &r.fills {
                            assert!(f.rgba.iter().all(|c| (0.0..=1.0).contains(c)));
                        }
                        assert_eq!(r.opaque, m.ends_with("-Opaque"));
                        count += 1;
                    }
                }
            }
        }
        assert!(count > 400, "parsed {count} recipes");
    }

    #[test]
    fn only_transparent_materials_have_backdrops() {
        let l = lib();
        for a in Appearance::ALL {
            for m in l.materials(Catalog::Appearance(a)) {
                if m.ends_with("-Opaque") {
                    let r = l.recipe_in(Catalog::Appearance(a), m, &MaterialOptions::default()).unwrap();
                    assert!(r.backdrop.is_none(), "{a:?}/{m}");
                }
            }
        }
    }

    #[test]
    fn curve_coefficients_match_bezier() {
        let v = [0.477, 0.484, 0.538, 0.877];
        let c = curve_coeffs(v);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let horner = ((c[0] * t + c[1]) * t + c[2]) * t + c[3];
            let m = 1.0 - t;
            let bez = m * m * m * v[0] + 3.0 * m * m * t * v[1] + 3.0 * m * t * t * v[2] + t * t * t * v[3];
            assert!((horner - bez).abs() < 1e-5);
        }
    }

    #[test]
    fn rejects_malformed_documents() {
        assert!(MaterialLibrary::from_str("{}").is_err());
        assert!(MaterialLibrary::from_str("[").is_err());
    }
}
