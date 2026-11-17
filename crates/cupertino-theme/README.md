# cupertino-theme

Typed access to Cupertino's theme data (`data/theme/`): design tokens, material recipes, animation presets, the timing
function table, window shadows and typography; and the toolkit's theme tokens (`ui`, [below](#the-toolkits-theme-tokens-ui)).
The crate parses the documents it is given and reads no files: `cupertino-assets` finds the theme data and loads a
`Theme` from it.

## Public API

| File | API |
|---|---|
| `tokens.json` | `DesignTokens`: named colours per `Appearance` (sRGB `f32` + alpha) and the system font sizes, metrics and tracking table |
| `materials.json` | `MaterialLibrary`/`Recipe`: backdrop filter chain + fill layers per window state and interaction, opaque variants, high-contrast catalogs |
| `glass` (code) | `GlassParams::new(GlassMode, short_side)`: the glass recipe of an appearance, style (`GlassStyle::{Regular, Clear, LargePanel, Bar, Dense, Module, Card, Track}`), Reduce Transparency, Increase Contrast and Reduce Motion, for a shape whose shorter side is `short_side` points; `foreground_matrix(dark)`, `adapts_foreground(side)`, `apply_matrix` for content on glass; `FaceTone`, `face_tone(luminance, previous)` (light above `FACE_TONE_THRESHOLD`, 0.179, where white and black text read alike, changing only `FACE_TONE_HYSTERESIS`, 0.03, past it), `foreground_matrix_for(dark, tone)` and `foreground_rgb(dark, tone)`, the colour of text on glass; `VIBRANT_PRIMARY`, `vibrant_secondary(dark)`, `VIBRANT_PLACEHOLDER` (1, about 0.55, 0.7), how opaque labels are drawn on glass, a secondary label at the opacity that shifts the label face by `SECONDARY_SHIFT_LIGHT` (−0.4) or `SECONDARY_SHIFT_DARK` (+0.3); `relative_luminance`, `contrast_ratio` |
| `control_sizes` (code) | `control_geometry(ControlFamily, ControlSize, compact)`: each control family's height, width, corners, label size and inset at the five sizes (`ControlSize::ASCENDING`: mini, small, regular, large, extra large), normal and compact; `switch_geometry(compact)`, `slider_geometry(size)` and `choice_mark(size, compact)` for the parts of a switch, a slider and a check box or radio button. It is the one table of control sizes: `metrics` keeps only `ControlSize`, `ShapeTokens` and `WindowShadowPreset` |
| `animations.json` | `AnimationPresets`: springs, timing functions, momentum, rubber band and dock timings as `cupertino-animation` types; `Easing` (bézier timing functions plus the cosine ease, which is not a bézier) via `AnimationPresets::easing(name)`; the dock auto-hide curve is `Easing::CosineEase` with the 0.15 s delay / 0.24 s duration; an unknown curve name is an error, not a silent fallback |
| `animation` (code) | `TimingPreset`: the timing-function table, five béziers (`ease`, `linear`, `ease-in`, `ease-out`, `ease-in-out`) and one spring (mass 1, stiffness 100, damping 10), by index or name, as a bézier, a spring, a sampled progress or a `cupertino-animation` `Curve` |
| `ui` (code) | the toolkit's theme tokens: `ThemeTokens`, `ColorTokens`, `ColorRole`, `StateVariant`, `ControlMetrics`, `TypeScale`, `FocusRing`, `Motion`, `Surface` ([below](#the-toolkits-theme-tokens-ui)) |


`Theme::from_strs(tokens, materials, animations)` parses the three documents; `theme.tokens.semantic(Appearance::Dark,
SemanticColor::WindowBackground)` and `theme.recipe(Appearance::Light, "Sidebar", &MaterialOptions::default())` look values
up. Every part also has `from_str`. `cupertino_assets::theme::load()` reads the files from `$CUPERTINO_SPEC_DIR` (else
the directory built in) and `cupertino_assets::theme::from_dir(dir)` from a given directory.

