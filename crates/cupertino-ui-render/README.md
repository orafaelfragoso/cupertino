# cupertino-ui-render

CupertinoKit rendering: the `DisplayList` of paint nodes core builds for each window (paths, gradients, images, glyph
runs, clipped and isolated layers, materials already resolved to plain paint), `PaintResources`, `DamageRegion`, and
the theme's `Color` it paints with (re-exported for the window-system interface), the `UiRenderer` trait whose frame (`CpuFrame`, premultiplied RGBA) the window system presents; a frame drawn on a GPU keeps its image on the device (`CpuFrame::device`, a `DeviceFrame`) and `CpuFrame::in_memory` reads it back. MPL-2.0. `gpu::GpuRenderer` is the GPU backend, over `cupertino-gpu` (*GPU renderer*). `CpuRenderer` is the CPU backend (tiny-skia, private): it validates a request before touching the image it keeps, repaints
only the damage, executes clips, group opacity, blur and shadows on isolated
layers, and caches rasterized glyphs and decoded images under explicit byte budgets. A frame drawn over the last one
has the bytes of the same list drawn whole, changes and damage at scales 1, 1.25,
1.5 and 2. The rasterizer cuts a path at the edge of the image it draws into, and a curve or slanted edge cut there
comes out a little differently, so nothing is rasterized into an image the damage sizes: a node's scratch image covers
all of the node that shows (or, for a node of solid fills, each rectangle of its rim on its own), a group's layer and an
operation layer all of theirs, and the damage only picks which of their pixels are laid over the frame. Only glyphs are
blended straight into the frame, each pixel on its own. A small change under a large node that is not solid fills
therefore rasterizes the whole node, and a frame drawn whole lays every node through a scratch image. Inside a group or
operation layer a node whose operations only draw is painted straight into the layer when no clip cuts it: a clip
that is a rectangle or rounded rectangle under a scale and translation, and lets through, a pixel in from its edge and
clear of its corners, everything the node can reach (`geometry::clip_holds`), cuts nothing, whatever the damage.

Laying an image over another (`source_over_part`, a group's opacity, a clip mask over a group's layer) works on whole
pixels as words, two channels to a 16-bit lane (`effects::mul_px`, `effects::over_px`, the same bytes as the channel by
channel arithmetic), skips runs of 16 empty pixels (or of
pixels a mask covers fully) read a word at a time, and has no branch per pixel, so it vectorizes: a mostly empty layer
the size of a 2x window, such as a panel's outline, is laid over in a fraction of a millisecond. A run of 16 opaque
pixels laid over whole is copied. An image drawn one texel per device pixel lands on the window pixel `floor(x +
0.5)` (`cupertino_raster_math::placement`), whatever piece of the window is painted, and one whose whole source
pixels fill a destination on whole pixels is laid over by a direct loop with the pattern fill's arithmetic and
rounding (the same bytes, under a clip and at four scales), its premultiplied
pixels read where they are. A group whose opacity
rounds to nothing (a panel fading in from 0) changes no pixel, so nothing of its subtree is painted.

