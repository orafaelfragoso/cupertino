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
use crate::color::Color;
use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum ColorOp {
    Saturate(f32),
    /// Bézier control values v0..v3 and mix amount.
    LumCurve([f32; 4], f32),
    Monochrome {
        color: [f32; 3],
        amount: f32,
    },
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
        fills: vec![Fill {
            rgba: [gray, gray, gray, 1.0],
            blend: Blend::Normal,
            tint_layer: false,
        }],
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
    let (saturation, gray, opacity) = if dark {
        (1.6, 0.1569, 0.40)
    } else {
        (1.8, 0.9646, 0.36)
    };
    Recipe {
        name: SWITCHER_MATERIAL.into(),
        material: SWITCHER_MATERIAL.into(),
        backdrop: Some(Backdrop {
            scale: 0.125,
            sigma_pt: 30.0,
            ops: vec![ColorOp::Saturate(saturation)],
        }),
        fills: vec![
            Fill {
                rgba: [gray, gray, gray, opacity],
                blend: Blend::Normal,
                tint_layer: false,
            },
            Fill {
                rgba: [0.0, 0.0, 0.0, 0.05],
                blend: Blend::Normal,
                tint_layer: true,
            },
        ],
        corner_radius: Some(24.0),
        notes: vec!["colorBrightness has no recipe operation and is left out".into()],
        ..Recipe::default()
    }
}

/// Name of the application grid's background material, a code recipe like the switcher's (see
/// [`app_grid_background`]).
pub const APP_GRID_MATERIAL: &str = "AppGrid";

/// The application grid's background: the desktop behind it blurred with σ [`APP_GRID_SIGMA`] points, saturated by
/// [`APP_GRID_SATURATION`], then a gain of 0.36 and a lift of 0.188 on every channel: a normal fill of gray `g` at
/// opacity `a` computes `c·(1 − a) + g·a`, so [`APP_GRID_COVER`] and [`APP_GRID_GRAY`] give them. White behind it comes
/// out a mid gray, black a dark one, and colours keep much of their strength through the saturation (approximate).
/// The backdrop is captured at 1/8 scale as the switcher's is. In the light appearance the fill is the light gray
/// [`APP_GRID_GRAY_LIGHT`] instead (and [`APP_GRID_CONTRAST_GRAY_LIGHT`], [`APP_GRID_REDUCED_GRAY_LIGHT`] for the
/// display options), so its dark labels stay readable.
///
/// The display options keep the blur and change the colours rather than turning the background solid:
/// * Reduce Transparency covers it with gray [`APP_GRID_REDUCED_GRAY`] at opacity [`APP_GRID_REDUCED_OPACITY`], a
///   near-black that lets only faint bands of what is behind through (approximate);
/// * Increase Contrast (a high-contrast appearance) lowers the saturation to [`APP_GRID_CONTRAST_SATURATION`] and the
///   lift to that of gray [`APP_GRID_CONTRAST_GRAY`], so the bands stay clearly visible on a darker panel
///   (approximate);
/// * with both, Reduce Transparency's.
pub fn app_grid_background(appearance: Appearance, reduce_transparency: bool) -> Recipe {
    let backdrop = |saturation: f32| {
        Some(Backdrop {
            scale: 0.125,
            sigma_pt: APP_GRID_SIGMA,
            ops: vec![ColorOp::Saturate(saturation)],
        })
    };
    let pick = |dark: f32, light: f32| if appearance.is_dark() { dark } else { light };
    let fill = |gray: f32, opacity: f32| Fill {
        rgba: [gray, gray, gray, opacity],
        blend: Blend::Normal,
        tint_layer: false,
    };
    if reduce_transparency {
        return Recipe {
            name: format!("{APP_GRID_MATERIAL}-ReduceTransparency"),
            material: APP_GRID_MATERIAL.into(),
            backdrop: backdrop(APP_GRID_SATURATION),
            fills: vec![fill(
                pick(APP_GRID_REDUCED_GRAY, APP_GRID_REDUCED_GRAY_LIGHT),
                APP_GRID_REDUCED_OPACITY,
            )],
            opaque: true,
            ..Recipe::default()
        };
    }
    if appearance.is_high_contrast() {
        return Recipe {
            name: format!("{APP_GRID_MATERIAL}-IncreaseContrast"),
            material: APP_GRID_MATERIAL.into(),
            backdrop: backdrop(APP_GRID_CONTRAST_SATURATION),
            fills: vec![fill(
                pick(APP_GRID_CONTRAST_GRAY, APP_GRID_CONTRAST_GRAY_LIGHT),
                APP_GRID_COVER,
            )],
            ..Recipe::default()
        };
    }
    Recipe {
        name: APP_GRID_MATERIAL.into(),
        material: APP_GRID_MATERIAL.into(),
        backdrop: backdrop(APP_GRID_SATURATION),
        fills: vec![fill(
            pick(APP_GRID_GRAY, APP_GRID_GRAY_LIGHT),
            APP_GRID_COVER,
        )],
        corner_radius: None,
        ..Recipe::default()
    }
}

