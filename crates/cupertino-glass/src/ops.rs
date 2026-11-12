//! Flat GPU representation of a recipe: colour filter operations, fill layers and the uniform
//! blocks shared with the shaders (std140-compatible, 16-byte aligned members only).

use crate::blur::MAX_BLUR_RADIUS;
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

/// Uniform block of the backdrop passes (`passes.wgsl`, `PassParams`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PassParams {
    /// src w, src h, dst w, dst h
    pub sizes: [i32; 4],
    /// dir x, dir y, blur radius, number of colour ops
    pub dir_radius: [i32; 4],
    pub weights: [[f32; 4]; 9],
    pub ops: [Op; MAX_OPS],
}

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

/// Uniform block of a material region (`material.wgsl`, `Region`).
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

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_theme::{Appearance, MaterialOptions, Theme};

    fn recipe(a: Appearance, m: &str) -> Recipe {
        Theme::load()
            .unwrap()
            .recipe(a, m, &MaterialOptions::default())
            .unwrap()
    }

    #[test]
    fn uniform_sizes_match_shader_layout() {
        assert_eq!(std::mem::size_of::<PassParams>(), 16 + 16 + 144 + 256);
        assert_eq!(std::mem::size_of::<RegionParams>(), 5 * 16 + 256);
        assert_eq!(std::mem::size_of::<VibrantUniform>(), 96);
        assert_eq!(std::mem::size_of::<Op>(), 32);
    }

    #[test]
    fn menu_bar_recipe_to_ops() {
        let r = recipe(Appearance::Light, "MenuBar");
        let ops = color_ops(&r.backdrop.as_ref().unwrap().ops);
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].a[0], OP_LUM_CURVE);
        assert!((ops[0].a[1] - 0.08).abs() < 1e-5);
        let c = curve_coeffs([0.477027, 0.484363, 0.538031, 0.876911]);
        for (got, want) in ops[0].b.iter().zip(c) {
            assert!((got - want).abs() < 1e-4);
        }
        assert_eq!(ops[1].a[0], OP_SATURATE);
        assert!((ops[1].a[1] - 1.2).abs() < 1e-5);
        assert!(fill_ops(&r).is_empty());
    }

    #[test]
    fn sidebar_recipe_fill_ops() {
        let r = recipe(Appearance::Light, "Sidebar");
        let f = fill_ops(&r);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].a[0], 0.0, "normal blend id");
        assert_eq!(f[0].a[1], 0.0, "no tint layer");
        assert!((f[0].b[3] - 0.98).abs() < 1e-3);
    }

    #[test]
    fn dark_sidebar_is_one_normal_fill() {
        let f = fill_ops(&recipe(Appearance::Dark, "Sidebar"));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].a[0], 0.0);
        assert!((f[0].b[3] - 0.966).abs() < 1e-3);
    }

    #[test]
    fn selection_recipe_has_monochrome_or_fills() {
        let r = recipe(Appearance::Light, "MenuBarSelection");
        let f = fill_ops(&r);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].a[0], 1.0, "plus-lighter id");
    }

    #[test]
    fn region_fill_packing() {
        let mut p: RegionParams = bytemuck::Zeroable::zeroed();
        let ops = fill_ops(&recipe(Appearance::Light, "InlineSidebar"));
        assert_eq!(ops.len(), 3);
        p.set_fills(&ops);
        assert_eq!(p.info[0], 3.0);
        assert_eq!(p.fills[2], ops[1].a);
        assert_eq!(p.fills[3], ops[1].b);
    }

    #[test]
    fn a_linear_matrix_takes_two_slots_and_the_chain_stops_when_full() {
        let m = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        let ops = color_ops(&[
            ColorOp::LinearMatrix(m),
            ColorOp::Invert,
            ColorOp::Contrast(0.25),
        ]);
        assert_eq!(
            ops[0],
            Op {
                a: [OP_LINEAR_MATRIX, 1.0, 2.0, 3.0],
                b: [4.0, 5.0, 6.0, 7.0]
            }
        );
        assert_eq!(ops[1].a, [OP_MATRIX_TAIL, 8.0, 9.0, 0.0]);
        assert_eq!(
            (ops[2].a[0], ops[3].a),
            (OP_INVERT, [OP_CONTRAST, 0.25, 0.0, 0.0])
        );
        let full = color_ops(
            &[
                vec![ColorOp::Invert; MAX_OPS - 1],
                vec![ColorOp::LinearMatrix(m)],
            ]
            .concat(),
        );
        assert_eq!(full.len(), MAX_OPS - 1, "a matrix never loses its tail");
    }

    #[test]
    fn pass_weight_packing() {
        let mut p = PassParams::new((10, 10), (5, 5));
        p.set_weights(&[0.5, 0.25, 0.125, 0.0625, 0.03, 0.01]);
        assert_eq!(p.weights[0], [0.5, 0.25, 0.125, 0.0625]);
        assert_eq!(p.weights[1][0], 0.03);
        assert_eq!(p.sizes, [10, 10, 5, 5]);
    }
}
