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

/// Name of the overview's desktops band material, a code recipe like the switcher's (see [`overview_band`]).
pub const OVERVIEW_BAND_MATERIAL: &str = "OverviewBand";

/// The overview's desktops band across the top of the output: the ultra-thin material the switcher's panel is made of,
/// with nothing of the wallpaper's tint and square ends. The desktop behind it captured at 1/8 scale, saturated by
/// [`OVERVIEW_BAND_SATURATION`] and blurred with σ [`OVERVIEW_BAND_SIGMA`], under a fill of
/// [`OVERVIEW_BAND_GRAY`] at [`OVERVIEW_BAND_COVER`]; the design's brightness (−0.01 light, +0.015 dark) has no recipe
/// operation and is left out, as for the switcher.
///
/// A high-contrast appearance and `reduce_transparency` select the solid styles of the same material, as the
/// switcher's: gray 0.7843 (light) or 0.05882 (dark) with Increase Contrast, gray 0.8548 or 0.1255 with Reduce
/// Transparency. That the band takes them is approximate: what the display options do to it was not established.
pub fn overview_band(appearance: Appearance, reduce_transparency: bool) -> Recipe {
    let dark = appearance.is_dark();
    let at = |pair: (f32, f32)| if dark { pair.1 } else { pair.0 };
    let solid = |gray: f32, style: &str| Recipe {
        name: format!("{OVERVIEW_BAND_MATERIAL}-{style}"),
        material: OVERVIEW_BAND_MATERIAL.into(),
        fills: vec![Fill {
            rgba: [gray, gray, gray, 1.0],
            blend: Blend::Normal,
            tint_layer: false,
        }],
        opaque: true,
        ..Recipe::default()
    };
    if appearance.is_high_contrast() {
        return solid(at(OVERVIEW_BAND_CONTRAST_GRAY), "IncreaseContrast");
    }
    if reduce_transparency {
        return solid(at(OVERVIEW_BAND_REDUCED_GRAY), "ReduceTransparency");
    }
    let gray = at(OVERVIEW_BAND_GRAY);
    Recipe {
        name: OVERVIEW_BAND_MATERIAL.into(),
        material: OVERVIEW_BAND_MATERIAL.into(),
        backdrop: Some(Backdrop {
            scale: 0.125,
            sigma_pt: OVERVIEW_BAND_SIGMA,
            ops: vec![ColorOp::Saturate(at(OVERVIEW_BAND_SATURATION))],
        }),
        fills: vec![Fill {
            rgba: [gray, gray, gray, at(OVERVIEW_BAND_COVER)],
            blend: Blend::Normal,
            tint_layer: false,
        }],
        notes: vec!["colorBrightness has no recipe operation and is left out".into()],
        ..Recipe::default()
    }
}

/// The overview band's blur in points.
pub const OVERVIEW_BAND_SIGMA: f32 = 30.0;
/// How much the band saturates the desktop behind it: (light, dark).
pub const OVERVIEW_BAND_SATURATION: (f32, f32) = (1.8, 1.6);
/// The gray laid over the band's blur and how much of it covers the blur: (light, dark).
pub const OVERVIEW_BAND_GRAY: (f32, f32) = (0.9646, 0.1569);
pub const OVERVIEW_BAND_COVER: (f32, f32) = (0.36, 0.40);
/// The band's solid gray with Increase Contrast and with Reduce Transparency: (light, dark).
pub const OVERVIEW_BAND_CONTRAST_GRAY: (f32, f32) = (0.7843, 0.05882);
pub const OVERVIEW_BAND_REDUCED_GRAY: (f32, f32) = (0.8548, 0.1255);

/// Name of the material of the pill under the current desktop's name in the overview's band, a code recipe (see
/// [`overview_band_selection`]).
pub const OVERVIEW_BAND_SELECTION_MATERIAL: &str = "OverviewBandSelection";