## Usage

```rust
use cupertino_theme::{Appearance, MaterialOptions, SemanticColor};

let theme = cupertino_assets::theme::load()?; // $CUPERTINO_SPEC_DIR, else the directory built in
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
size and track, and line metrics.

## The toolkit's theme tokens (`ui`)

`cupertino_theme::ui` holds the CupertinoKit theme tokens: semantic colours per `ColorRole` and `StateVariant`, control
metrics, type scale, focus ring and motion. It reuses `Color`, `Appearance`, `ControlSize` and `DesignTokens` of the
crate root.

### Colours

A theme is one colour per role plus a few state colours (emphasized border, pressed button fills, switch tracks and
the disabled thumb). Resolution expands them into every role in all 16 states, so `ColorTokens::get` never falls back
to a normal colour for a selected or disabled control. Selection applies first, then hover or press; disabled overrides
both.

A table also carries how far a window that just became active has crossed from its inactive colours to its active ones
(`ColorTokens::set_activation`, 1 by default): below 1, `get` of an active state blends from the inactive one. The
runtime sets it on the theme it paints such a window with.

- `ColorTokens::cupertino(tokens, appearance)`: light and dark name tokens of `data/theme/tokens.json` in that
  appearance only. A missing token is an error, never a colour from another appearance.
- `ColorTokens::neutral(appearance)`: built in, for any compositor, with no theme data, pack or desktop service. Every
  colour is opaque. Light and dark are the Cupertino colours flattened onto the surface they are drawn on (a test keeps
  them in step with the token data), except where that falls short: text is raised to 4.5:1 and control and field
  borders and the focus ring to 3:1 against their surface in every enabled state. Disabled text recedes, as the
  token data's does: grey 192 on white, [86,87,91] on dark, at least 1.4:1 against its surface and 2.5:1 apart from
  enabled text. The accent and red fills are darker, so white text on them reaches 4.5:1.
- High contrast is built in and used by both: opaque, text at least 4.5:1 and borders, tracks, thumbs, separators and
  the focus ring at least 3:1 against their surface in every enabled state (tested per resolved pair). Disabled text
  reads at 3:1 there (grey 150 in dark) and still stands 2.5:1 apart from enabled text. The backgrounds keep the
  normal ones: a white window in light, [33,34,39] and a text background of 23 in dark. Hover and press
  turn borders black (light) or white (dark). The dark switch thumb is dark, so it stands out on both tracks, and it
  stands 3:1 against the control border and the panel track, so a slider's knob shows against its ring and its track.

Slider tracks. The empty part of a slider's track has a role of its own (`SliderTrack`), so it is not the switch
track; its disabled colour is the disabled switch track's. It and a panel's level slider track (`PanelTrack`) stand 3:1
or more against the surface they sit on in every palette, light and dark, as is, with Increase Contrast and with
Reduce Transparency, in every enabled state: `SliderTrack` against the window and a text background (a mid grey, 128,
in light and dark; 118 and 150 in high contrast), `PanelTrack` against a module, the group fill over the window
background (118 in light, 145 in dark; 110 and 165 in high contrast). The thumb colour, which is a slider's knob and a
panel slider's fill, stands 3:1 against both tracks. A disabled `PanelTrack` has its own colour, the enabled track faded towards the module (185 in light, 95 in dark; 160 and 110 in high contrast): still apart from the module, fainter than the enabled track.

Menus, popovers and the Dock (`Surface::Menu`, `Surface::Popover`, `Surface::Dock`) are glass (`GLASS_MATERIAL`, `CLEAR_GLASS_MATERIAL`): `ThemeTokens::surface` keeps asking for it with Reduce Transparency and in high contrast, where the compositor draws the glass recipe of those options; the Dock asks for its own `Dock` body with Reduce Transparency. The application grid (`Surface::AppGrid`) keeps its `AppGrid` material too, whose recipe has variants of its own for those options, and a window's background drawn by the compositor (`Surface::WindowBackground`, `WindowBackground`) is opaque, so it is kept as well. The Dock's running dot (`Surface::DockIndicator`) asks for `RunningIndicator`, which lightens what lies behind it, in the dark appearance only (not in high contrast); elsewhere it asks for nothing and the dot paints itself. The other surfaces ask for no material with Reduce Transparency or in high contrast and paint their opaque fill.

`ThemeTokens::on_glass(tone)` is the theme of what a window draws on glass whose face the compositor measured as `tone`
(`None` until it did, and for shapes too large to follow what lies behind them): the neutral colours of `Text`,
`ControlText` and `Header`, `SecondaryText` and `PlaceholderText` take `glass_foreground` (the foreground matrix of the
tone, or the appearance's) at the vibrant levels of `vibrant_level` (1, 0.75, 0.7), or more where the label was
stronger; an opaque grey label of the high-contrast and reduced-transparency themes counts as the appearance's
foreground at the opacity of how far it lies towards it; a disabled label keeps its own strength, and a selected label (drawn on its selection's fill), accents and fills keep their colours. The same holds in
every accessibility mode.

The overlay scroller thumb (`ScrollerThumb`) is translucent in Cupertino light and dark and opaque in the built-in
palettes, at 3:1 or more against the window there. The dialog scrim (`DialogScrim`) is the one colour that is
translucent in every palette, since it dims what it covers: 20 % black in light, 40 % in dark, 60 % in high-contrast
dark.

Selection. The selected fills of every control (a sidebar's, a list's, a table's or an outline's selected row in an
active window, a menu's highlighted item, the default button, an on switch) are the theme's accent token itself, on a
window and on glass: no role lightens, fades, saturates or tints it. A sidebar's neutral
selection (`SidebarSelection`) is a grey laid over the sidebar's material, so it lifts each of the material's channels
evenly and keeps its hue: white at 34/255 in dark (each channel of a navy material up by about 29), black at 25/255 in
light. A file browser's places draw their selected row with it in an active window, and every sidebar draws it for its
selected row in a window that is not active, where it resolves fainter: white at 16/255 in dark, black at 13/255 in
light. The built-in palettes flatten it onto the window background, opaque, with an active sidebar's labels at 4.5:1 on it;
an inactive window's secondary labels read at 4:1 or better on its fainter selection (the neutral light one, 224, a
little darker than the opaque sidebar it lies on, so a little under 4.5:1 there, as on the sidebar itself).

Inactive windows. Three roles carry what an inactive window's chrome recedes to: `InactiveSidebarText`, a sidebar's
labels, symbols and headings (white at 89/255 in dark, black at 102/255 in light, so the sidebar's tint still shows
around them), `TertiaryText`, the toolbar's title and glyphs (the tertiary label token), and `InactiveTitlebar`, the
solid band under an inactive window's toolbar row ([38,39,44] dark, 247 light). The built-in palettes give opaque
greys that read at 4:1 or better on the sidebar's inactive selection and 4.5:1 on the band.

Links (`Link`) take the theme's link token in light and dark and reach 4.5:1 against the window and text backgrounds in
the built-in palettes. A page indicator dot for a page not shown (`PageDot`) is text at 20 % in light and dark and an
opaque grey at 3:1 or more in the built-in palettes; the shown page's dot is `Text`.

`ThemeTokens::surface` gives each surface the material to ask the compositor for (`SidebarGlass` for a sidebar, `MenuGlass` for menus,
`DenseGlass` for popovers, `Glass` for the Dock, `LoginWindowControl` for a heads-up display, `AppGrid` for the application grid's panel,
`SwitcherGlass` for the application switcher's panel, `RunningIndicator` for the Dock's running dot, `WindowBackground`
for a window's background and `TextBackground` for the colour behind a form's groups) and the opaque fill to paint
where there is none (the text background colour for `TextBackground`, the window background for the others). With reduced transparency or
in high contrast it asks for no material, except for the surfaces above that keep theirs.

### Material inventory

Every component of the toolkit and of the shell that lies on a material the compositor draws, with the call that asks
for it and what becomes of it with Reduce Transparency and with Increase Contrast. A `Glass::…` call or a `GlassStyle`
is glass of that style (`GlassParams` of `glass`); a `Surface::…` is the theme surface whose material
`ThemeTokens::surface` names; a quoted name is a recipe of `materials.json`.

- **kept**: the component asks for the same material and the compositor draws that material's recipe of the option. For
  glass that is, with Reduce Transparency, a wide blur under a nearly opaque face with nothing bent at the edge, and
  with Increase Contrast the same optics under the contrast face; for a recipe of `materials.json`, its opaque variant
  and the high-contrast catalog's.
- **kept in dark**: kept in the dark appearance, not drawn in the light one.
- **opaque**: the component asks for no material and paints the theme's opaque window colour in the same shape.
- **painted**: the component asks for no material and paints an opaque body of its own.

| Component | Drawn in | Draws | Reduce Transparency | Increase Contrast |
|---|---|---|---|---|
| Menu (pop-up, context and bar menus) | `crates/cupertino-ui-controls/src/menu.rs` | `Surface::Menu` | kept | kept |
| Menu with a pointer | `crates/cupertino-ui-controls/src/menu.rs` | `Glass::menu()` | kept | kept |
| Popover | `crates/cupertino-ui-controls/src/popover.rs` | `Surface::Popover` | kept | kept |
| Popover with a pointer | `crates/cupertino-ui-controls/src/popover.rs` | `Glass::dense()` | kept | kept |
| Alert | `crates/cupertino-ui-controls/src/dialog.rs` | `Glass::dense().radius(ALERT_RADIUS)` | painted | painted |
| Toolbar controls | `crates/cupertino-ui-controls/src/glass.rs` | `Glass::toolbar()` | kept | kept |
| Glass button | `crates/cupertino-ui-controls/src/glass.rs` | `Glass::regular()` | kept | kept |
| A callout's pointer | `crates/cupertino-ui-controls/src/callout.rs` | `cx.glass(self.glass.capsule())` | kept | kept |
| Sidebar panel | `crates/cupertino-ui-controls/src/glass.rs` | `Surface::Sidebar` | opaque | kept |
| One-panel window (About, portal prompts) | `crates/cupertino-ui-controls/src/shell.rs` | `Surface::Sidebar` | opaque | kept |
| Window content pane | `crates/cupertino-ui-controls/src/shell.rs` | `Surface::WindowBackground` | kept | kept |
| Form pane | `crates/cupertino-ui-controls/src/form.rs` | `Surface::WindowBackground` | kept | kept |
| Scrolled navigation bar's band | `crates/cupertino-ui-controls/src/navigation.rs` | `Surface::ToolbarBand` | opaque | opaque |
| Menu bar strip | `crates/cupertino-menubar/src/app.rs` | `set_material("MenuBar")` | kept | kept |
| Menu bar menus and prompt | `crates/cupertino-menubar/src/view.rs` | `Surface::Menu` | kept | kept |
| Dock bar | `crates/cupertino-dock/src/view/bar.rs` | `Surface::DockBar` | kept | kept |
| Dock stack panel (the grid and its pointer, dense glass) | `crates/cupertino-dock/src/view/stack.rs` | `cx.glass(panel_glass(` | kept | kept |
| Dock tile name and menu | `crates/cupertino-dock/src/view/label.rs` | `Surface::Tooltip` | opaque | kept |
| Dock running indicator | `crates/cupertino-dock/src/view/bar.rs` | `Surface::DockIndicator` | kept in dark | opaque |
| Application switcher | `crates/cupertino-dock/src/switcher_kit.rs` | `Surface::Switcher` | opaque | kept |
| Control panel tiles | `crates/cupertino-controlcenter/src/view.rs` | `Glass::module()` | kept | kept |
| Control panel level tracks | `crates/cupertino-controlcenter/src/view.rs` | `Glass::track()` | kept | kept |
| Notification cards (banners and the centre) | `crates/cupertino-notifications/src/view.rs` | `Glass::card()` | kept | kept |
| Launcher panel | `crates/cupertino-launcher/src/view.rs` | `Glass::large_panel()` | kept | kept |
| Volume and brightness overlay | `crates/cupertino-osd/src/view.rs` | `Surface::Hud` | opaque | kept |
| Screenshot toolbar | `crates/cupertino-screenshot/src/view.rs` | `Surface::Hud` | opaque | kept |
| Quick preview panel | `crates/cupertino-files/src/kit/peek.rs` | `Glass::dense()` | kept | kept |
| Log out, restart, shut down, power button, interrupt and consent panels | `crates/cupertino-login/src/view/widgets.rs` | `Glass::regular()` | kept | kept |
| Authentication and keyring prompts | `crates/cupertino-polkit-agent/src/render.rs` | `then_some(PANEL_MATERIAL)` | opaque | opaque |
| Portal prompts (sharing, open with, consent) | `crates/cupertino-portal/src/dialog.rs` | `PANEL_WINDOW_BACKGROUND` | opaque | kept |
| Input-source switcher | `crates/cupertino-server/src/kbd_ui.rs` | `GlassStyle::Regular` | kept | kept |
| Accent popup | `crates/cupertino-server/src/kbd_ui.rs` | `SceneKind::AccentMaterial` | kept | kept |

Solid by design, so not in the table: a sheet, a tooltip, the Force Quit window and the pills of a fanned stack.


Approximate: every state colour, the disabled fills and borders, the opaque thumb, the dark white role text and the
dark tooltip in the Cupertino light and dark palettes; all neutral and high-contrast values.

Accepted exceptions: the Cupertino light and dark palettes keep their look below 4.5:1 for text and 3:1 for boundaries
in places (white on the accent and red fills, secondary, placeholder and light header text, control, field and switch
outlines, separators, the light focus ring at 2.49:1, link text on the light window background at 4.45:1). High contrast
and neutral are the accessible palettes. Switch tracks and separators of the neutral palettes keep the Cupertino look
below 3:1. The dark switch track is a fifth of white so it stands apart (1.75:1 or more) from the window, the text
background and the control background behind it, and the switch thumb stands 3:1 against it (tested). Slider tracks
are no exception (above).

`ColorTokens::with_accent` re-derives the selected fill, its pressed shade, the disabled selected switch track and the selected-text background from an accent colour and an optional highlight colour, so one set of tokens serves every accent.


### Theme tokens

`ThemeTokens::cupertino(theme, preferences)` is the Cupertino theme: the colours above for the preferred appearance;
the regular, non-compact row of the control size table (`control_sizes`) (`ControlMetrics::regular`: buttons and fields
24 pt with a 6 pt corner, button inset 13.5 pt, field inset 6 pt, the switch 54 × 24 pt with a 20 pt thumb 2 pt in);
body (13 pt) and caption (11 pt) sizes and the font family from the theme's type scale; the reduce-motion preference.
`ControlMetrics::at_size(size, compact)` gives the metrics at another size and setting, keeping the minimum target, and
`ControlMetrics::geometry(family)` the geometry of any family at the metrics' size. With reduced transparency, light and
dark take the opaque neutral colours, which also raise text, borders and the focus ring to their contrast ratios; high
contrast is opaque already. Approximate: 24 × 24 pt minimum target, section 12 pt and title 15 pt semibold, a 2 pt
focus ring 2 pt outside the chrome with 3 pt added radius, and 500 ms caret phases. The accent and the selection are
a softer, less saturated blue than the system blue.

`ThemeTokens::neutral(preferences)` is the theme without theme data: the neutral colours on the same metrics, body 13 pt
and caption 11 pt (the theme data's sizes), and no font family, so the application's UI font draws the text.
