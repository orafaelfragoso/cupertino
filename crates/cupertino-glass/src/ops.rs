//! Flat GPU representation of a recipe: colour filter operations, fill layers and the uniform
//! blocks shared with the shaders (std140-compatible, 16-byte aligned members only).

use cupertino_raster_math::blur::MAX_BLUR_RADIUS;
use cupertino_theme::recipe::{curve_coeffs, ColorOp, Recipe};

pub const MAX_OPS: usize = 8;
pub const MAX_FILLS: usize = 8;

pub const OP_SATURATE: f32 = 1.0;
pub const OP_LUM_CURVE: f32 = 2.0;
pub const OP_MONOCHROME: f32 = 3.0;
pub const OP_LINEAR_MATRIX: f32 = 4.0;
pub const OP_INVERT: f32 = 5.0;
pub const OP_CONTRAST: f32 = 6.0;
/// Second slot of [`OP_LINEAR_MATRIX`]; drawn alone it changes nothing.
pub const OP_MATRIX_TAIL: f32 = 7.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Op {
    pub a: [f32; 4],
    pub b: [f32; 4],
}

/// Colour filter chain: `a = [kind, amount, 0, 0]`, `b` = parameters (luminance curve: Horner
/// coefficients; monochrome: colour; contrast: `a.y` is the contrast). A linear matrix takes two
/// slots: `[kind, m00, m01, m02]`, `[m10, m11, m12, m20]`, then `[tail, m21, m22, 0]`. The chain
/// stops before an operation that no longer fits in [`MAX_OPS`] slots.
pub fn color_ops(ops: &[ColorOp]) -> Vec<Op> {
    let mut out = Vec::new();
    for o in ops {
        let packed: Vec<Op> = match o {
            ColorOp::Saturate(s) => vec![Op {
                a: [OP_SATURATE, *s, 0.0, 0.0],
                b: [0.0; 4],
            }],
            ColorOp::LumCurve(v, amt) => vec![Op {
                a: [OP_LUM_CURVE, *amt, 0.0, 0.0],
                b: curve_coeffs(*v),
            }],
            ColorOp::Monochrome { color, amount } => vec![Op {
                a: [OP_MONOCHROME, *amount, 0.0, 0.0],
                b: [color[0], color[1], color[2], 0.0],
            }],
            ColorOp::LinearMatrix(m) => vec![
                Op {
                    a: [OP_LINEAR_MATRIX, m[0][0], m[0][1], m[0][2]],
                    b: [m[1][0], m[1][1], m[1][2], m[2][0]],
                },
                Op {
                    a: [OP_MATRIX_TAIL, m[2][1], m[2][2], 0.0],
                    b: [0.0; 4],
                },
            ],
            ColorOp::Invert => vec![Op {
                a: [OP_INVERT, 0.0, 0.0, 0.0],
                b: [0.0; 4],
            }],
            ColorOp::Contrast(c) => vec![Op {
                a: [OP_CONTRAST, *c, 0.0, 0.0],
                b: [0.0; 4],
            }],
        };
        if out.len() + packed.len() > MAX_OPS {
            break;
        }
        out.extend(packed);
    }
    out
}

