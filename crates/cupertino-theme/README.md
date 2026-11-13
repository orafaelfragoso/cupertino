# cupertino-theme

Typed access to Cupertino's theme data (`data/theme/`): design tokens, material recipes, animation presets, window shadows
and typography.

## Public API

| File | API |
|---|---|
| `tokens.json` | `DesignTokens`: named colours per `Appearance` (sRGB `f32` + alpha) and the system font sizes, metrics and tracking table |
| `materials.json` | `MaterialLibrary`/`Recipe`: backdrop filter chain + fill layers per window state and interaction, opaque variants, high-contrast catalogs |
| `glass` (code) | `GlassParams::new(GlassMode, short_side)`: the glass recipe of an appearance, style (`GlassStyle::{Regular, Clear, LargePanel, Bar, Dense, Module, Card, Track}`), Reduce Transparency, Increase Contrast and Reduce Motion, for a shape whose shorter side is `short_side` points; `foreground_matrix(dark)`, `adapts_foreground(side)`, `apply_matrix` for content on glass; `FaceTone`, `face_tone(luminance, previous)` (light above `FACE_TONE_THRESHOLD`, 0.179, where white and black text read alike, changing only `FACE_TONE_HYSTERESIS`, 0.03, past it), `foreground_matrix_for(dark, tone)` and `foreground_rgb(dark, tone)`, the colour of text on glass; `VIBRANT_PRIMARY`, `vibrant_secondary(dark)`, `VIBRANT_PLACEHOLDER` (1, about 0.55, 0.7), how opaque labels are drawn on glass, a secondary label at the opacity that shifts the label face by `SECONDARY_SHIFT_LIGHT` (−0.4) or `SECONDARY_SHIFT_DARK` (+0.3); `relative_luminance`, `contrast_ratio` |
| `control_sizes` (code) | `control_geometry(ControlFamily, ControlSize, compact)`: each control family's height, width, corners, label size and inset at the five sizes (`ControlSize::ASCENDING`: mini, small, regular, large, extra large), normal and compact; `switch_geometry(compact)`, `slider_geometry(size)` and `choice_mark(size, compact)` for the parts of a switch, a slider and a check box or radio button. It is the one table of control sizes: `metrics` keeps only `ControlSize`, `ShapeTokens` and `WindowShadowPreset`, and `tests/one-size-table.rs` checks that no crate reads a per-control height table beside it |
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
`light-high-contrast` has its own `window-rim` and `window-rim-2x`, [0,0,0,127] like `dark-high-contrast`'s
(approximated), so its rims are dark strokes instead of the light appearance's white one.
The high-contrast backgrounds keep the normal ones: the light window stays white, the dark window is [33,34,39] (a
shade above the normal one) and the dark control and text backgrounds are 23; none is lighter than its normal token
in light or darker in dark beyond that, and none is more transparent (tested).
`Appearance::of_session(dark, increase_contrast) is the appearance a shell process draws with, from the session's dark
flag and its Increase Contrast option.

### Material recipes

- **Variants.** Each material has six variants keyed `<window>.<interaction>` (`active.normal` … `inactive.pressed`), chosen
  by `WindowState` and `Interaction`, or explicitly by `MaterialOptions::rendition_key`. A variant the material does not
  draw differently names the variant it shares, and `Recipe::rendition_key` reports it.
- **Inactive windows.** The materials of a window's own body (`KEPT_WHEN_INACTIVE`: `Sidebar`, `InlineSidebar`,
  `Titlebar`, `SeamlessWindowTitlebar`, `Header`) draw their own inactive variant in an inactive window: an opaque base
  laid with the desktop tint and no backdrop, so the sidebar keeps the wallpaper's tint whatever lies behind the window
  and reads the same over a busy or a dark scene. The dark sidebar's base is grey 0.13 with the tint at 0.145, so a
  blue wallpaper leaves it a dark navy; the light one is grey 0.9412 with the tint at 0.075. Their `-Opaque` variants (Reduce Transparency) are drawn the same in
  an inactive window as in an active one, in every appearance and interaction, since the data's inactive opaque
  variants are darker than the window. What else an inactive window changes is its labels, its selection and its
  window buttons, through their own roles. Other materials (menus, selections, sheets) keep their inactive variants,
  and an explicit `rendition_key` still reads any variant of the data.
- **Reduce Transparency.** `reduce_transparency` selects the `-Opaque` variant (mapping). The high-contrast
  appearances only ship opaque variants and fall back to the normal-contrast catalog for materials they do not define.
- **Hidden state layer.** A few selection materials draw nothing in one state: an empty recipe with a note.
- **Code recipes.** Five sit beside the catalog, and `Theme::recipe` answers them by name:
  - `Switcher`, the app switcher's panel. Under Reduce Transparency it is the solid gray 0.8548 (light) /
    0.1255 (dark), under Increase Contrast 0.7843 / 0.05882, opaque with no backdrop. Increase
    Contrast wins when both are on (approximate).
  - `AppGrid`, the app grid's background (`app_grid_background`): the desktop at 1/8 scale blurred with σ 20 pt and
    saturated by 2.8, then a fill of gray 0.294 at opacity 0.64 (a gain of 0.36 and a lift of 0.188; gray 0.92 in the
    light appearance). The display options keep the blur and change the colours: Reduce Transparency covers it with
    gray 0.0784 (light 0.92) at 0.98, opaque, so only faint bands show; Increase Contrast lowers the saturation to 1.4
    and the fill to gray 0.17 (light 0.80); with both, Reduce Transparency's (approximate).
  - `Dock`, the Dock bar and its stack panel with Reduce Transparency (`dock_reduced`; otherwise they are glass): the
    desktop at 1/8 scale blurred with σ 40 pt and saturated by 3.2, covered by black (dark) or white (light) at 0.87,
    so the body is one deep colour of the wallpaper rather than a gray (approximate).
  - `RunningIndicator`, the Dock's running dot (`running_indicator`): no backdrop, gray 0.49 added to what lies behind
    it (plus-lighter) in the dark appearance, taken away (plus-darker) in the light one.
  - `WindowBackground`, a window's own background drawn by the compositor (`window_background`): the theme's
    `window-background` colour, copied opaque, so glass over it in the same window reads the window and not the
    desktop.
  - `TextBackground`, the colour behind a form's groups drawn the same way (`opaque_background`): the theme's
    `text-background` colour, copied opaque.
- **Glass** (`glass.rs`, code). `GlassParams` holds every value of the glass background pass in stage order: the
  blur (`GlassBlur`: four levels of radius `radius·2^i`, widest first, each narrower one laid over at its opacity, and
  a blur fill that darkens, lightens or covers), the face colour (`GlassFace`: a saturation about the luma of the encoded
  colour, then a white gain and per-channel limit on linear light, a luminance cap, a black lift and a fill), the refraction (`GlassRefraction`: strength in points
  and band along the edge; a pixel `t` of the band deep reads the face `amount · refraction_profile(t)` along the outward
  normal, 0.9·e^(−4.2t)·(1 − t⁴)), the bleed of the widest level (`GlassBleed`), the key and fill rim lights (`GlassHighlight`) and the
  drop and ring shadows (`GlassShadow`, at opacity 0 at rest). Some values follow the shape's size: for regular glass
  the refraction offset (−min(s/2, 60)) and band (min(s/4, 20)), the bleed band (0.35·min(s, 140)), the opacity of the
  two narrowest levels (0.4 + clamp(s − 64, 0, 76)/240) and, in the dark appearance, the luminance cap (0.6 at 60 pt
  down to 0.35 at 120 pt); for clear glass the offset (−min(0.65·s, 60)), band (min(0.36·s, 20)), blur radius
  (5 to 10 between 48 and 120 pt) and shadows. Each accessibility option selects its own recipe rather than turning
  the glass off: Reduce Transparency removes the bending at the edge, widens the blur (40.6 pt, 200 for clear glass)
  and lays a dense fill over it; Increase Contrast keeps the optics with firmer colours; both together use the
  reduced-transparency optics with the increased-contrast colours; Reduce Motion drops the refraction band and widens the
  narrowest blur. `GlassParams::capture_scale` is the scale the backdrop is read at: `GLASS_BACKDROP_SCALE` (0.5),
  `CARD_CAPTURE_SCALE` (0.25) for cards, `TRACK_CAPTURE_SCALE` (1) for tracks and 0.25 (0.125 once its blur is widest)
  for dense glass. Content on glass is coloured by a
  4×5 matrix per appearance; `adapts_foreground` tells which controls (shorter side under 64 pt) take it. The bar
  (`GlassStyle::Bar`, the dock) blurs with a radius of 1 pt, bends 8.64 over a 3.84 pt band, and its face takes
  saturation 1.25, black 0.15 and white 0.9 (light) or black 0.05, white 0.92, a luma cap of 0.75 and a faint black veil
  (dark), with a thin line of light along its top and bottom edges over a wide faint glow. Popover glass (`GlassStyle::Popover`, a floating bar's names and menus) has no blur of its own, only an
  8 pt blur fill, bends 24 over a 5 pt band and lights its top and bottom as the bar does. Switcher glass (`GlassStyle::Switcher`, the application switcher's panel)
  blurs with a radius of 0.96 pt at full opacity and no blur fill, bends 41.6 over a 9.6 pt band at every size with a rim
  of 0.4 (bias −0.5) and no ring or drop shadow; its face takes saturation 1.2, black 0.25, white 1.06, a luma cap of
  0.85 and a clamp of 1.142 (light) or saturation 0.8, black 0.05 and white 0.3 (dark). Sidebar glass
  (`GlassStyle::Sidebar`, a window's sidebar) blurs with a radius of 5 under an 8 pt blur fill (lighten or darken 0.882,
  normal 0.48), bends 24 over 12 pt, has a ring shadow of 0.06 and a face of saturation 1.2, black 0.4, white 1.03, a
  luma cap of 0.85 and a white veil of 0.2 (light) or saturation 0.6, black 0.1 and white 0.5 (dark). Subdued module
  glass (`GlassStyle::SubduedModule`) is module glass with twice the blur, white 0.88 (light) and black 0 (dark).
  Snippet glass (`GlassStyle::Snippet`) has a faint blur (the first level at 0.2666) under the 8 pt blur fill, bends 24
  over 12 pt and veils its face white at 0.5 (light) or black at 0.4 (dark). Veiled glass (`GlassStyle::Veiled`) is the
  clear face under the blur fill, bending 34.56 over 19.2 pt, and `GlassParams::veil` darkens it with black from 0.2 at
  the top of its shapes to 1 at their bottom. Toolbar glass (`GlassStyle::Toolbar`, a window's toolbar controls) is the
  regular recipe of its size with black 0.11 (dark) or white 0.99 (light), no rim and no edge line, and a line of light
  0.45 pt wide a quarter point inside its edge (strength 0.7 dark, 0.25 light) on arcs 140° wide about the top, half as
  strong at the bottom, over a glow eight times as wide at 0.12 of its strength. Dense glass (`GlassStyle::Dense`, menus and popovers) is
  the regular recipe of its size with its own blur (radius 6 pt up to a short side of 60, 12 pt from 120, the first
  level at full opacity), bending by 0.4 of its short side over 0.2 of it (at most 60 and 20) and a ring shadow of 0.06,
  over a face that washes the backdrop out: black lifted by 0.8 under a faint light grey veil (light), saturation 1.3,
  black 0.15 and white 0.3 with the regular glass's luma cap (dark). Module glass (`GlassStyle::Module`, the control panel's
  tiles) is clear glass of its size blurring with a radius of 5 pt up to a short side of 60 and 10 pt from 140, bending by 80 over a band of 0.18 of
  its short side (at most 25 pt) with a rim
  bias of −0.3: white comes out at 244 in the light appearance and 207 in the dark one. Card glass
  (`GlassStyle::Card`, notification cards) is the clear face over a blur of radius 10 and the regular blur fill,
  bending by 60 over 20 pt, with a ring shadow of 0.06 at rest, read at a quarter scale. Track glass
  (`GlassStyle::Track`, the control panel's levels) has no blur, bends by 0.3 of its height over 0.18 of it, lifts black
  by 0.1 and has a ring shadow of 0.15 (light) or 0.4 (dark), read at the output's scale. The large
  panel (`GlassStyle::LargePanel`) is regular glass with a blur twice as wide (`LARGE_PANEL_BLUR_SCALE`: levels from 10
  pt, the blur fill 16 pt) and its tones pressed together, so a launcher-sized sheet reads as one calm surface over any
  wallpaper, stripes and text in the scene melting into broad fields of colour: in the dark appearance a luminance cap
  of 0.102, a black lift of 0.15 and a saturation of 1.15 (white comes out near 125, black near 53, a strong red keeps
  its colour), in the light one a cap of 0.94, a lift of 0.5 and 1.1 (white near 253, mid tones as before, black near 153); and a drop shadow of its own (28 pt blur, 16 pt below, black at 0.5 dark or 0.22
  light), drawn by the compositor outside the shape. With Reduce Transparency or Increase Contrast it takes regular
  glass's colours; Reduce Transparency also takes regular glass's whole blur. `GlassParams::shadow_reach` is how far the shadow reaches past the shape.
  `GlassShadow::edge_line` is the opacity of the dark line one pixel wide just outside the outline (0.2 light, 0.22 dark
  for regular and large panel glass; 0.31 / 0.54 dense; 0.22 / 0.24 track; 0.175 for light popover glass, 0.19 for a
  light card, 0.2 / 0.32 switcher, 0.2 / 0.4 sidebar, 0.1 for light snippet glass; none on clear, bar and module glass), scaled by how bright the backdrop is there.
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

### M1 control colour data

The ordinary `light` and `dark` catalogs include explicit M1 chrome state colours. Existing entries are unchanged.
The following table records the design basis of every added value. All new state assignments and literals are
approximated. They await the control fixture review. RGBA values are straight 8-bit sRGB.

| Tokens | Light | Dark | Design basis |
|---|---|---|---|
| `control-fill`, `control-fill-hover` | `control` | `control` | Existing fill, retained on hover |
| `control-fill-pressed` | `secondary-selected-control` | `secondary-selected-control` | Neutral momentary press, paired with `control-text` |
| `control-fill-disabled`, `default-button-fill-disabled`, `destructive-fill-disabled`, `text-background-disabled` | [242,242,242,255] | [62,62,62,255] | approximated dedicated disabled fill without subtree opacity |
| `control-border` | `container-border` | `container-border` | Existing border |
| `control-border-emphasized` | [0,0,0,96] | [255,255,255,96] | approximated hover and pressed border |
| `control-border-disabled` | [0,0,0,32] | [255,255,255,32] | approximated dedicated disabled border |
| `default-button-fill`, `default-button-fill-hover`, `selected-control`, `switch-track-selected`, `switch-track-selected-hover` | `control-accent-blue` | `control-accent-blue` | Existing accent, retained on hover |
| `default-button-fill-pressed`, `switch-track-selected-pressed` | [0,104,218,255] | [0,104,218,255] | approximated explicit pressed accent |
| `default-button-text`, `destructive-text`, `selected-control-text` | [255,255,255,255] | [255,255,255,255] | Light uses existing alternate selection foreground; dark completion approximated |
| `alternate-selected-control-text` (dark only) | Existing entry | [255,255,255,255] | approximated missing dark selection foreground |
| `default-button-text-disabled`, `destructive-text-disabled` | `disabled-control-text` | `disabled-control-text` | Existing disabled foreground |
| `destructive-fill`, `destructive-fill-hover` | `system-red` | `system-red` | Existing destructive colour, retained on hover |
| `destructive-fill-pressed` | [215,45,38,255] | [220,55,46,255] | approximated explicit destructive press |
| `switch-track`, `switch-track-hover` | `system-fill` | `system-fill` | Existing off treatment |
| `switch-track-pressed` | [0,0,0,51] | [255,255,255,51] | approximated off press |
| `switch-track-disabled` | [220,220,220,255] | [70,70,70,255] | Existing secondary selection RGB, opaque disabled off |
| `switch-track-selected-disabled` | [166,198,232,255] | [54,82,112,255] | approximated disabled on treatment; thumb position also conveys state |
| `switch-thumb`, `switch-thumb-hover`, `switch-thumb-pressed` | [255,255,255,255] | [255,255,255,255] | approximated opaque thumb; dark `control` is translucent and unsuitable |
| `switch-thumb-disabled` | [248,248,248,255] | [180,180,180,255] | approximated dedicated disabled thumb |
| `secondary-selected-control-text` | `control-text` | `control-text` | Existing foreground for neutral selection |
| `selected-text` | `text` | `text` | Existing foreground over translucent text selection |
| `control-focus-indicator` | `keyboard-focus-indicator` | [26,169,255,127] (dark) | Focus ring colour, half transparent in both appearances |
| `tool-tip` (dark only) | Existing entry | [40,40,40,255] | approximated opaque completion of the dark tooltip pair |

Names in the Light and Dark columns mean exact copies of that appearance's existing values, not runtime aliases.
Disabled overrides pressed and hover. Selection is independent of interaction. Active selected controls use
`selected-control` with `selected-control-text`; inactive selected controls use `secondary-selected-control` with
`secondary-selected-control-text`. Switch selection retains its on colour when the window is inactive. Keyboard
focus paints a separate ring. No runtime colour darkening or whole-control opacity is needed.

These additions provide ordinary appearance data for the chrome fixture. They do not implement the Kit adapter or
the neutral built-in theme, and do not complete the high-contrast catalogs. White text on the inherited accent and
red, disabled foregrounds, and faint boundaries still require resolved-pair contrast review. A label-free chrome
capture cannot establish text contrast or complete accessibility acceptance.

```
cargo test -p cupertino-theme
```

The tests run against the theme data in `data/theme/`.