/// The application grid's blur in points, so broad bands of what is behind stay apart, its saturation of the blurred
/// desktop, and the gray and the opacity of the fill over it.
pub const APP_GRID_SIGMA: f32 = 20.0;
pub const APP_GRID_SATURATION: f32 = 2.8;
pub const APP_GRID_GRAY: f32 = 0.294;
pub const APP_GRID_COVER: f32 = 0.64;
/// The application grid's saturation and fill gray with Increase Contrast, at opacity [`APP_GRID_COVER`].
pub const APP_GRID_CONTRAST_SATURATION: f32 = 1.4;
pub const APP_GRID_CONTRAST_GRAY: f32 = 0.17;
/// The fill grays of the application grid in the light appearance: at rest, with Increase Contrast and with Reduce
/// Transparency.
pub const APP_GRID_GRAY_LIGHT: f32 = 0.92;
pub const APP_GRID_CONTRAST_GRAY_LIGHT: f32 = 0.80;
pub const APP_GRID_REDUCED_GRAY_LIGHT: f32 = 0.92;

/// The near-black the application grid's background takes with Reduce Transparency, and how much of it covers the
/// blurred desktop.
pub const APP_GRID_REDUCED_GRAY: f32 = 0.0784;
pub const APP_GRID_REDUCED_OPACITY: f32 = 0.98;

/// Name of the Dock bar's material with Reduce Transparency, a code recipe (see [`dock_reduced`]). At other times the
/// bar is glass.
pub const DOCK_MATERIAL: &str = "Dock";

/// The Dock bar and its stack panel with Reduce Transparency: a solid-looking body in the colour of the wallpaper under
/// it rather than a neutral gray. The desktop is captured at 1/8 scale and blurred with σ [`DOCK_REDUCED_SIGMA`], so the
/// body shows one broad colour with no detail, saturated by [`DOCK_REDUCED_SATURATION`], then covered by black (dark,
/// a deep navy over a blue wallpaper) or white (light) at [`DOCK_REDUCED_COVER`] (approximate).
pub fn dock_reduced(appearance: Appearance) -> Recipe {
    let c = if appearance.is_dark() { 0.0 } else { 1.0 };
    Recipe {
        name: format!("{DOCK_MATERIAL}-ReduceTransparency"),
        material: DOCK_MATERIAL.into(),
        backdrop: Some(Backdrop {
            scale: 0.125,
            sigma_pt: DOCK_REDUCED_SIGMA,
            ops: vec![ColorOp::Saturate(DOCK_REDUCED_SATURATION)],
        }),
        fills: vec![Fill {
            rgba: [c, c, c, DOCK_REDUCED_COVER],
            blend: Blend::Normal,
            tint_layer: false,
        }],
        opaque: true,
        ..Recipe::default()
    }
}

/// Blur of the Dock's reduced-transparency body in points, its saturation and how much black or white covers it.
pub const DOCK_REDUCED_SIGMA: f32 = 40.0;
pub const DOCK_REDUCED_SATURATION: f32 = 3.2;
pub const DOCK_REDUCED_COVER: f32 = 0.87;

/// Name of the Dock's running indicator material, a code recipe (see [`running_indicator`]).
pub const RUNNING_INDICATOR_MATERIAL: &str = "RunningIndicator";

