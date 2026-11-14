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
//! * [`glass`]: the glass model (stages of the background pass, foreground matrix) and its uniform block; the WGSL
//!   and GLSL ES shaders follow it.
//! * [`gpu`]: headless helpers (adapter, upload/read-back, procedural wallpaper, PSNR).
//!
//! Recipes come from `cupertino-theme` (decoded layer archives). The GLSL ES 3.00 port of every
//! shader lives in `shaders/gles/`; see the README.

pub mod cpu;
pub mod display_filter;
pub mod glass;
pub mod gpu;
pub mod ops;
pub mod renderer;
pub mod vibrant;

pub use renderer::{FrameDesc, MaterialRegion, MaterialRenderer, RenderStats, TargetInit};
pub use vibrant::{VibrantDraw, VibrantMode, VibrantStyle};

/// GLSL ES 3.00 sources (vertex, fragment) of the ported shaders, by name.
pub mod gles {
    pub const FULLSCREEN_VERT: &str = include_str!("../shaders/gles/fullscreen.vert");
    pub const DOWNSAMPLE_FRAG: &str = include_str!("../shaders/gles/downsample.frag");
    pub const BLUR_FRAG: &str = include_str!("../shaders/gles/blur.frag");
    pub const COLOR_FRAG: &str = include_str!("../shaders/gles/color.frag");
    pub const BLIT_FRAG: &str = include_str!("../shaders/gles/blit.frag");
    pub const MATERIAL_VERT: &str = include_str!("../shaders/gles/material.vert");
    pub const MATERIAL_FRAG: &str = include_str!("../shaders/gles/material.frag");
    pub const VIBRANT_VERT: &str = include_str!("../shaders/gles/vibrant.vert");
    pub const VIBRANT_FRAG: &str = include_str!("../shaders/gles/vibrant.frag");
    pub const GLASS_VERT: &str = include_str!("../shaders/gles/glass.vert");
    pub const GLASS_FRAG: &str = include_str!("../shaders/gles/glass.frag");

    /// `(file name, is fragment stage, source)` for every shader.
    pub const ALL: [(&str, bool, &str); 11] = [
        ("fullscreen.vert", false, FULLSCREEN_VERT),
        ("downsample.frag", true, DOWNSAMPLE_FRAG),
        ("blur.frag", true, BLUR_FRAG),
        ("color.frag", true, COLOR_FRAG),
        ("blit.frag", true, BLIT_FRAG),
        ("material.vert", false, MATERIAL_VERT),
        ("material.frag", true, MATERIAL_FRAG),
        ("vibrant.vert", false, VIBRANT_VERT),
        ("vibrant.frag", true, VIBRANT_FRAG),
        ("glass.vert", false, GLASS_VERT),
        ("glass.frag", true, GLASS_FRAG),
    ];
}
