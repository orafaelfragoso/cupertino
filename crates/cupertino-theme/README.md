# cupertino-theme

Typed access to Cupertino's theme data (`data/theme/`): design tokens, material recipes, animation presets, window shadows
and typography.

## Public API

| File | API |
|---|---|
| `tokens.json` | `DesignTokens`: named colours per `Appearance` (sRGB `f32` + alpha) and the system font sizes, metrics and tracking table |
| `materials.json` | `MaterialLibrary`/`Recipe`: backdrop filter chain + fill layers per window state and interaction, opaque variants, high-contrast catalogs |
| `animations.json` | `AnimationPresets`: springs, timing functions, momentum, rubber band and dock timings as `cupertino-animation` types; `Easing` (bézier timing functions plus the cosine ease, which is not a bézier) via `AnimationPresets::easing(name)`; the dock auto-hide curve is `Easing::CosineEase` with the 0.15 s delay / 0.24 s duration; an unknown curve name is an error, not a silent fallback |


`Theme::load()` reads `$CUPERTINO_SPEC_DIR` (default `../../data/theme`); `theme.tokens.semantic(Appearance::Dark,
SemanticColor::WindowBackground)` and `theme.recipe(Appearance::Light, "Sidebar", &MaterialOptions::default())` look values
up. Every loader also has `from_str` (and `Theme::from_strs`).

## Usage

```rust
use cupertino_theme::{Appearance, MaterialOptions, SemanticColor, Theme};

let theme = Theme::load()?; // $CUPERTINO_SPEC_DIR, default ../../data/theme
let bg = theme.tokens.semantic(Appearance::Dark, SemanticColor::WindowBackground);
let recipe = theme.recipe(Appearance::Light, "Sidebar", &MaterialOptions::default())?;
```

## How it works

### Appearances

`Appearance::{Light, Dark, LightHighContrast, DarkHighContrast}`, keyed `light`, `dark`, `light-high-contrast` and
`dark-high-contrast` in the data. Colour names are kebab-case (`window-background`, `control-accent-blue`). High-contrast
colour catalogs are partial, so missing names fall back to the matching normal-contrast appearance.
`Appearance::of_session(dark, increase_contrast) is the appearance a shell process draws with, from the session's dark
flag and its Increase Contrast option.

### Material recipes

- **Variants.** Each material has six variants keyed `<window>.<interaction>` (`active.normal` … `inactive.pressed`), chosen
  by `WindowState` and `Interaction`, or explicitly by `MaterialOptions::rendition_key`. A variant the material does not
  draw differently names the variant it shares, and `Recipe::rendition_key` reports it.
- **Reduce Transparency.** `reduce_transparency` selects the `-Opaque` variant (mapping). The high-contrast
  appearances only ship opaque variants and fall back to the normal-contrast catalog for materials they do not define.
- **Hidden state layer.** A few selection materials draw nothing in one state: an empty recipe with a note.
- **Code recipes.** Two sit beside the catalog, and `Theme::recipe` answers them by name:
  - `Switcher`, the app switcher's panel. Under Reduce Transparency it is the solid gray 0.8548 (light) /
    0.1255 (dark), under Increase Contrast 0.7843 / 0.05882, opaque with no backdrop. Increase
    Contrast wins when both are on (approximate).
  - `AppGrid`, the app grid's background: blur 30, saturate 1.8, and the gain 0.55 / bias 0.05292 colour matrix
    as a fill of gray 0.1176 at opacity 0.45.
- **Display filter steps.** `ColorOp::{LinearMatrix, Invert, Contrast}` feed `cupertino-glass::display_filter`. No catalog
  recipe uses them.

### Window shadows and corner radii

`window_shadow` holds the complete window shadow parameter table as typed data (`WINDOW_SHADOW_TABLE`, 32 rows: window
class × active/inactive × light/dark × contrast variant; density, Gaussian σ in points, vertical offset, clip-top, rim
terms). It is as designed and tested against the raw extraction table.

- `window_shadow(option, active, dark, increased_contrast) selects a row. The contrast half starts at table offset 0x500
  under Increase Contrast.
- `sigma_to_radius_px` is the verified `ceil(σ · 2.8)`. `rim_radius` is the verified 2/3 rule.
- Rows of the standard class: active σ 20 / offset 18 / density 0.40, inactive σ 12 / 8 / 0.25, dark active density 0.70,
  dark inactive 0.45.
- Which window gets which class at run time is unknown. The `Other` class is to be sheets.

Window corner radii are `WindowCornerSize::{Regular = 12 pt, Small = 10 pt} (supersedes the
8-9 pt estimate). `ShapeTokens` and `WindowShadowPreset` derive from these values. The contact shadow of the preset remains
approximate.

### Typography

The system font substitute is `Inter`. The crate holds size presets (13/11/9/10 pt), the `trak` tracking table with linear interpolation by
size and track, and line metrics. A test checks the built-in table against the one loaded from the theme data.

## Tests

```
cargo test -p cupertino-theme
```

The tests run against the theme data in `data/theme/`.