A paint node marks where a renderer may keep a layer (`PaintNode::layer`): `Layer::Scroll` is a scroll container's
content with its drawn offset, `Layer::Opacity` a translucent group that may change its opacity, `Layer::Transform` a
subtree whose transform animates (a popover growing out of its anchor) and whose group may change its opacity. `scroll_layers` says
which scroll layers a renderer keeps (a clip, a translation alone, no group or kept layer above), and the damage
tracker uses the same rule. The CPU renderer keeps each such layer as an image of its content in the content's own
pixels, its rows a ring, so a vertical scroll moves no pixel and paints only the rows that come into view and what
changed in the layer; a horizontal scroll or a change of the viewport paints it whole. A layer new to a frame is laid
over only where the frame is damaged, as its nodes are all new to it and damage what they draw. Inside a layer each node's
scratch image follows the node's own extent, never the damage or the image; a group in it is painted with all of its
subtree, whatever rows are painted, and what a group or a layer of effects reads is cut at the window's edges moved by
the layer's offset, as painting in place cuts it, so a blur carries in the same pixels; and a node too large for one is painted
in 1024-pixel squares fixed in the content, so a row painted as it scrolls in has the bytes of the same row in a layer
painted whole, at scales 1, 1.5 and 2, scrolling up, down, sideways and past the viewport. The layer is laid over the frame where every clip
above it lets whole pixels through; at a fractional clip edge its content is painted in place, as without layers, so
the only difference from no layer is the rounding of laying the content over at once, within 2 of 255.
Where the frame under the layer is still only to be set to one colour (the window's background, waiting as a span
write), the layer is laid over that colour as it is written, in one pass: an empty run writes the colour, an opaque
run is copied, and the background is not written first (`scroll_on_background` counts it; the bytes are those of the
background written and the layer laid over it).
Kept layers hold at most the bytes of one frame and 8 MiB more (`LAYER_SLACK_BYTES`); a layer past that is not kept and
is painted every frame. `UiRenderer::drop_layers` lets them all go
and keeps the frame; the toolkit calls it when its window is suspended. A group marked `Layer::Opacity` or
`Layer::Transform` that is painted into the frame is kept as the image of its subtree with its effects, before its
opacity and the clips above it, within the same cap; while its subtree is the same (each node's revision, transform,
clip, group and box) and it lies where it was painted, it is laid over from that image at its opacity and its subtree
is not painted (`groups_kept`), with the bytes of a frame that paints it, at scales 1, 1.5 and 2. A group whose
transform changed is painted again: laid over moved, even by whole device pixels, the image would not put its paths'
edges and its dithering on the pixels painting them there does, and scaled it would be resampled.

`UiRenderer::render_into` draws a frame into memory the renderer is lent (`MemoryTarget`: a window's shared-memory
buffer, its stride and the rectangles in which it differs from the frame drawn last, `None` when its pixels are
unknown) instead of its own image: the frame's damage and what the memory missed, all of it when unknown, painted over
what the memory holds, so it ends with the bytes the copy path leaves in a buffer (`render`, then the damage and what
the buffer missed copied over), new ones, one held for frames on end
and ones of unknown pixels at scales 1, 1.5 and 2. Its kept layers and scratch images stay its own; its own image is let
go, and a frame drawn with `render` after it is drawn whole. `UiRenderer::draws_into_memory` says whether a renderer
can (the CPU renderer; not the GPU renderer, whose frames are copied on the device).

