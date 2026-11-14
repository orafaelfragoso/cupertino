# Plan: drawing a fractional scale at the next whole scale

On a display whose scale is fractional (a 3840 × 2160 panel at 1.5 is 2560 × 1440 points), the desktop is drawn at
the next whole scale, 2, into a frame of 5120 × 2880 pixels, and one last pass shrinks that frame to the panel's
pixels. Points, layout and input stay as they are; only the density the desktop is drawn at changes. The rules of
`docs/architecture/rendering.md` hold throughout. Tick a box only when the item is on `main`.

## Why

Drawn straight at 1.5, a feature thinner than a pixel lands on fractions of pixels: the window buttons' dark rings are
0.75 device pixels wide and look stepped, and an application that lays its own surfaces out at 1.5 can leave a device
row it declares opaque but never fills, which then shows what lies behind it. At 2 every point is two whole pixels:
edges land on the pixel grid, and the shrink turns what the 2× frame covers of each panel pixel into its value.

## Design

**What clients see.** On an output drawn this way the scale a surface is asked for is the whole render scale, the
next whole number above the output's scale (2 for 1.25, 1.5 and 1.75; 3 for 2.5): `wp_fractional_scale_v1`'s
preferred scale and `wl_surface.preferred_buffer_scale` both say it, and `wl_output.scale` says it already (the
output's scale rounded up). The output's mode, its logical size (`xdg_output`), its scale in the output management
protocol, every position and the input mapping stay the output's own. A client that follows the preferred scale draws
2 × its points; one that follows `wl_output.scale` draws the same. The cursor's own surface, which the server draws at
the panel's pixels (below), is told the output's own scale.

**What the compositor draws.** The scene is composed at the render scale with the frame's size the output's points
times that scale, upright, into an offscreen frame of the output's own: windows, layers, glass, materials, shadows,
decorations, HUDs and popups, in their usual order and with their usual damage. One final element covers the output
and draws that frame shrunk to the panel's pixels; the pointer and what lies above it (the compositor's notices and
the session's fades) are composed at the panel's own scale and drawn over it, so the cursor stays as sharp as before
and can still go on the cursor plane. The output's transform is applied by the final pass.

**The filter.** Each panel pixel is the average of the 2× frame over the area the pixel covers there (a box of
4/3 texels a side at 1.5), each texel weighted by how much of it the pixel covers: four bilinear lookups give the
weights exactly. Its weights are never negative, so an edge does not ring; an edge's value is the share of the pixel
it covers, linear in where the edge lies, so a moving or thin edge does not step, and a line keeps its weight
wherever it lies. A pattern of a period of 4 panel pixels keeps about 0.85 of its contrast (bilinear 0.89), and a
pattern finer than the panel can show (0.6 cycles a pixel, which the 2× frame holds) folds back as a coarser one at
under 0.4 of its contrast, at least 0.05 less than through bilinear. Plain bilinear lookups at the pixel centres
weigh texels by where the centre falls, so the same edge comes out differently at each of the three phases of the
4 : 3 grid, holds still over part of its travel, and a thin line's weight swings with where it lies by more than
0.15 of it; a sharper cubic filter keeps more contrast but overshoots and undershoots at every edge. The definition,
the comparisons and their tests are in `cupertino_raster_math::resample`; the shader is tested against it.

**Precision and dither.** The offscreen frame has 10 bits a channel (`Abgr2101010`, 8 bits where the device cannot
render to it), at least what any output gets, and nothing reads its alpha. The final pass dithers what it writes by
the panel pixel's position with a step of the output's depth (1/255, or 1/1023 at 10 bits), as the definition of
dithering says; with dithering off it does not.

**Damage.** The offscreen frame is drawn through a damage tracker of its own and keeps its content from one frame to
the next, so only what changed is drawn at 2×. What changed maps to the panel's pixels, widened by the filter's
footprint (one pixel), and that is the final element's damage; a frame with no damage draws nothing, and an idle
desktop draws no frames.

**Direct scanout** of a full-screen window is not possible while the frame is shrunk; it is given up on such outputs.

**Screenshots and screen capture** hand out what the panel shows: the shrunk frame at the panel's pixels (3840 ×
2160), with the cursor drawn at the panel's scale. A capture of one window draws the window at the render scale, as
its client drew it: its points times 2.

**The setting.** `fractional_scale_mode` in `compositor.json`: `"supersample"` (the default) or `"direct"`, or an
object of display names (the connector, such as `DP-1`, or the display's identity) to modes with `"*"` for the rest.
An output at a whole scale is drawn directly in either mode. `"direct"` is the drawing before this plan: everything at
the output's scale.

**Opaque rows under `"direct"`.** A client that declares a region opaque has its region taken as opaque at device
pixels, rounded to the nearest pixel edge, and drawn without blending. Where its buffer leaves such a row
transparent, the row shows black, as the client declared nothing shows through it; the wallpaper and the windows
behind are never drawn there. This already held for a single surface at both phases of the 1.5 grid, and a test
keeps it; a seam an application leaves between surfaces of its own that are not declared opaque is what the
application draws, under either mode. Under `"supersample"` an application laid out at 2 has no row half way
across a pixel to leave.

## Cost (expected; the lead measures)

- Pixels: 5120 × 2880 = 14.7 M a full frame at 2×, 1.78 times the 8.3 M of 3840 × 2160; damage-limited frames draw
  1.78 times their area, plus the final pass over the damage at the panel's pixels (four lookups a pixel).
- Memory: one offscreen frame a supersampled output, 5120 × 2880 × 4 bytes = 59 MB; the material passes' own levels
  grow with the frame (their half-scale levels at 2× are 2560 × 1440).
- Direct scanout is lost on such outputs.
- Idle: unchanged, no frame without damage.

## Checklist

- [ ] `fractional_scale_mode` read from `compositor.json`, per display, `"supersample"` by default.
- [ ] Surfaces on a supersampled output are told the render scale (fractional and buffer scale); the cursor surface
      the output's own.
- [ ] The scene composed at the render scale into the output's offscreen frame through its own damage tracker; the
      pointer, notices and fades composed at the output's scale above it.
- [ ] The filter's definition with its frequency and step tests against plain bilinear; the shader tested against it.
- [ ] The final pass on every target: the display (cursor plane kept), another card's copy, the headless and nested
      outputs, screenshots and screen capture.
- [ ] Damage mapped to the panel widened by the footprint; no frame without damage.
- [ ] 10-bit offscreen frame and the final pass dithered at the output's depth.
- [ ] Declared-opaque transparent rows drawn black under `"direct"`.
- [ ] Docs: displays page, GPU rendering design (the texture program by name), the settings table.
- [ ] Measured on the target display: frame time at 1.5 supersampled against direct (lead).