/// The Dock's running indicator: no backdrop of its own, the gray [`RUNNING_INDICATOR_LIFT`] added to what lies behind
/// it (plus-lighter) in the dark appearance, so the dot is a lighter, brighter version of the glass under it; in the
/// light appearance the same gray is taken away (plus-darker).
pub fn running_indicator(appearance: Appearance) -> Recipe {
    let (gray, blend) = if appearance.is_dark() {
        (RUNNING_INDICATOR_LIFT, Blend::PlusLighter)
    } else {
        (1.0 - RUNNING_INDICATOR_LIFT, Blend::PlusDarker)
    };
    Recipe {
        name: RUNNING_INDICATOR_MATERIAL.into(),
        material: RUNNING_INDICATOR_MATERIAL.into(),
        fills: vec![Fill {
            rgba: [gray, gray, gray, 1.0],
            blend,
            tint_layer: false,
        }],
        ..Recipe::default()
    }
}

/// How much lighter the running indicator is than what lies behind it, per channel.
pub const RUNNING_INDICATOR_LIFT: f32 = 0.49;

/// Name of the window background material, a code recipe (see [`window_background`]).
pub const WINDOW_BACKGROUND_MATERIAL: &str = "WindowBackground";

/// Name of the text background material: the colour behind a form's groups and other text areas, drawn by the
/// compositor as the window background is (see [`opaque_background`]).
pub const TEXT_BACKGROUND_MATERIAL: &str = "TextBackground";

/// A window's own background drawn by the compositor: the opaque colour `color` and nothing of what lies behind, so
/// glass over it in the same window reads the window's background rather than the desktop.
pub fn window_background(color: Color) -> Recipe {
    opaque_background(WINDOW_BACKGROUND_MATERIAL, color)
}

/// An opaque background of the window drawn by the compositor under the material `name`: the colour `color` and
/// nothing of what lies behind.
pub fn opaque_background(name: &str, color: Color) -> Recipe {
    Recipe {
        name: name.into(),
        material: name.into(),
        fills: vec![Fill {
            rgba: [color.r, color.g, color.b, 1.0],
            blend: Blend::Copy,
            tint_layer: false,
        }],
        opaque: true,
        ..Recipe::default()
    }
}

/// Bézier control values → Horner coefficients `[a, b, c, d].
pub fn curve_coeffs(v: [f32; 4]) -> [f32; 4] {
    let [v0, v1, v2, v3] = v;
    [
        v3 - v0 + 3.0 * (v1 - v2),
        3.0 * (v0 - 2.0 * v1 + v2),
        3.0 * (v1 - v0),
        v0,
    ]
}

/// The materials of a window's own body: its sidebars, its title bars and its header. A window that is not active
/// draws their translucent variants as the data's own inactive variant: an opaque base laid with the desktop tint,
/// without the backdrop, so it keeps the tint of the wallpaper whatever lies behind it and reads the same over a busy
/// or a dark scene. Their opaque variants (Reduce Transparency) it draws as an active window does, since the data's
/// inactive opaque variants are darker than the window; what else changes is its text, its selection and its window
/// buttons, which their own roles follow. An explicit [`MaterialOptions::rendition_key`] still reads any variant.
pub const KEPT_WHEN_INACTIVE: [&str; 5] = [
    "Sidebar",
    "InlineSidebar",
    "Titlebar",
    "SeamlessWindowTitlebar",
    "Header",
];

