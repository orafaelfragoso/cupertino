# cupertino-decorations

Server-side window decorations, renderer-agnostic: a decoration model plus CPU rasterisation (`tiny-skia`) into
premultiplied RGBA buffers that a compositor uploads as textures.

## Public API

| Module | Content |
|---|---|
| `metrics` | `TitlebarStyle::{Standard, Unified, UnifiedCompact, Sidebar}`, `ButtonKind`, `DecorationMetrics`, `DoubleClickAction`, `TitleAlignment`; the window style constants `WINDOW_BUTTON_DIAMETER` (14 pt; `WINDOW_BUTTON_DIAMETER_SMALL`, 9 pt, at the other sizes), `WINDOW_BUTTON_PITCH`, `TITLED_BAR_HEIGHT`, `TOOLBAR_ROW_HEIGHT`, `COMPACT_TOOLBAR_ROW_HEIGHT` and `WINDOW_CORNER_RADIUS` |
| `chrome` | Window chrome as a layout system: `ChromeStyle` (frame kind, `ToolbarStyle`, title shown, full-size content, transparent title bar), `ChromeLayout` (outer `frame`, drawable `content`, `unobscured` layout rectangle, button row, toolbar row, title and items slots, accessories), `ChromeLayout::full_screen`, `button_presence` (the capability table), `Direction` (`of_locale`), `Accessory` |
| `layout` | `FrameLayout`: titlebar, content, `unobscured` (the content no title bar or client-drawn row covers), three button circles + art boxes (mirrored right to left with `DecorationMetrics::direction`), button group, `title_slot` (centred title stays centred and clear of the buttons, leading title for unified bars) |
| `hit` | `hit_test` → `Hit::{Button, Titlebar, Content, Resize(edge), None}`; `resize_hit(frame, metrics, p, exterior)` the resize zone alone, for any frame (the compositor uses it for client-decorated and untitled windows; `exterior` false leaves out the 4 pt outside part, which titled windows only have) (no `Titlebar` hit in a unified frame: the client moves the window), resize zones from `resize_outset` outside to `resize_inset` inside the geometry with corner zones measured along the edges (`resize_corner`, `resize_corner_bottom_right`), `input_region`, button-group hover, double-click action |
| `buttons` | states normal / hover-group (glyphs on all three) / pressed / inactive (grey, colour returns on hover) / disabled; glyphs authored as vector strokes (cross, dash, plus) |
| `palette` | colours from the theme tokens: titlebar fills from the opaque titlebar recipes (an inactive window's the same as an active one's; its title takes the secondary label colour and its buttons turn grey), the window rim token and its 2× variant, the window frame text token for the title, button hues from the system colours; `Palette::builtin` works without spec files and is tested equal |
| `glyph` | Glyph rasterisation: exact-area coverage of the flattened outline; the dilation (stem darkening) of drawn text, d = min((0.0058 + 0.01 Yq) s, 0.3) on both axes from the fill colour's quantised linear luminance Yq and the device pixel size s, applied by edge translation with the +d upward shift; the glyph origin quantisation by size |
| `field` | The inside of a single-line text field for the apps: `caret_offsets` measures the x of each caret position in one shaping pass (`TextRenderer::offsets`), `draw_field` draws the selection band (the `selected-text-background` token), the text, the composition underline and the caret (1 pt), clipped to the field and scrolled; the band and caret reach 2 pt above and below the text size |
| `text` | `swash` shaping and title rendering: `TextRenderer::new(&assets, FontRole::Ui)` reads the fonts of the asset packs, then the built-in font (`cupertino-assets`; no host fonts), and takes the packs' family for the role, else the first usable family. The `ui` font is one variable font, so shaping and rasterisation both get the variation coordinates of the request: CSS weight mapped through the font's named instances (each at its `wght` coordinate on a standard axis, else by its style name, Thin 100 … Black 900, case-insensitive; static faces by their weight class) and, after `set_scale`, `opsz` = point size clamped to the axis (17..96); truncation with an ellipsis; tracking table of `cupertino-theme` as letter spacing |
| `edge` | `edge(kind, dark, active, increased_contrast, contrast_rim)` → `Edge`: a black outline half a point wide just outside a window or a menu (window 0.93 dark, 0.22 light, an inactive window 0.62 of that; menu 0.65 dark, 0.15 light), a menu's only along its sides (`OutlineFalloff::Sides`, `|n.x|²` of the outward normal); a window's white inner rim at half the shadow table's inner-rim density, as wide as its inner-rim scale (1 pt dark, ½ pt light), and a menu's white inner rim half a point wide, 0.22 dark and 0.5 light along the top and bottom and 0.45 of that along the sides (`OutlineFalloff::Ends`, `0.45 + 0.55 |n.y|`); with Increase Contrast the outline is the `window-rim` token a point wide all around and a menu has no inner rim. `Hairline::device(scale)` gives whole device pixels at the strength their coverage leaves |
| `shadow` | `ShadowParams::lookup` over the verified 32-row table, `radius_px = ceil(σ·2.8)`, margins, `ShadowGpuParams` (device pixels, flat array), `ShadowRasterizer` (cached nine-slice source, equals the direct render within 1/255), knock-out under the window |
| `render` | `Decorations`: `layout`, `hit_test`, `render_titlebar`, `render_rim` (the inner rim), `edge` and `menu_edge`, `shape`, `shadow_params`, `title_placement` |
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
fractional: buffers are `round(size · scale)`, and the edge's lines are whole device pixels, at least one, at the strength
their coverage leaves (half a point at 1× is one pixel at half strength). The compositor draws the outline outside the
window; `render_rim` draws the inner rim inside it. The window shape (all four corners
rounded) is `Decorations::shape`, and the compositor must clip the client content to it.

