# cupertino-raster-math

The one definition of the drawing maths every Cupertino renderer and shader implements, with test vectors. The
modules are listed in the crate documentation. MPL-2.0.

## glyph

Glyph rasterisation for Cupertino text: an outline is dilated (stem darkening) by edge translation and scan-converted to
8-bit coverage by exact area. `Smoothing::On` is the default. The window decorations' text renderer and the CupertinoKit
glyph rasteriser (`cupertino-ui-text`) both call it, so the formulas and their parameters have one source.
