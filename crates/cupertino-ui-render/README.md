# cupertino-ui-render

CupertinoKit rendering: the `DisplayList` of paint nodes core builds for each window (paths, gradients, images, glyph
runs, clipped and isolated layers, materials already resolved to plain paint), `PaintResources`, `DamageRegion`, and
the theme's `Color` it paints with (re-exported for the window-system interface), the `UiRenderer` trait whose frame (`CpuFrame`, premultiplied RGBA) the window system presents; a frame drawn on a GPU keeps its image on the device (`CpuFrame::device`, a `DeviceFrame`) and `CpuFrame::in_memory` reads it back. MPL-2.0. `CpuRenderer` is the CPU backend (tiny-skia, private): it validates a request before touching the image it keeps, repaints
only the damage, executes clips, group opacity, blur and shadows on isolated
layers, and caches rasterized glyphs and decoded images under explicit byte budgets. A frame drawn over the last one
has the bytes of the same list drawn whole, which a test checks on random lists, changes and damage at scales 1, 1.25,
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
channel arithmetic, which a test checks on every value and on random pixels), skips runs of 16 empty pixels (or of
pixels a mask covers fully) read a word at a time, and has no branch per pixel, so it vectorizes: a mostly empty layer
the size of a 2x window, such as a panel's outline, is laid over in a fraction of a millisecond. A group whose opacity
rounds to nothing (a panel fading in from 0) changes no pixel, so nothing of its subtree is painted.

A paint node marks where a renderer may keep a layer (`PaintNode::layer`): `Layer::Scroll` is a scroll container's
content with its drawn offset, `Layer::Opacity` a translucent group that may change its opacity. `scroll_layers` says
which scroll layers a renderer keeps (a clip, a translation alone, no group or kept layer above), and the damage
tracker uses the same rule. The CPU renderer keeps each such layer as an image of its content in the content's own
pixels, its rows a ring, so a vertical scroll moves no pixel and paints only the rows that come into view and what
changed in the layer; a horizontal scroll or a change of the viewport paints it whole. A layer new to a frame is laid
over only where the frame is damaged, as its nodes are all new to it and damage what they draw. Inside a layer each node's
scratch image follows the node's own extent, never the damage or the image, and a node too large for one is painted
in 1024-pixel squares fixed in the content, so a row painted as it scrolls in has the bytes of the same row in a layer
painted whole: a test scrolls random scenes up, down, sideways and past the viewport at scales 1, 1.5 and 2, changing
nodes on the way, and checks every frame against one drawn whole. The layer is laid over the frame where every clip
above it lets whole pixels through; at a fractional clip edge its content is painted in place, as without layers, so
the only difference from no layer is the rounding of laying the content over at once, within the goldens' 2 of 255.
Kept layers hold at most the bytes of one frame and 8 MiB more (`LAYER_SLACK_BYTES`); a layer past that is not kept and
is painted every frame, which a test checks with five window-sized layers. `UiRenderer::drop_layers` lets them all go
and keeps the frame; the toolkit calls it when its window is suspended. Opacity layers are marked but not kept yet.

The frame's pixels are kept from one frame to the next; a frame drawn whole clears or paints over every one of them, so
they are not cleared beforehand as well. Pixels of a new size (a window's first frame) are allocated zeroed, which the
system gives without writing them, and the frame's clear then writes nothing, so a transparent window touches only the
pixels it paints.

A drop or inner shadow is one coverage byte a pixel until it is tinted: the layer's alpha, shifted, blurred by three box
passes on each axis (rows in place, columns a row at a time with a running sum per column), then tinted and laid under
or over the layer, skipping the pixels it cannot change. It gives the same bytes as blurring all four channels of the
silhouette, which a test checks at scales 1, 1.5 and 2, for about a tenth of the work: at scale 2 a panel's shadow
spans millions of pixels, and it was most of the frame.

Glyph rasters are keyed by their pixel size, so one renderer drawing a scene at 1, 1.25, 1.5, 1.75 and 2 and back gives
each frame the bytes a fresh renderer gives at that scale; a test checks that, and that an opaque rectangle whose edges
lie on device pixel edges covers whole pixels with nothing half covered beside it, and that text has ink at every scale.

Two fast paths give the same bytes as that general path, which a test checks on random scenes at several scales and
damages. A node painted through a scratch image, under clips that are all axis-aligned rectangles, whose large fills are
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
opaque fill (`hidden`), the pixels painted into kept layers (`layer_pixels`), their composite (`scroll_composite`)
and the layers kept (`retained_layers`), so a timing run can tell a slower frame from a heavier one. The opt-in
`tests/render_matrix.rs` prints cold, steady and full-repaint timings with those counters for 1, 10 and 100 rows at
scale 1 and 2; run it with `--release --ignored --nocapture` in an exclusive slot.

Dev builds compile this crate and tiny-skia optimised (the workspace `Cargo.toml`, explained in `CONTRIBUTING.md`), so a
dev binary or test draws several times faster than unoptimised; debug assertions and overflow checks stay on.
