# cupertino-glass

Material rendering for the Cupertino compositor: backdrop blur, colour filter chains, fill layers with compositing
filters and vibrant blending, driven by the recipes of `cupertino-theme`.

## Public API

| Item | Role |
|---|---|
| `MaterialRenderer` | library API on an external wgpu `Device`/`Queue`; `render` records into the caller's encoder from a `FrameDesc` and a list of `MaterialRegion`s |
| `glass-renderer` | headless binary (`--demo`, `--cpu-check`, `--bench N`) |
| `cpu` | f32 CPU reference of the same pipeline (half-float intermediates emulated); `apply_color_ops` runs a recipe's colour chain on one colour, which the server uses to measure the wallpaper as the menu bar's material shows it |
| `shaders/gles/ | GLSL ES 3.00 port of every shader for the compositor's GLES renderer |
| `glass` | the glass background pass: `Container` (shapes joined by a smooth union whose blend width is the container's spacing, each with the recipe of its size from `cupertino-theme`), `Container::shade` (the CPU model of every stage, for tests), `GlassUniform` (the std140 block of `glass.wgsl` and `gles/glass.frag`) |
| `display_filter` | the display's colour filter, invert and contrast as a colour chain over a whole output (settings from `cupertino-defaults`) |

## Usage

```rust
use cupertino_glass::{FrameDesc, MaterialRegion, MaterialRenderer, TargetInit};
use cupertino_geometry::CornerStyle;

let mut renderer = MaterialRenderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
let regions = [
    MaterialRegion::new([60.0, 80.0, 220.0, 420.0], &sidebar_recipe)
        .with_corner(12.0, CornerStyle::continuous()),
];
let mut enc = device.create_command_encoder(&Default::default());
let stats = renderer.render(&mut enc, &FrameDesc {
    source: &scene_view, source_size: (w, h),
    target: &target_view, target_size: (w, h),
    display_scale: 1.0,
    damage_generation: generation,   // bump when the source content changes
    tint_color: wallpaper_average,
    init: TargetInit::Keep,
}, &regions);
queue.submit([enc.finish()]);
```

## How it works

### Regions and caching

- Each region has a rectangle (target pixels), a `Recipe`, a corner radius and style (circular or continuous, from
  `cupertino-geometry`) and an opacity.
- The processed backdrop (downsample pyramid, separable Gaussian, colour filter chain) depends only on the recipe's
  backdrop settings, the display scale and the source size. Regions share it, and it is cached across frames keyed by
  `damage_generation`. While the counter is unchanged, no blur pass is encoded (`RenderStats::backdrop_hits` /
  `backdrop_misses` / `passes_encoded`). Entries unused for 120 frames are dropped, and `invalidate clears the cache.
- The rounded mask is a signed distance field evaluated in the fragment shader, and anti-aliasing is a one-pixel linear
  ramp. The shader uses the closed-form first-order distance of `RoundedRect::sdf_approx`.
- `render` never submits. `source` and `target` must be different textures in non-sRGB formats (values are blended
  gamma-encoded).
- `draw_vibrant` draws premultiplied template glyphs with the vibrant blend functions (lighten-sover over
  colour dodge;: darken-sover over colour burn, plus the reversed nesting). The
  effect constants `c0`/`c1` are unknown and default to the identity multiplier. `backdrop` must hold a copy of the target
  content.

### Pipeline

```
source ─(2× box)→ capture @ backdrop scale ─(2× box pyramid while σ² > 64)→ separable Gaussian
       ─→ colour chain (luminance curve, saturation, monochrome, display filter ops) ─→ cached backdrop (rgba16float)
region quad ─ SDF mask ─ base = backdrop (or source) ─ fills with blend modes ─ alpha blend
```

All stages are full-screen fragment passes (no compute), so each has a direct GLSL ES 3.00 counterpart. The tint layer colour (wallpaper average) is
approximate and supplied by the caller.

### Display filters