/// Whether a window that is not active draws `material` as an active one does: the `-Opaque` variant of a material in
/// [`KEPT_WHEN_INACTIVE`].
pub fn kept_when_inactive(material: &str) -> bool {
    material
        .strip_suffix("-Opaque")
        .is_some_and(|plain| KEPT_WHEN_INACTIVE.contains(&plain))
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

const VARIANTS: [&str; 6] = [
    "active.normal",
    "active.rollover",
    "active.pressed",
    "inactive.normal",
    "inactive.rollover",
    "inactive.pressed",
];

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
        Backdrop {
            scale: b.scale,
            sigma_pt: b.sigma,
            ops,
        }
    });
    let fills = raw
        .fills
        .into_iter()
        .map(|f| {
            Ok(Fill {
                rgba: f.rgba,
                blend: blend(&f.blend)?,
                tint_layer: f.tint,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let corner_curve = match raw.corner_curve.as_str() {
        "circular" => CornerCurve::Circular,
        "continuous" => CornerCurve::Continuous,
        other => {
            return Err(Error::Invalid(format!(
                "unknown corner curve {other} in {what}"
            )))
        }
    };
    Ok(Recipe {
        rendition_key: raw.variant,
        backdrop,
        fills,
        corner_radius: raw.corner_radius,
        corner_curve,
        notes: raw.notes,
        ..Recipe::default()
    })
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
                let recipes = m
                    .recipes
                    .into_iter()
                    .map(|r| convert(r, &what))
                    .collect::<Result<Vec<_>>>()?;
                let mut variants = BTreeMap::new();
                for v in VARIANTS {
                    let i = *m
                        .variants
                        .get(v)
                        .ok_or_else(|| Error::NotFound(format!("variant {v} of {what}")))?;
                    let r = recipes.get(i).ok_or_else(|| {
                        Error::Invalid(format!(
                            "variant {v} of {what} names recipe {i} of {}",
                            recipes.len()
                        ))
                    })?;
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
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_str(&text)
    }

    pub fn materials(&self, catalog: Catalog) -> Vec<&str> {
        self.catalogs
            .get(catalog.label())
            .map(|c| c.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    pub fn has_material(&self, catalog: Catalog, material: &str) -> bool {
        self.catalogs
            .get(catalog.label())
            .is_some_and(|c| c.contains_key(material))
    }

    pub fn recipe(
        &self,
        appearance: Appearance,
        material: &str,
        opts: &MaterialOptions,
    ) -> Result<Recipe> {
        let want_opaque = opts.reduce_transparency || appearance.is_high_contrast();
        let mut catalogs = vec![Catalog::Appearance(appearance)];
        if appearance.is_high_contrast() {
            catalogs.push(Catalog::Appearance(appearance.base()));
        }
        let plain = material.strip_suffix("-Opaque").unwrap_or(material);
        let opaque = format!("{plain}-Opaque");
        for cat in catalogs {
            let names: Vec<&str> = if want_opaque {
                vec![opaque.as_str(), plain]
            } else {
                vec![material]
            };
            for name in names {
                if self.has_material(cat, name) {
                    return self.recipe_in(cat, name, opts);
                }
            }
        }
        Err(Error::NotFound(format!(
            "material {material} for {}",
            appearance.as_str()
        )))
    }

    pub fn recipe_in(
        &self,
        catalog: Catalog,
        material: &str,
        opts: &MaterialOptions,
    ) -> Result<Recipe> {
        let variants = self
            .catalogs
            .get(catalog.label())
            .and_then(|c| c.get(material))
            .ok_or_else(|| Error::NotFound(format!("material {}/{material}", catalog.label())))?;
        let window = if kept_when_inactive(material) {
            WindowState::Active
        } else {
            opts.window
        };
        let key = opts
            .rendition_key
            .clone()
            .unwrap_or_else(|| format!("{}.{}", window.key(), opts.interaction.key()));
        let mut r = variants.get(&key).cloned().ok_or_else(|| {
            Error::NotFound(format!("variant {key} of {}/{material}", catalog.label()))
        })?;
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
        assert_eq!(r.fills.len(), 1);
        assert_eq!(r.rendition_key, "active.normal");
        match &b.ops[..] {
            [ColorOp::LumCurve(v, amt), ColorOp::Saturate(s)] => {
                assert!(
                    near(v[0], 0.477)
                        && near(v[1], 0.484)
                        && near(v[2], 0.538)
                        && near(v[3], 0.877)
                );
                assert!(near(*amt, 0.08));
                assert!(near(*s, 1.2));
            }
            other => panic!("unexpected ops {other:?}"),
        }
    }

    /// The bar's own backdrop (`show.background`) lays one plain grey over its blur: 0.9647 at 80 % in the light
    /// appearance and 0.1765 at 90 % in the dark one, in every variant.
    #[test]
    fn menu_bar_lays_a_grey_fill_over_its_blur_in_each_appearance() {
        for (appearance, grey, alpha) in [
            (Appearance::Light, 0.9647, 0.8),
            (Appearance::Dark, 0.1765, 0.9),
        ] {
            for variant in ["active.normal", "inactive.normal"] {
                let r = lib()
                    .recipe(
                        appearance,
                        "MenuBar",
                        &MaterialOptions {
                            rendition_key: Some(variant.into()),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                assert!(
                    r.backdrop.is_some(),
                    "{appearance:?} {variant}: the blur stays"
                );
                match &r.fills[..] {
                    [fill] => {
                        assert_eq!(
                            fill.rgba,
                            [grey, grey, grey, alpha],
                            "{appearance:?} {variant}"
                        );
                        assert_eq!(fill.blend, Blend::Normal);
                        assert!(!fill.tint_layer);
                    }
                    other => panic!("{appearance:?} {variant}: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn menu_bar_dark_has_larger_sigma() {
        let r = rec(&lib(), Appearance::Dark, "MenuBar");
        assert_eq!(r.backdrop.unwrap().sigma_pt, 360.0);
    }

    #[test]
    fn sidebar_light_active_is_one_translucent_fill_over_a_saturated_backdrop() {
        let r = rec(&lib(), Appearance::Light, "Sidebar");
        let b = r.backdrop.as_ref().unwrap();
        assert_eq!(b.sigma_pt, 30.0);
        assert!(matches!(b.ops[..], [ColorOp::Saturate(s)] if near(s, 2.2)));
        assert_eq!(r.fills.len(), 1);
        assert!(near(r.fills[0].rgba[0], 0.9137) && near(r.fills[0].rgba[3], 0.98));
        assert_eq!(r.fills[0].blend, Blend::Normal);
        assert!(!r.uses_tint_layer());
        assert!(!r.opaque);
    }

    #[test]
    fn dark_sidebar_is_one_translucent_fill_over_a_more_saturated_backdrop() {
        let r = rec(&lib(), Appearance::Dark, "Sidebar");
        assert_eq!(r.fills.len(), 1);
        assert_eq!(r.fills[0].blend, Blend::Normal);
        assert!(near(r.fills[0].rgba[0], 0.137) && near(r.fills[0].rgba[3], 0.966));
        assert!(matches!(r.backdrop.unwrap().ops[..], [ColorOp::Saturate(s)] if near(s, 2.4)));
    }

    /// A window that is not active keeps its body's opaque materials (Reduce Transparency) as an active one draws
    /// them, in every appearance and interaction, so it grows no darker; their translucent variants draw the data's
    /// own inactive variant, an opaque base with the desktop tint (at least where the active one has it) and no backdrop, not darker than the window's
    /// background by more than three levels of 255 in the dark appearance.
    #[test]
    fn an_inactive_window_keeps_its_body_s_opaque_materials_and_tints_the_translucent_ones() {
        let l = lib();
        let theme = Theme::load().unwrap();
        let mut translucent = 0;
        for appearance in Appearance::ALL {
            for reduce_transparency in [false, true] {
                for interaction in [
                    Interaction::Normal,
                    Interaction::Rollover,
                    Interaction::Pressed,
                ] {
                    for material in KEPT_WHEN_INACTIVE {
                        let opts = |window| MaterialOptions {
                            window,
                            interaction,
                            reduce_transparency,
                            ..Default::default()
                        };
                        let (Ok(active), Ok(inactive)) = (
                            l.recipe(appearance, material, &opts(WindowState::Active)),
                            l.recipe(appearance, material, &opts(WindowState::Inactive)),
                        ) else {
                            continue;
                        };
                        let what = format!(
                            "{appearance:?} {material} rt={reduce_transparency} {interaction:?}"
                        );
                        if inactive.opaque {
                            assert_eq!(active, inactive, "{what}");
                            continue;
                        }
                        assert!(inactive.rendition_key.starts_with("inactive."), "{what}");
                        assert!(inactive.backdrop.is_none(), "{what}: no backdrop");
                        let base = &inactive.fills[0];
                        assert!(
                            base.rgba[3] == 1.0 && !base.tint_layer,
                            "{what}: an opaque base"
                        );
                        if interaction == Interaction::Normal {
                            translucent += 1;
                            assert!(
                                inactive.uses_tint_layer() || !active.uses_tint_layer(),
                                "{what}: the desktop tint, where the active one has it"
                            );
                            let window =
                                theme.tokens.color(appearance, "window-background").unwrap();
                            let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                            let base = luma([base.rgba[0], base.rgba[1], base.rgba[2]]);
                            if appearance.is_dark() {
                                assert!(
                                    base >= luma([window.r, window.g, window.b]) - 3.0 / 255.0,
                                    "{what}: {base} is darker than the window"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(translucent, 10, "five materials in two appearances");
    }

    /// Only the opaque variants of a window's body materials are kept when inactive; a menu, a selection or a sheet
    /// draws its own inactive variant, as does a translucent body material.
    #[test]
    fn only_a_window_s_opaque_body_materials_are_kept_when_inactive() {
        assert!(kept_when_inactive("Sidebar-Opaque") && kept_when_inactive("Titlebar-Opaque"));
        assert!(kept_when_inactive("Header-Opaque") && kept_when_inactive("InlineSidebar-Opaque"));
        for other in [
            "Sidebar",
            "InlineSidebar",
            "Titlebar",
            "Menu",
            "Menu-Opaque",
            "Selection",
            "MenuSelection",
            "Sheet",
            "MenuBar",
            "Glass",
        ] {
            assert!(!kept_when_inactive(other), "{other}");
        }
        let l = lib();
        let inactive = |m| {
            l.recipe(
                Appearance::Dark,
                m,
                &MaterialOptions {
                    window: WindowState::Inactive,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        assert_eq!(inactive("Selection").rendition_key, "inactive.normal");
        let sidebar = inactive("Sidebar");
        assert_eq!(sidebar.rendition_key, "inactive.normal");
        assert!(sidebar.backdrop.is_none());
        // a grey near the window's background, laid with the desktop tint strongly enough that the wallpaper's colour
        // still reads in the sidebar of a window that is not active
        assert!(near(sidebar.fills[0].rgba[0], 0.13) && near(sidebar.fills[0].rgba[3], 1.0));
        assert!(sidebar.fills[1].tint_layer && near(sidebar.fills[1].rgba[3], 0.145));
    }

    #[test]
    fn opaque_variants_for_reduce_transparency() {
        let l = lib();
        let o = MaterialOptions {
            reduce_transparency: true,
            ..Default::default()
        };
        for m in ["Sidebar", "Menu", "MenuBar", "Titlebar", "Selection"] {
            let r = l.recipe(Appearance::Light, m, &o).unwrap();
            assert!(r.opaque && r.backdrop.is_none(), "{m}");
            assert!(!r.fills.is_empty(), "{m}");
            assert_eq!(r.name, format!("light/{m}-Opaque"));
        }
        let r = l
            .recipe(
                Appearance::Light,
                "Sidebar-Opaque",
                &MaterialOptions::default(),
            )
            .unwrap();
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
        let own = l
            .recipe(
                Appearance::DarkHighContrast,
                "Sidebar",
                &MaterialOptions::default(),
            )
            .unwrap();
        assert!(own.name.starts_with("dark-high-contrast/"));
        assert!(!l.has_material(
            Catalog::Appearance(Appearance::DarkHighContrast),
            "Menu-Opaque"
        ));
        let fb = l
            .recipe(
                Appearance::DarkHighContrast,
                "Menu",
                &MaterialOptions::default(),
            )
            .unwrap();
        assert!(fb.name.starts_with("dark/"), "{}", fb.name);
    }

    #[test]
    fn selection_states_and_blend_modes() {
        let l = lib();
        let sel = |i| {
            l.recipe(
                Appearance::Light,
                "MenuSelection",
                &MaterialOptions {
                    interaction: i,
                    ..Default::default()
                },
            )
            .unwrap()
        };
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
        let inactive = l
            .recipe(
                Appearance::Light,
                "MenuBarSelection",
                &MaterialOptions {
                    window: WindowState::Inactive,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(inactive.fills[0].blend, Blend::PlusDarker);
    }

    #[test]
    fn explicit_rendition_key() {
        let l = lib();
        let r = l
            .recipe(
                Appearance::Light,
                "MenuBar",
                &MaterialOptions {
                    rendition_key: Some("inactive.normal".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(r.rendition_key, "inactive.normal");
        assert!(l
            .recipe(
                Appearance::Light,
                "MenuBar",
                &MaterialOptions {
                    rendition_key: Some("nope".into()),
                    ..Default::default()
                }
            )
            .is_err());
        assert!(l
            .recipe(
                Appearance::Light,
                "NoSuchMaterial",
                &MaterialOptions::default()
            )
            .is_err());
    }

    #[test]
    fn the_switcher_panel_follows_the_dock_code_values() {
        let light = switcher_panel(Appearance::Light, false);
        assert_eq!(
            light.backdrop,
            Some(Backdrop {
                scale: 0.125,
                sigma_pt: 30.0,
                ops: vec![ColorOp::Saturate(1.8)]
            })
        );
        assert_eq!(light.fills[0].rgba, [0.9646, 0.9646, 0.9646, 0.36]);
        let dark = switcher_panel(Appearance::Dark, false);
        assert_eq!(
            dark.backdrop.clone().unwrap().ops,
            vec![ColorOp::Saturate(1.6)]
        );
        assert_eq!(dark.fills[0].rgba, [0.1569, 0.1569, 0.1569, 0.40]);
        assert_eq!(
            (dark.fills[1].tint_layer, dark.fills[1].rgba[3]),
            (true, 0.05)
        );
        assert_eq!(dark.corner_radius, Some(24.0));
    }

    #[test]
    fn the_switcher_panel_is_solid_under_reduce_transparency_and_increase_contrast() {
        let gray = |r: &Recipe| {
            assert!(
                r.opaque && r.backdrop.is_none() && r.fills.len() == 1,
                "{}",
                r.name
            );
            assert_eq!(r.corner_radius, Some(24.0));
            r.fills[0].rgba
        };
        assert_eq!(
            gray(&switcher_panel(Appearance::Light, true)),
            [0.8548, 0.8548, 0.8548, 1.0]
        );
        assert_eq!(
            gray(&switcher_panel(Appearance::Dark, true)),
            [0.1255, 0.1255, 0.1255, 1.0]
        );
        assert_eq!(
            gray(&switcher_panel(Appearance::LightHighContrast, false)),
            [0.7843, 0.7843, 0.7843, 1.0]
        );
        assert_eq!(
            gray(&switcher_panel(Appearance::DarkHighContrast, true)),
            [0.05882, 0.05882, 0.05882, 1.0]
        );
    }

    #[test]
    fn the_app_grid_background_is_a_gain_and_a_lift_as_a_fill() {
        let recipe = app_grid_background(Appearance::Dark, false);
        assert_eq!(
            recipe.backdrop,
            Some(Backdrop {
                scale: 0.125,
                sigma_pt: APP_GRID_SIGMA,
                ops: vec![ColorOp::Saturate(APP_GRID_SATURATION)]
            })
        );
        let [r, g, b, a] = recipe.fills[0].rgba;
        assert_eq!((r, g, b), (g, g, g));
        // a backdrop gray c comes out as c·0.36 + 0.188: white a mid gray, black a dark one
        for c in [0.0f32, 0.3, 1.0] {
            let out = c * (1.0 - a) + g * a;
            assert!((out - (c * 0.36 + 0.188)).abs() < 1e-3, "c = {c}: {out}");
        }
        assert_eq!(recipe.corner_radius, None);
        // the light appearance has the same chain with a light fill, under its dark labels
        let light = app_grid_background(Appearance::Light, false);
        assert_eq!(light.backdrop, recipe.backdrop);
        assert_eq!(light.fills[0].rgba[3], a);
        for reduce_transparency in [false, true] {
            for appearance in [Appearance::Light, Appearance::LightHighContrast] {
                let r = app_grid_background(appearance, reduce_transparency);
                let fills = |c| through_fills(&r, c);
                assert!(
                    fills(0.0) > 0.5,
                    "{appearance:?} {reduce_transparency}: light over black"
                );
            }
        }
    }

    /// What a recipe of fills only (no colour operation besides the backdrop's saturation) makes of backdrop gray `c`.
    fn through_fills(recipe: &Recipe, c: f32) -> f32 {
        recipe.fills.iter().fold(c, |d, f| {
            let (s, a) = (f.rgba[0], f.rgba[3]);
            let b = match f.blend {
                Blend::PlusLighter => (s + d).min(1.0),
                Blend::PlusDarker => (s + d - 1.0).max(0.0),
                _ => s,
            };
            d * (1.0 - a) + b * a
        })
    }

    #[test]
    fn the_app_grid_keeps_its_blur_and_changes_its_colours_with_the_display_options() {
        let reduced = app_grid_background(Appearance::Dark, true);
        assert!(reduced.backdrop.is_some(), "faint bands still show through");
        let (white, black) = (through_fills(&reduced, 1.0), through_fills(&reduced, 0.0));
        assert!(
            white < 0.1 && black > 0.07,
            "near black over anything: {black}..{white}"
        );
        assert_eq!(
            app_grid_background(Appearance::DarkHighContrast, true),
            reduced,
            "Reduce Transparency wins"
        );
        let contrast = app_grid_background(Appearance::DarkHighContrast, false);
        let saturation = |r: &Recipe| match r.backdrop.as_ref().unwrap().ops[..] {
            [ColorOp::Saturate(s)] => s,
            _ => panic!("one saturation"),
        };
        let normal = app_grid_background(Appearance::Dark, false);
        assert!(
            1.0 < saturation(&contrast) && saturation(&contrast) < saturation(&normal),
            "the bands stay, less vivid"
        );
        for c in [0.0f32, 0.5, 1.0] {
            let (out, rest) = (through_fills(&contrast, c), through_fills(&normal, c));
            assert!(out < rest, "darker than at rest: {out} {rest}");
        }
    }

    #[test]
    fn the_dock_with_reduce_transparency_is_the_wallpapers_colour_darkened_not_a_gray() {
        let dark = dock_reduced(Appearance::Dark);
        let backdrop = dark.backdrop.as_ref().unwrap();
        assert!(backdrop.sigma_pt >= 40.0, "one broad colour, no detail");
        assert!(matches!(backdrop.ops[..], [ColorOp::Saturate(s)] if s > 1.0));
        assert_eq!(dark.fills[0].rgba[..3], [0.0; 3]);
        assert!(through_fills(&dark, 1.0) < 0.2, "dark");
        assert_eq!(dock_reduced(Appearance::Light).fills[0].rgba[..3], [1.0; 3]);
    }

    #[test]
    fn the_running_indicator_lightens_what_is_behind_it_in_the_dark() {
        let dark = running_indicator(Appearance::Dark);
        assert!(dark.backdrop.is_none());
        // a body of (46, 73, 168) comes out as (171, 198, 255): every channel lifted, the brightest clipped
        let out = [46.0f32, 73.0, 168.0].map(|c| through_fills(&dark, c / 255.0) * 255.0);
        for (o, want) in out.iter().zip([171.0, 198.0, 255.0]) {
            assert!((o - want).abs() <= 1.5, "{out:?}");
        }
        let light = running_indicator(Appearance::Light);
        assert!((through_fills(&light, 0.8) - (0.8 - RUNNING_INDICATOR_LIFT)).abs() < 1e-6);
    }

    #[test]
    fn every_material_parses_in_every_state() {
        let l = lib();
        let mut count = 0;
        let cats: Vec<Catalog> = Appearance::ALL
            .iter()
            .map(|a| Catalog::Appearance(*a))
            .collect();
        for cat in cats {
            let names = l.materials(cat);
            assert!(!names.is_empty());
            for m in names {
                for window in [WindowState::Active, WindowState::Inactive] {
                    for interaction in [
                        Interaction::Normal,
                        Interaction::Rollover,
                        Interaction::Pressed,
                    ] {
                        let r = l
                            .recipe_in(
                                cat,
                                m,
                                &MaterialOptions {
                                    window,
                                    interaction,
                                    ..Default::default()
                                },
                            )
                            .unwrap_or_else(|e| panic!("{cat:?}/{m}: {e}"));
                        assert!(
                            r.backdrop.is_some() || !r.fills.is_empty() || !r.notes.is_empty(),
                            "{cat:?}/{m} {window:?} {interaction:?} produced an empty recipe"
                        );
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
                    let r = l
                        .recipe_in(Catalog::Appearance(a), m, &MaterialOptions::default())
                        .unwrap();
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
            let bez = m * m * m * v[0]
                + 3.0 * m * m * t * v[1]
                + 3.0 * m * t * t * v[2]
                + t * t * t * v[3];
            assert!((horner - bez).abs() < 1e-5);
        }
    }

    #[test]
    fn rejects_malformed_documents() {
        assert!(MaterialLibrary::from_str("{}").is_err());
        assert!(MaterialLibrary::from_str("[").is_err());
    }
}