### Window styles

| Style | Row the buttons lie in | Button centres | Drawn by the server |
|---|---|---|---|
| `Standard` (titled) | a 32 pt title bar above the content | (16, 16), then every 23 pt | the title bar, the centred title and the buttons |
| `Unified` | the client's 52 pt toolbar row | (26, 26), then every 23 pt | the buttons only |
| `UnifiedCompact` | the client's 40 pt toolbar row | (19, 20), then every 23 pt | the buttons only |
| `Sidebar` (full-height sidebar) | the 52 pt top of the client's sidebar | (26, 26), then every 23 pt | the buttons only |

The buttons are 14 pt across; each centre lies half the row's height from the top and leading edges, except in the
compact row, whose group starts 12 pt from the leading edge (13 pt from the top). Every style has a
16 pt window corner and the same edge and shadow. In the three client-drawn styles the content fills the frame, the
client draws the row and moves the window from it, and `hit_test` gives `Hit::Button` over the buttons alone.

### Chrome layout

`ChromeLayout::new(content, style, capabilities, direction, accessories)` models three rectangles: the outer frame, the
content the client draws and the unobscured rectangle where its layout starts. With full-size content the content is
the whole frame and the chrome covers its top; otherwise the frame grows by the chrome. The chrome's height (frame less
unobscured) depends on the frame kind and toolbar only:

| Style | Chrome | First button (left, top), side, pitch |
|---|---|---|
| titled, no toolbar (title shown or hidden, transparent or not) | 32 | (9, 9), 14, 23 |
| utility panel | 24 | (8, 8), 10, 17 |
| unified toolbar (also the automatic style) | 52 | (19, 19), 14, 23 |
| compact unified toolbar | 40 | (12, 13), 14, 23 |
| expanded toolbar (title row above a 36 pt toolbar row) | 68 | (9, 9), 14, 23 |
| settings-style toolbar (title row above a 56 pt toolbar row) | 88 | (9, 9), 14, 23 |
| borderless | 0 | none |

So a 600 pt full-size-content window has a 568 pt unobscured rectangle without a toolbar and 548 pt under a unified
toolbar. The server's styles map onto it with `ChromeStyle::of_titlebar`: `Standard` is titled, `Unified` and
`UnifiedCompact` are full-size content under their row, `Sidebar` is the unified row with the title hidden.

The title shares the unified or compact row after the button group, or sits centred above an expanded or settings-style
toolbar, or in the title bar without one; hiding it gives its room to the toolbar items. Leading and trailing
accessories sit in the toolbar row clear of the group, the title and the items; bottom accessories add to the chrome,
and `ChromeLayout::full_screen` keeps only their `full_screen_min_height` while the title bar and toolbar hide. A
right-to-left window (`Direction::RightToLeft`, from the locale with `Direction::of_locale`) mirrors the group, the
title, the items and the accessories; close stays outermost.

`button_presence(frame, capabilities)` decides the buttons: a borderless window, and a titled one that can neither
close, minimize nor resize, has none; any other has all three in place, each disabled when the window cannot do it
(zoom follows resizability; a window that may not go full screen keeps zoom enabled). A utility panel closes only, so its
minimize and zoom are present and disabled.

Decisions: an expanded or settings-style toolbar keeps its allocation with the title hidden, as a titled window does;
the title in a shared row takes at most 160 pt before the items (`SHARED_TITLE_WIDTH`). The server decorates every
window in the left-to-right direction: no client tells it its direction yet.

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
colours and glyphs per state, the edge (outline strength by appearance, activity and Increase Contrast, the side
fall-off of a menu's outline, the inner rim and its device pixels), titlebar fills, shadow fall-off and peak density (0.40 light, 0.70 dark), nine-slice
equals direct render at 1× and 2×, GPU parameters, and title centring, truncation, tracking, the weight scale, ink growth with weight and the optical size. The same title bar
at 1, 1.25, 1.5, 1.75 and 2 is painted throughout, has each button's ink centred on its laid-out point within half a
device pixel (in the dark appearance too: a colour change moves no control), has an inked title and a rim on whole device
pixels, and after other scales draws the same bytes at 2 again. The bench prints, on the
development host, titlebar rasterisation (≈0.3 ms), nine-slice shadow expansion (≈2 ms at 1×, ≈11 ms at 2× for a 1600×900
window) and the direct render (≈31 ms).