`display_filter::display_filter_ops(&AccessibilitySettings) gives the chain the compositor runs over a whole output
(backdrop at scale 1 with no blur, then the chain), in the design's order:

| Op | Slots | Effect |
|---|---|---|
| `LinearMatrix(m) | 2 (`[4, m00, m01, m02] [m10, m11, m12, m20]`, tail `[7, m21, m22, 0]) | sRGB decode, `m. rgb`, clamp, encode |
| `Invert` | 1 (`[5, 0, 0, 0]) | `1 - rgb` on encoded values |
| `Contrast(c) | 1 (`[6, c, 0, 0]) | encoded `[c/2, 1 - c/2]` stretched to `[0, 1]`; `c = 1` is a threshold at 0.5 |

`filter_matrix` builds grayscale, the protanopia / deuteranopia / tritanopia corrections and the tint (HSV hue sector
table) with the design's `Normalize`. Channel gains, fade, post offsets and reduce white point are not in the chain
(the design drops the offsets on this path).

### Glass

`src/glass.rs` is the model of the glass background pass and `shaders/glass.wgsl` / `gles/glass.vert`, `gles/glass.frag`
draw it, one draw per container over its sampled area (the shapes grown by the reach of the blur and refraction,
`Container::sample_area`). The backdrop is read at half scale into up to four blurred copies and a blur fill; the
composite then, per pixel:

1. evaluates the field: the smooth union of the shapes' rounded-rectangle distances, polynomial blend of width
   `spacing` (0 is a hard union, at most `MAX_SPACING`, 64 points), with each shape's size-dependent values mixed by
   the same weights; the union bulges up to a quarter of the spacing past the shapes, which the sampled area includes;
2. refraction: inside the band along the edge, the sample point moves along the outward normal by the offset times
   (1 − t)², t the depth into the band;
3. blur and blur fill: each shape's backdrop is one Gaussian, its deviation twice the recipe's radius scaled by the first
   level's opacity (4 points for a small regular control, 8 for a regular panel 160 points high, 20 for clear glass of
   radius 10); the container blurs once per distinct deviation of its shapes (at most four copies, a shape between two
   mixing them), then the fill, a Gaussian of twice its radius, darkens, lightens or covers;
4. face colour: saturation about the luma of the encoded colour (so the weaker channels of a strong colour keep a milky
   floor rather than dropping to black, as they would in linear light), then the luma compression
   `c − (1 − max_luma)·L·(0.75c + 0.25L)` (a grey x becomes `x − (1 − max_luma)·x²`), the levels, one shift
   `black·(1 − L) − (1 − white)·L` added to every channel (L the luma after the compression), then the fill over it,
   all on encoded values;
5. bleed: the blur fill (made whenever a shape bleeds, even when the fill does not show), saturated (encoded) and mapped
   between black and white, lightens (dark) or darkens (light) the face in the bleed band, at the bleed's opacity (none
   at rest);
6. soft band and rim: the soft band, a Gaussian about the edge where the highlight matrix is applied to the face, lit
   by a raised-cosine lobe around each end of the light's axis (the top and the bottom edge, not the sides), drawn for
   interaction states and for the surfaces that ask for it; then the rim, one shift added to every channel,
   `amount × colour bias × g(L) × w(φ)` in a band 1.7 × the highlight height deep, g the response to the face's luma
   (0, 0.63, 1, 1.13, 1 at 0, ¼, ½, ¾, 1) and w = 0.15 + 0.85·sin²(2φ) of the edge's direction (darkening most on the
   diagonal corners when the bias is negative);
7. the ring shadow, a stroke inside the outline moved up by its offset and blurred by a Gaussian of its radius,
   darkens the face by its opacity; outside the shape, the drop shadow at its opacity.

Unit tests check each stage on the model (`glass::tests`; a one-dimensional run of stages 3 and 4, the blur and the
fill each a Gaussian of its deviation, shows the large panel blurring a step over twice the distance of regular glass of its
size and a 6 point bright stripe coming through at about half the height), and that body text (black at 85 %) on light regular glass stays at 4.5:1 or more over the darkest backdrops at every panel size and accessibility recipe, and `tests/shaders.rs` checks that the `Glass` block has the
same members in both shader languages and the Rust struct's size.

