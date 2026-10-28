# cupertino-ui-render

CupertinoKit rendering: the `DisplayList` of paint nodes core builds for each window (paths, gradients, images, glyph
runs, clipped and isolated layers, materials already resolved to plain paint), `PaintResources`, `DamageRegion`, and
the `UiRenderer` trait whose CPU frame (`CpuFrame`, premultiplied RGBA) the window system presents. MPL-2.0. `CpuRenderer` is the CPU backend (tiny-skia, private): it validates a request before touching the image it keeps, repaints
only the damage (an incremental repaint equals a fresh one), executes clips, group opacity, blur and shadows on isolated
layers, and caches rasterized glyphs and decoded images under explicit byte budgets.

`CpuRenderer::stats` counts what the last frame did (dirty pixels, nodes and groups painted, primitives, scratch and layer
allocations, clip masks, and each copy site with its bytes: clear, node, group and layer composites, crops, mask
application, cache fills, retained caches), so a timing run can tell a slower frame from a heavier one. The opt-in
`tests/render_matrix.rs` prints cold, steady and full-repaint timings with those counters for 1, 10 and 100 rows at
scale 1 and 2; run it with `--release --ignored --nocapture` in an exclusive slot.
