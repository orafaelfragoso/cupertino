//! The integration tests of `cupertino-raster-math`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "blur-shadow.rs"]
mod blur_shadow;
mod coverage;
#[path = "dither-gradient.rs"]
mod dither_gradient;
mod glyph;
mod vectors;