**Foreground.** `cupertino-theme` holds the foreground matrices (`foreground_matrix`, lifting the face under a glyph to
white on dark glass and dropping it to black on light glass) and `adapts_foreground` (shapes whose shorter side is under
64 points choose the matrix from what is behind them, larger bars and panels keep their appearance's); `glass::foreground`
is their CPU model. The toolkit draws its text in the client, which never sees the backdrop, so the server tells it:

1. after it blurs a container whose union of shapes adapts, the server reads the widest blur level back into a grid of
   at most 32 × 32 texels through a pixel buffer, without stalling (the fence is polled after later frames), takes the
   mean under each shape, runs it through `face_color` and sends the `tone` event of `cupertino_backdrop_v1` (version 7)
   when the face's luminance crosses `FACE_TONE_THRESHOLD` by more than `FACE_TONE_HYSTERESIS`. Nothing is read or sent
   while the backdrop does not change; a shape that stops adapting is told `appearance` once;
2. the toolkit gives a glass node and everything drawn on it (its subtree, and later nodes inside its shape) the theme
   of text on glass (`ThemeTokens::on_glass`): neutral labels take `foreground_rgb` of the tone at the vibrant levels
   (`VIBRANT_PRIMARY`, `VIBRANT_SECONDARY`, `VIBRANT_PLACEHOLDER`); accents, fills and selected labels keep theirs.

The alternative, drawing glass text in the compositor as the menu bar's vibrant text is (`draw_vibrant`), needs the
glyphs on a surface of their own and is not planned.

### GLSL ES 3.00 port

`shaders/gles/`: `fullscreen.vert`, `downsample.frag`, `blur.frag`, `color.frag`, `blit.frag`,
`material.vert`, `material.frag`, `vibrant.vert`, `vibrant.frag`, `glass.vert`, `glass.frag`. Uniform blocks use std140 and are
byte-identical to the Rust structs in `src/ops.rs`, so the same buffers work for both back ends.
Material and vibrant quads are 4-vertex triangle strips (`gl_VertexID`). Samplers: `u_src`,
`u_source`, `u_backdrop`, `u_glyphs`. When the source/backdrop textures were rendered by GL (row 0 at
the bottom) set `Region.info.z = 1`.

`tests/shaders.rs` validates the WGSL sources and the GLSL ports with `naga`. Its GLSL front end only accepts the Vulkan
dialect, so the test first rewrites the ES sources (version line, `gl_VertexID`, location/binding qualifiers,
texture/sampler split, `precision` statements).

## Tests

```
cargo test -p cupertino-glass
```

Tests cover recipe → ops encoding, blur plan, uniform layout, display filter matrices against the
reference builders, the CPU chain of matrix, invert and contrast, GPU vs CPU PSNR ≥ 60 dB (multi-region frames at scale 1
and 2), the CPU colour chain against the GPU colour pass within one level (both menu bar recipes, unblurred, on a grey
ramp, a saturated hue ramp and the procedural wallpaper), the rounded mask against `cupertino-geometry` coverage, circle area, opacity, backdrop cache hit/miss accounting,
vibrant blend GPU vs CPU, and WGSL and GLSL ES validation (`tests/shaders.rs`). GPU tests skip (pass) when no adapter is
available.

## Binary

```
cargo run --release -p cupertino-glass --bin glass-renderer -- --demo
cargo run --release -p cupertino-glass --bin glass-renderer -- --material Sidebar --appearance dark \
    --size 1920x1080 --continuous --radius 12 --cpu-check --bench 200
```

Images are written to `output/`. `--appearance` accepts `light`, `dark`, `light-high-contrast`,
`dark-high-contrast`; `--reduce-transparency`, `--window inactive` and `--interaction rollover|pressed`
select the other recipe variants.
