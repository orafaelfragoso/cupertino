# cupertino-ui-render

CupertinoKit rendering: the `DisplayList` of paint nodes core builds for each window (paths, gradients, images, glyph
runs, clipped and isolated layers, materials already resolved to plain paint), `PaintResources`, `DamageRegion`, and
the `UiRenderer` trait whose CPU frame (`CpuFrame`, premultiplied RGBA) the window system presents. MPL-2.0. `CpuRenderer` is the CPU backend (tiny-skia, private): it validates a request before touching the image it keeps, repaints
only the damage, executes clips, group opacity, blur and shadows on isolated
layers, and caches rasterized glyphs and decoded images under explicit byte budgets. An incremental repaint matches a
fresh one within a few levels: a group's layer covers the pixels its descendants' blurs and shadows read, but an
anti-aliased curved or fractional edge cut by a damaged rectangle's border is sampled on a slightly different sub-pixel
grid.

A drop or inner shadow is one coverage byte a pixel until it is tinted: the layer's alpha, shifted, blurred by three box
passes on each axis (rows in place, columns a row at a time with a running sum per column), then tinted and laid under
or over the layer, skipping the pixels it cannot change. It gives the same bytes as blurring all four channels of the
silhouette, which a test checks at scales 1, 1.5 and 2, for about a tenth of the work: at scale 2 a panel's shadow
spans millions of pixels, and it was most of the frame.

Glyph rasters are keyed by their pixel size, so one renderer drawing a scene at 1, 1.25, 1.5, 1.75 and 2 and back gives
each frame the bytes a fresh renderer gives at that scale; a test checks that, and that an opaque rectangle whose edges
lie on device pixel edges covers whole pixels with nothing half covered beside it, and that text has ink at every scale.

Two fast paths give the same bytes as that general path, which a test checks on random scenes at several scales and
damages. A node the general path would paint through a scratch image (one that a clip above it or the damage cuts),
under clips that are all axis-aligned rectangles, whose large fills are solid rectangles or rounded rectangles, has
pixels its fills and clips cover fully and its other operations (smaller fills, strokes, images, glyph rasters) do not
reach. Those pixels hold one colour in the scratch image: the renderer writes them as rows, copied when the colour is
opaque and blended with the scratch image's own integer source-over otherwise, and paints the rest through the scratch
image with those pixels cut out of the fills, as holes on whole pixels that change no other pixel's coverage. Span
writes to the frame wait until something else paints there, so layers of one colour each over the same pixels (the
cleared rectangle, the window's background, a page's background, a translucent group over it) collapse into one write.
And when the last node whose opaque fill covers a whole damaged rectangle has no group above it, nothing painted before
it there is painted at all. On a 3840×2160 frame each pass over the pixels costs a few milliseconds of memory traffic,
so these paths matter most for large damage.

`CpuRenderer::stats` counts what the last frame did (dirty pixels, nodes and groups painted, primitives, scratch and layer
allocations, clip masks, and each copy site with its bytes: clear, node, group and layer composites, crops, mask
application, cache fills, retained caches), the rectangles the span path wrote (`spans`) and the nodes skipped under an
opaque fill (`hidden`), so a timing run can tell a slower frame from a heavier one. The opt-in
`tests/render_matrix.rs` prints cold, steady and full-repaint timings with those counters for 1, 10 and 100 rows at
scale 1 and 2; run it with `--release --ignored --nocapture` in an exclusive slot.