/// The pill under the current desktop's name in the overview's band: no backdrop of its own, the gray
/// [`OVERVIEW_BAND_SELECTION_DARK`] added to the band under it (plus-lighter) in the dark appearance, and in the light
/// one the gray [`OVERVIEW_BAND_SELECTION_LIGHT`] laid plus-darker, which takes its distance from white away.
pub fn overview_band_selection(appearance: Appearance) -> Recipe {
    let (gray, blend) = if appearance.is_dark() {
        (OVERVIEW_BAND_SELECTION_DARK, Blend::PlusLighter)
    } else {
        (OVERVIEW_BAND_SELECTION_LIGHT, Blend::PlusDarker)
    };
    Recipe {
        name: OVERVIEW_BAND_SELECTION_MATERIAL.into(),
        material: OVERVIEW_BAND_SELECTION_MATERIAL.into(),
        fills: vec![Fill {
            rgba: [gray, gray, gray, 1.0],
            blend,
            tint_layer: false,
        }],
        ..Recipe::default()
    }
}

/// The gray of the current desktop's pill: light (plus-darker) and dark (plus-lighter).
pub const OVERVIEW_BAND_SELECTION_LIGHT: f32 = 0.9098;
pub const OVERVIEW_BAND_SELECTION_DARK: f32 = 0.1373;

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

/// The Dock's running indicator: no backdrop of its own, a gray laid over what lies behind it: in the dark appearance
/// [`RUNNING_INDICATOR_DARK`] added to it (plus-lighter), so the dot is a lighter, brighter version of the glass under
/// it; in the light appearance [`RUNNING_INDICATOR_LIGHT`] taken away (plus-darker).
pub fn running_indicator(appearance: Appearance) -> Recipe {
    plus_gray(
        RUNNING_INDICATOR_MATERIAL,
        appearance,
        RUNNING_INDICATOR_LIGHT,
        RUNNING_INDICATOR_DARK,
    )
}

/// Gray of the running indicator in the light appearance, laid plus-darker: the result is the backdrop less 0.498.
pub const RUNNING_INDICATOR_LIGHT: f32 = 0.502;
/// Gray of the running indicator in the dark appearance, laid plus-lighter: the backdrop plus 0.486.
pub const RUNNING_INDICATOR_DARK: f32 = 0.486;

/// Name of the Dock's separator material, a code recipe (see [`dock_separator`]).
pub const DOCK_SEPARATOR_MATERIAL: &str = "DockSeparator";

/// The line between the Dock's tiles: no backdrop of its own, [`DOCK_SEPARATOR_DARK`] added to what lies behind it
/// (plus-lighter) in the dark appearance and [`DOCK_SEPARATOR_LIGHT`] taken away (plus-darker) in the light one.
pub fn dock_separator(appearance: Appearance) -> Recipe {
    plus_gray(
        DOCK_SEPARATOR_MATERIAL,
        appearance,
        DOCK_SEPARATOR_LIGHT,
        DOCK_SEPARATOR_DARK,
    )
}

/// Gray of the Dock's separator in the light appearance, laid plus-darker: the backdrop less 0.224.
pub const DOCK_SEPARATOR_LIGHT: f32 = 0.776;
/// Gray of the Dock's separator in the dark appearance, laid plus-lighter: the backdrop plus 0.255.
pub const DOCK_SEPARATOR_DARK: f32 = 0.255;

/// A recipe of one opaque gray with no backdrop, plus-lighter in the dark appearance and plus-darker in the light.
fn plus_gray(name: &str, appearance: Appearance, light: f32, dark: f32) -> Recipe {
    let (gray, blend) = if appearance.is_dark() {
        (dark, Blend::PlusLighter)
    } else {
        (light, Blend::PlusDarker)
    };
    Recipe {
        name: name.into(),
        material: name.into(),
        fills: vec![Fill {
            rgba: [gray, gray, gray, 1.0],
            blend,
            tint_layer: false,
        }],
        ..Recipe::default()
    }
}

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
