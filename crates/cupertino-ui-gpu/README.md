# cupertino-ui-gpu

CupertinoKit's GPU renderer: `GpuRenderer` implements `UiRenderer` over a `cupertino-gpu` device and draws a window's
`DisplayList` into a texture the presenter hands to the compositor. MPL-2.0. It keeps the CPU renderer's results: the
CPU renderer is the standard it is tested against (every golden scene within 2/255 per channel) and the fallback.

What it draws, and how:

- Rectangles and rounded rectangles, filled and stroked, by their distance in the fragment program, with the shared
  coverage, stroke and rounding of `cupertino-raster-math`.
- Linear and radial gradients of up to eight stops, interpolated and dithered as the CPU renderer does, by the window
  pixel.
- Glyph runs from two atlases: coverage glyphs tinted by the run's colour, colour glyphs laid as they are.
- Images and icons from textures kept by revision under a byte budget, sampled nearest or bilinear inside their
  crop, edges covered as the CPU renderer covers them; one image pixel to one window pixel is laid unfiltered.
- Paths as coverage masks in the mask atlas; a solid stroke along a path is drawn in its colour and laid as an image.
- Clips: rectangles on whole pixels by the scissor, up to four rectangles or rounded rectangles in the program,
  multiplied in turn, and the rest (paths among them) in one clip mask in the mask atlas.
- Groups and operation layers as offscreen targets from the target pool, laid over at their opacity.
- Blur, drop shadow and inner shadow on a layer: halved levels, a Gaussian across and down, and a resolve pass that
  shifts, tints, dithers and lays the result as the CPU renderer does.

A frame repaints only the damage: the rectangles every renderer repaints (`cupertino_ui_render::damage_rects`), each
cleared and drawn under a scissor. The draws of a frame are recorded first (`record`), then their uniforms and
instances uploaded in one buffer each and the passes run (`exec`). Offscreen targets come from the target pool; one
the frame has finished with (a layer laid over, a blur level read) gives its texture to the next target of its size
and format, so a frame holds only the targets alive at once and a frame like the last makes none.

A window draws with `WindowRenderer`, a `UiRenderer` whose frames are `CpuFrame`s with empty `pixels` and the
`WindowTexture` in `device`: the window-sized texture, shared between the renderer and its frames, which the
presenter on the same device copies into its buffers and anything else reads back (`CpuFrame::in_memory`). The
programs (`GpuPrograms`) are made once per device and shared by every renderer on it (`GpuRenderer::with_programs`).

The tests (`tests/`) draw the golden scenes against the CPU renderer's baselines, and generated scenes of each
primitive group on both renderers at scales 1, 1.5 and 2 on odd sizes, whole and repainting damage. They need an EGL
device: the first render node, or the surfaceless platform; `CUPERTINO_GPU_REQUIRED=1` makes a missing or software
device a failure (the local GPU tier), `CUPERTINO_EGL_REQUIRED=1` a missing one (CI).
