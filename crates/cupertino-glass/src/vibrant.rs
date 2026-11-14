//! Parameters for vibrant glyph drawing (`shaders/vibrant.wgsl`).
//!
//! The blend functions are as designed.

use crate::ops::VibrantUniform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VibrantMode {
    /// `lightenSover(s·c1, colorDodge(s·c0, d))`, for dark materials.
    Dark,
    /// `darkenSover(s·c1, colorBurn(s·c0, d))`, for light materials.
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VibrantStyle {
    pub mode: VibrantMode,
    /// Swap the nesting order of the two blend functions.
    pub reversed: bool,
    /// unknown placeholder: colour dodge / burn multiplier.
    pub c0: [f32; 4],
    /// unknown placeholder: lighten / darken multiplier.
    pub c1: [f32; 4],
}

impl VibrantStyle {
    pub fn new(mode: VibrantMode) -> Self {
        Self {
            mode,
            reversed: false,
            c0: [1.0; 4],
            c1: [1.0; 4],
        }
    }
}

/// One glyph run: a premultiplied template texture drawn into `rect` (target pixels, y down),
/// sampling `uv = [u0, v0, u1, v1]` of the texture.
pub struct VibrantDraw<'a> {
    pub glyphs: &'a cupertino_gpu::Texture,
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub style: VibrantStyle,
}

impl<'a> VibrantDraw<'a> {
    pub fn new(glyphs: &'a cupertino_gpu::Texture, rect: [f32; 4], style: VibrantStyle) -> Self {
        Self {
            glyphs,
            rect,
            uv: [0.0, 0.0, 1.0, 1.0],
            style,
        }
    }

    pub(crate) fn uniform(&self, target: (u32, u32)) -> VibrantUniform {
        VibrantUniform {
            rect: self.rect,
            uv: self.uv,
            sizes: [target.0 as f32, target.1 as f32, 0.0, 0.0],
            mode: [
                if self.style.mode == VibrantMode::Light {
                    1.0
                } else {
                    0.0
                },
                if self.style.reversed { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
            c0: self.style.c0,
            c1: self.style.c1,
        }
    }
}