The frame's pixels are kept from one frame to the next; a frame drawn whole clears or paints over every one of them, so
they are not cleared beforehand as well. Pixels of a new size (a window's first frame) are allocated zeroed, which the
system gives without writing them, and the frame's clear then writes nothing, so a transparent window touches only the
pixels it paints.

A drop or inner shadow is one coverage byte a pixel until it is tinted: the layer's alpha, shifted, blurred by three box
passes on each axis (rows in place, columns a row at a time with a running sum per column), then tinted and laid under
or over the layer, skipping the pixels it cannot change. It gives the same bytes as blurring all four channels of the
silhouette, 1.5 and 2, for about a tenth of the work: at scale 2 a panel's shadow
spans millions of pixels, and it was most of the frame.

Glyph rasters are keyed by their pixel size, so one renderer drawing a scene at 1, 1.25, 1.5, 1.75 and 2 and back gives
each frame the bytes a fresh renderer gives at that scale; an opaque rectangle whose edges
lie on device pixel edges covers whole pixels with nothing half covered beside it, and text has ink at every scale.

Two fast paths give the same bytes as that general path. A node painted through a scratch image, under clips that are all axis-aligned rectangles, whose large fills are
solid rectangles or rounded rectangles, has pixels its fills and clips cover fully and its other operations (smaller
fills, strokes, images, glyph rasters) do not reach. Those pixels hold one colour in the scratch image: the renderer
writes them as rows, copied when the colour is opaque and blended with the scratch image's own integer source-over
otherwise, and paints the rest through the scratch image with those pixels cut out of the fills, as holes on whole
pixels that change no other pixel's coverage. Span writes to the frame wait until something else paints there, so layers
of one colour each over the same pixels (the cleared rectangle, the window's background, a page's background, a
translucent group over it) collapse into one write. And when the last node whose opaque fill covers a whole damaged
rectangle has no group above it, nothing painted before it there is painted at all. On a 3840×2160 frame each pass over
the pixels costs a few milliseconds of memory traffic, so these paths matter most for large damage.

`CpuRenderer::stats` counts what the last frame did (dirty pixels, nodes and groups painted, primitives, scratch and layer
allocations, clip masks, and each copy site with its bytes: clear, node, group and layer composites, crops, mask
application, cache fills, retained caches), the rectangles the span path wrote (`spans`) and the nodes skipped under an
opaque fill (`hidden`), the pixels painted into kept layers (`layer_pixels`), their composite (`scroll_composite`, of
it `scroll_on_background`), the groups laid over from their kept images (`groups_kept`) and the layers kept
(`retained_layers`), so a timing run can tell a slower frame from a heavier one. The opt-in


Dev builds compile this crate and tiny-skia optimised (the workspace `Cargo.toml`, explained in `CONTRIBUTING.md`), so a
dev binary draws several times faster than unoptimised; debug assertions and overflow checks stay on.

## GPU renderer (`gpu`)

`gpu::GpuRenderer` implements `UiRenderer` over a `cupertino-gpu` device and draws a window's `DisplayList` into a
texture the presenter hands to the compositor. It keeps the CPU renderer's results: the CPU renderer is the standard it
follows (every scene within 2/255 per channel) and the fallback.

What it draws, and how:

- Rectangles and rounded rectangles, filled and stroked, by their distance in the fragment program, with the shared
  coverage, stroke and rounding of `cupertino-raster-math`.
- Linear and radial gradients of up to eight stops, interpolated and dithered as the CPU renderer does, by the window
  pixel.
- Glyph runs from two atlases: coverage glyphs tinted by the run's colour, colour glyphs laid as they are.
- Images and icons from textures kept by revision under a byte budget, sampled nearest or bilinear inside their
  crop, edges covered as the CPU renderer covers them; one image pixel to one window pixel is laid unfiltered, on the
  window pixel `floor(x + 0.5)` both renderers take (`cupertino_raster_math::placement`).
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

A scroll layer (`Layer::Scroll`, kept as `cupertino_ui_render::scroll_layers` says) keeps its content between frames
in a texture the size of its viewport, painted under none of its clips and dithered by its own pixel (the window
pixel moved by the layer's offset). A frame that moves it by whole device pixels draws what stays in view into the
layer's second texture, moved, and paints only the rows or columns that come into view and what the damage names; a
move by a part of a pixel, a new viewport or a full frame paints it whole. The window lays the texture over where
every clip around the layer lets pixels through whole and paints the content in place along a clip's partly covered
edge, as the CPU renderer composites its kept images. The layers' textures stay within twice the window's bytes and
16 MiB more; `trim` lets them go. The device counts the textures it made and has not destroyed
(`Device::live_textures`): a dropped renderer gives back every one, and one trimmed keeps its frame alone.

A window draws with `WindowRenderer`, a `UiRenderer` whose frames are `CpuFrame`s with empty `pixels` and the
`WindowTexture` in `device`: the window-sized texture, shared between the renderer and its frames, which the
presenter on the same device copies into its buffers and anything else reads back (`CpuFrame::in_memory`). The
programs (`GpuPrograms`) are made once per device and shared by every renderer on it (`GpuRenderer::with_programs`).

Both renderers are meant to agree on the sample scenes and on generated scenes of each primitive group at scales
1, 1.5 and 2 on odd sizes, whole and repainting damage, and with scroll layers scrolled frame by frame against the
same frames drawn whole. The GPU renderer needs an EGL device: the first render node, or the surfaceless platform;
`CUPERTINO_GPU_REQUIRED=1` makes a missing or software device a failure, `CUPERTINO_EGL_REQUIRED=1` a missing one (a
run on Mesa's software rasteriser).