/// Fill layers: `a = [blend id, tint flag, 0, 0]`, `b` = straight rgba.
pub fn fill_ops(r: &Recipe) -> Vec<Op> {
    r.fills
        .iter()
        .take(MAX_FILLS)
        .map(|f| Op {
            a: [
                f.blend as u32 as f32,
                if f.tint_layer { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
            b: f.rgba,
        })
        .collect()
}

/// Uniform block of the backdrop passes (`shaders/include/pass.wgsl`, `PassParams`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PassParams {
    /// src w, src h, dst w, dst h
    pub sizes: [i32; 4],
    /// dir x, dir y, blur radius, number of colour ops
    pub dir_radius: [i32; 4],
    pub weights: [[f32; 4]; 9],
    pub ops: [Op; MAX_OPS],
    /// [`PASS_DITHER`] when the blur's result is dithered by its pixel before an 8-bit store; [`PASS_KEEP_ALPHA`] when
    /// the downsample and the blur keep their source's alpha; 0, 0
    pub flags: [i32; 4],
}

/// `PassParams::flags[0]`: the blur dithers its result by pixel position (its target holds 8 bits a channel).
pub const PASS_DITHER: i32 = 1;
/// `PassParams::flags[1]`: the downsample and the blur keep the premultiplied alpha of what they blur (a layer's content,
/// transparent around it) instead of writing an opaque backdrop; the blur's colour stays within its alpha.
pub const PASS_KEEP_ALPHA: i32 = 1;

impl PassParams {
    pub fn new(src: (u32, u32), dst: (u32, u32)) -> Self {
        let mut p: PassParams = bytemuck::Zeroable::zeroed();
        p.sizes = [src.0 as i32, src.1 as i32, dst.0 as i32, dst.1 as i32];
        p
    }

    pub fn set_weights(&mut self, w: &[f32]) {
        self.weights = [[0.0; 4]; 9];
        for (i, v) in w.iter().take(MAX_BLUR_RADIUS + 1).enumerate() {
            self.weights[i / 4][i % 4] = *v;
        }
    }

    pub fn set_ops(&mut self, ops: &[Op]) {
        self.ops = [Op::default(); MAX_OPS];
        for (i, o) in ops.iter().take(MAX_OPS).enumerate() {
            self.ops[i] = *o;
        }
        self.dir_radius[3] = ops.len().min(MAX_OPS) as i32;
    }
}

/// Uniform block of a material region (`shaders/material.wgsl`, `Region`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RegionParams {
    pub rect: [f32; 4],
    /// corner extent (px), superellipse exponent, opacity, AA width (px)
    pub shape: [f32; 4],
    /// number of fills, has backdrop, 0, 0
    pub info: [f32; 4],
    /// target w, h, source w, h
    pub sizes: [f32; 4],
    pub tint: [f32; 4],
    pub fills: [[f32; 4]; 16],
}

impl RegionParams {
    pub fn set_fills(&mut self, fills: &[Op]) {
        self.fills = [[0.0; 4]; 16];
        for (i, f) in fills.iter().take(MAX_FILLS).enumerate() {
            self.fills[2 * i] = f.a;
            self.fills[2 * i + 1] = f.b;
        }
        self.info[0] = fills.len().min(MAX_FILLS) as f32;
    }
}

/// Uniform block of where a region program (material, glass) draws (`shaders/include/place.wgsl`, `Placement`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Placement {
    /// The frame's projection from target pixels to clip space, column-major, each column padded to four floats.
    pub proj: [[f32; 4]; 3],
    /// x, y, w, h of the quad drawn, in target pixels.
    pub quad: [f32; 4],
    /// [`PLACE_DITHER`] when the result is dithered by its pixel before an 8-bit store; 0, 0, 0
    pub flags: [i32; 4],
}

/// `Placement::flags[0]`: the program dithers its result by pixel position.
pub const PLACE_DITHER: i32 = 1;

impl Placement {
    /// The placement of `quad` through the column-major 3 x 3 `projection`, dithered or not.
    pub fn new(projection: &[f32; 9], quad: [f32; 4], dither: bool) -> Self {
        let column = |c: usize| {
            [
                projection[3 * c],
                projection[3 * c + 1],
                projection[3 * c + 2],
                0.0,
            ]
        };
        Self {
            proj: [column(0), column(1), column(2)],
            quad,
            flags: [if dither { PLACE_DITHER } else { 0 }, 0, 0, 0],
        }
    }
}

/// Uniform block of a mesh draw (`shaders/mesh.wgsl`, `Mesh`): a window snapshot drawn through an effect mesh.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshUniform {
    /// The frame's projection from output pixels to clip space, column-major, each column padded to four floats.
    pub proj: [[f32; 4]; 3],
    /// The element's opacity, 0, 0, 0.
    pub alpha: [f32; 4],
}

impl MeshUniform {
    /// The block of a draw through the column-major 3 x 3 `projection` at `alpha`.
    pub fn new(projection: &[f32; 9], alpha: f32) -> Self {
        let place = Placement::new(projection, [0.0; 4], false);
        Self {
            proj: place.proj,
            alpha: [alpha, 0.0, 0.0, 0.0],
        }
    }
}

/// Uniform block of a vibrant glyph draw (`vibrant.wgsl`, `Vibrant`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VibrantUniform {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub sizes: [f32; 4],
    pub mode: [f32; 4],
    pub c0: [f32; 4],
    pub c1: [f32; 4],
}
