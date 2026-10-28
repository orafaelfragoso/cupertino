# cupertino-ui-render

CupertinoKit rendering: the `DisplayList` of paint nodes core builds for each window (paths, gradients, images, glyph
runs, clipped and isolated layers, materials already resolved to plain paint), `PaintResources`, `DamageRegion`, and
the `UiRenderer` trait whose CPU frame (`CpuFrame`, premultiplied RGBA) the window system presents. MPL-2.0. `CpuRenderer` is the CPU backend (tiny-skia, private): it validates a request before touching the image it keeps, repaints
only the damage (an incremental repaint equals a fresh one), executes clips, group opacity, blur and shadows on isolated
layers, and caches rasterized glyphs and decoded images under explicit byte budgets.
