# cupertino-ui-render

CupertinoKit rendering: the `DisplayList` of paint nodes core builds for each window (paths, gradients, images, glyph
runs, clipped and isolated layers, materials already resolved to plain paint), `PaintResources`, `DamageRegion`, and
the `UiRenderer` trait whose CPU frame (`CpuFrame`, premultiplied RGBA) the window system presents. MPL-2.0. Today the
crate declares the public types; the CPU backend follows.
