# cupertino-decorations

Server-side window decorations, renderer-agnostic: a decoration model plus CPU rasterisation (`tiny-skia`) into
premultiplied RGBA buffers that a compositor uploads as textures.

## Public API

| Module | Content |
|---|---|
| `metrics` | `TitlebarStyle::{Standard, Unified}`, `ButtonKind`, `DecorationMetrics`, `DoubleClickAction`, `TitleAlignment` |
| `layout` | `FrameLayout`: titlebar, content, three button circles + art boxes, button group, `title_slot` (centred title stays centred and clear of the buttons, leading title for unified bars) |
| `hit` | `hit_test` → `Hit::{Button, Titlebar, Content, Resize(edge), None}`; `resize_hit(frame, metrics, p, exterior)` the resize zone alone, for any frame (the compositor uses it for client-decorated and untitled windows; `exterior` false leaves out the 4 pt outside part, which titled windows only have) (no `Titlebar` hit in a unified frame: the client moves the window), resize zones from `resize_outset` outside to `resize_inset` inside the geometry with corner zones measured along the edges (`resize_corner`, `resize_corner_bottom_right`), `input_region`, button-group hover, double-click action |
| `buttons` | states normal / hover-group (glyphs on all three) / pressed / inactive (grey, colour returns on hover) / disabled; glyphs authored as vector strokes (cross, dash, plus) |
| `palette` | colours from the theme tokens: titlebar fills from the opaque titlebar recipes, the window rim token and its 2× variant, the window frame text token for the title, button hues from the system colours; `Palette::builtin` works without spec files and is tested equal |
| `glyph` | Glyph rasterisation: exact-area coverage of the flattened outline; the dilation (stem darkening) of drawn text, d = min((0.0058 + 0.01 Yq) s, 0.3) on both axes from the fill colour's quantised linear luminance Yq and the device pixel size s, applied by edge translation with the +d upward shift; the glyph origin quantisation by size |
| `text` | `swash` shaping and title rendering: `TextRenderer::new(&assets, FontRole::Ui)` reads the fonts of the asset packs, then the built-in font (`cupertino-assets`; no host fonts), and takes the packs' family for the role, else the first usable family. The `ui` font is one variable font, so shaping and rasterisation both get the variation coordinates of the request: CSS weight mapped through the font's named instances (each at its `wght` coordinate on a standard axis, else by its style name, Thin 100 … Black 900, case-insensitive; static faces by their weight class) and, after `set_scale`, `opsz` = point size clamped to the axis (17..96); truncation with an ellipsis; tracking table of `cupertino-theme` as letter spacing |
| `shadow` | `ShadowParams::lookup` over the verified 32-row table, `radius_px = ceil(σ·2.8)`, margins, `ShadowGpuParams` (device pixels, flat array), `ShadowRasterizer` (cached nine-slice source, equals the direct render within 1/255), knock-out under the window |
| `render` | `Decorations`: `layout`, `hit_test`, `render_titlebar`, `render_rim`, `shape`, `shadow_params`, `title_placement` |
| `preview` | composed window (shadow + content stand-in + titlebar + rim) |

## Usage

```rust
use cupertino_decorations::*;
use cupertino_geometry::Size;
use cupertino_theme::{Appearance, Theme};

let theme = Theme::load()?;                                   // or DecorationConfig::builtin(..)
let mut dec = Decorations::new(DecorationConfig::from_theme(&theme, Appearance::Light, TitlebarStyle::Standard));
let content = Size::new(800.0, 600.0);
let frame = dec.frame_size(content);                          // 800 × 628
let hit = dec.hit_test(content, pointer_in_frame_coordinates, true);
let state = DecorationState { title: "Document".into(), group_hovered: true, ..Default::default() };
let titlebar = dec.render_titlebar(frame.width, 2.0, &state); // tiny_skia::Pixmap, premultiplied RGBA
let rim = dec.render_rim(frame, 2.0);
let params = dec.shadow_params(state.active);
let shadow = ShadowRasterizer::new().render(&params, frame, 12.0, dec.config.corner_style, 2.0);
```

## How it works

Coordinates are frame-local logical points (y down, titlebar at the top). `scale` is device pixels per point and may be
fractional: buffers are `round(size · scale)`, and the rim is always one device pixel. The window shape (all four corners
rounded) is `Decorations::shape`, and the compositor must clip the client content to it.

### Samples

```
cargo run -p cupertino-decorations --release --example render_samples
```

writes to `output/`: `window_states_light.png` /
`window_states_dark.png` (active, inactive, hover-group, pressed close, disabled zoom), `styles_unified_and_small.png`,
`buttons_closeup.png` (8× close-ups of every state) and `shadow_profiles.png` (standard active/inactive, panel, tooltip,
light and dark). The samples and every test that draws text need `CUPERTINO_ASSET_DIR` (see `cupertino-assets`); without it they
draw with the built-in font and the pixel expectations of the tests fail.

## Open issues

- A `TextRenderer` whose owner never calls `set_scale` draws the display design of the `ui` font (about 12% narrower than the 17 master at 13 pt). Every owner in the tree calls it with its output scale: `Decorations` (title placement and rendering), the server's overlay text, the dock, launcher, menu bar, file manager, control panel, notifications, OSD, login and screenshot processes. A new owner must too.
- Text is shaped left to right only; right-to-left runs are not reordered.
- Dilation is on for all text (`text::DEFAULT_SMOOTHING`) and follows the fill colour: dark
  text at 13 pt and 2x gets 0.1508 px, white text the 0.3 cap. It is off only when the smoothing preference holds a value
  other than 1, 2 or 3; that preference has no key here yet.
- `TextRenderer::set_declarative` floors the dilation size to a power of 1.25 (26 px becomes 22.74, 0.1319 px
  for dark text). Only the settings application sets it (its design is a declarative-toolkit application).
  Which toolkit the references of the launcher and the control panel use is not recorded, so they keep the unfloored size.
- Measured against the two screenshots per-word ink integrals at 13 pt, 2x; the record, not a target:
  the screenshots likely had smoothing off): undilated / reference 0.997 (Settings, 7 words) and 0.97-0.98 (file
  manager); with this rule 1.16-1.20 and 1.145-1.153; with the earlier smoothing-table medium 1.33-1.36 and 1.31-1.32.
- No autohinter. With the `ui` font at 13 px (x-height 6.97 px, cap height 9.16) the "high" target
  pair fails the stem-preserving test (m1 0.94, m2 2.44), where the design uses a two-segment fallback that is not
  known; the "round" pair passes (m2 1.01). How the evaluator picks the pair and the fallback map are open in.

## Tests

```
cargo test -p cupertino-decorations
cargo bench -p cupertino-decorations
```

Tests cover layout, hit tests, button
colours and glyphs per state, rim, titlebar fills, shadow fall-off and peak density (0.40 light, 0.70 dark), nine-slice
equals direct render at 1× and 2×, GPU parameters, and title centring, truncation, tracking, the weight scale, ink growth with weight and the optical size. The bench prints, on the
development host, titlebar rasterisation (≈0.3 ms), nine-slice shadow expansion (≈2 ms at 1×, ≈11 ms at 2× for a 1600×900
window) and the direct render (≈31 ms).
