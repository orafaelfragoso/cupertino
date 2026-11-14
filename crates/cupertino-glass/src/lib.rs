//! Material rendering for the Cupertino compositor.
//!
//! * [`renderer::MaterialRenderer`]: draws N material regions per frame on an external wgpu device,
//!   with a damage-generation keyed backdrop cache and an SDF rounded mask (circular or
//!   continuous corners from `cupertino-geometry`).
//! * [`vibrant`]: parameters for the vibrant blend functions used to draw template glyphs and text
//!   on materials (`VibrantDraw`, `MaterialRenderer::draw_vibrant`).
//! * [`cpu`]: CPU reference implementation of the same pipeline for validation.
//! * [`ops`]: the flat GPU encoding of recipes; the blur plan is `cupertino_raster_math::blur`.
//! * [`display_filter`]: colour filters, invert and contrast of the whole display as a colour chain.
//! * [`glass`]: the glass model (stages of the background pass, foreground matrix) and its uniform block; the glass
//!   program follows it.
//! * [`gpu`]: headless helpers (adapter, upload/read-back, procedural wallpaper, PSNR).
//!
//! * [`shaders`]: the generated programs (the backdrop passes, the material region, the glass composite and the
//!   vibrant glyphs), each written once in WGSL; the compositor links their GLSL.
//!
//! Recipes come from `cupertino-theme` (decoded layer archives).

pub mod cpu;
pub mod display_filter;
pub mod glass;
pub mod gpu;
pub mod ops;
pub mod renderer;
pub mod shaders;
pub mod vibrant;

pub use renderer::{FrameDesc, MaterialRegion, MaterialRenderer, RenderStats, TargetInit};
pub use vibrant::{VibrantDraw, VibrantMode, VibrantStyle};
