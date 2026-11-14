//! The uniform blocks of the toolkit's coverage and colour programs ([`crate::shaders::UI_FILL`] and
//! [`crate::shaders::UI_TEXTURE`]) as Rust types, laid out as the programs' `std140` blocks are (every member a
//! `vec4` or an array of them, so nothing is padded): what anyone drawing with those programs fills, the toolkit's
//! renderer and the compositor alike.

/// An analytic clip of a draw: window pixel to the clip shape's centred units (`linear`: a b c d; `moved`: e f and
/// the half size), its corner radius and pixels per unit (`corner`: radius, pixels, unused, unused).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClipBlock {
    pub linear: [f32; 4],
    pub moved: [f32; 4],
    pub corner: [f32; 4],
}

/// The clips in force for a draw, the same in both blocks: how many analytic clips there are, whether the path clip
/// mask is used (and, in the fill block, the shape's corner exponent), the mask's window pixels and atlas texel, and
/// up to four analytic clips.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClipsBlock {
    /// Analytic clips, path clip mask used, and in [`FillBlock`] the corner exponent of its shape (0: circular) and
    /// whether its corners each have their own radius ([`FillBlock::corners`]).
    pub info: [f32; 4],
    /// The path clip mask's window pixels (x0 y0 x1 y1).
    pub mask_rect: [f32; 4],
    /// The atlas texel of the mask's first pixel.
    pub mask_at: [f32; 4],
    pub clips: [ClipBlock; 4],
}

/// The block of [`crate::shaders::UI_FILL`]: a shape by the shared distance or a coverage image, in a solid colour or
/// a gradient, times the clips in force.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FillBlock {
    /// The target's size in pixels, and the window pixel of its first pixel.
    pub frame: [f32; 4],
    /// Window pixel to the shape's centred units: a b c d, then e f and the half size.
    pub shape_linear: [f32; 4],
    pub shape_moved: [f32; 4],
    /// Corner radius (0: square corners), pixels per unit, stroke width (0 for a fill, else a stroke centred on the
    /// outline), what covers (0 the shape, 1 the coverage image).
    pub shape: [f32; 4],
    /// The solid colour, premultiplied, in bytes.
    pub color: [f32; 4],
    /// Paint kind (0 solid, 1 linear, 2 radial), stop count, whether it is dithered, and the dither pattern's shift.
    pub paint: [f32; 4],
    /// Window pixel to paint coordinates: a b c d, then e f.
    pub paint_linear: [f32; 4],
    pub paint_moved: [f32; 4],
    /// Linear: start and end; radial: centre and radius.
    pub paint_geometry: [f32; 4],
    /// The stops' offsets, four a `vec4`.
    pub offsets: [[f32; 4]; 2],
    /// The stops' straight colours.
    pub colors: [[f32; 4]; 8],
    pub clips: ClipsBlock,
    /// The shape's corner radii when the clips' `info[3]` is set: top left, top right, bottom right, bottom left.
    pub corners: [f32; 4],
}

/// The block of [`crate::shaders::UI_TEXTURE`]: a colour glyph, a layer or an image, times the clips in force.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextureBlock {
    /// The target's size in pixels, and the window pixel of its first pixel.
    pub frame: [f32; 4],
    /// What is drawn (0 a colour glyph, 1 a layer, 2 an image), the opacity in bytes, linear sampling, whether the
    /// image's edge coverage mask is used.
    pub mode: [f32; 4],
    /// A glyph run's colour, premultiplied, in bytes.
    pub tint: [f32; 4],
    /// A layer: the window pixel of its first pixel.
    pub source: [f32; 4],
    /// An image: the texels sampled (x0 y0 x1 y1), and window pixel to texel coordinates (a b c d, then e f).
    pub crop: [f32; 4],
    pub image_linear: [f32; 4],
    pub image_moved: [f32; 4],
    /// The edge coverage mask's window pixels and the atlas texel of its first pixel.
    pub cover_rect: [f32; 4],
    pub cover_at: [f32; 4],
    pub clips: ClipsBlock,
}

macro_rules! floats {
    ($ty:ty, $n:expr) => {
        impl $ty {
            /// Floats in the block.
            pub const FLOATS: usize = $n;

            /// The block as the floats a uniform buffer holds, in order.
            pub fn floats(&self) -> &[f32; $n] {
                // Safety: the type is `repr(C)` of `f32` arrays only, `$n` floats in all (checked below), so it has
                // no padding and the alignment of `f32`.
                unsafe { &*(self as *const Self as *const [f32; $n]) }
            }

            /// The block's floats, to write in place.
            pub fn floats_mut(&mut self) -> &mut [f32; $n] {
                // Safety: as in `floats`; every bit pattern is a valid `f32`.
                unsafe { &mut *(self as *mut Self as *mut [f32; $n]) }
            }

            /// The block's bytes, as a uniform buffer is written.
            pub fn bytes(&self) -> &[u8] {
                // Safety: plain floats without padding, viewed as their bytes.
                unsafe { std::slice::from_raw_parts(self as *const Self as *const u8, $n * 4) }
            }
        }
        const _: () = assert!(std::mem::size_of::<$ty>() == $n * 4);
    };
}

floats!(FillBlock, 140);
floats!(TextureBlock, 96);

impl FillBlock {
    /// Where the clips start, in floats.
    pub const CLIPS: usize = 76;
}

impl TextureBlock {
    /// Where the clips start, in floats.
    pub const CLIPS: usize = 36;
}

impl ClipsBlock {
    /// Floats in the clips' part of a block.
    pub const FLOATS: usize = 60;
}

const _: () = assert!(std::mem::size_of::<ClipsBlock>() == ClipsBlock::FLOATS * 4);
const _: () = assert!(std::mem::offset_of!(FillBlock, clips) == FillBlock::CLIPS * 4);
const _: () = assert!(std::mem::offset_of!(TextureBlock, clips) == TextureBlock::CLIPS * 4);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaders::{NamedBinding, UI_FILL, UI_TEXTURE};

    /// The members of the struct `Draw`, which the program's block at group 0, binding 0 holds, in order, from its
    /// generated GLSL.
    fn members(shader: &crate::shaders::ShaderSource) -> Vec<String> {
        assert!(shader.bindings().iter().any(|b| matches!(
            b,
            NamedBinding::Uniform { group: 0, binding: 0, block } if block.starts_with("Draw_block_")
        )));
        let glsl = shader.fragment_glsl;
        let body = &glsl[glsl.find("struct Draw {").expect("the block's struct") + 13..];
        body[..body.find('}').unwrap()]
            .split(';')
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_blocks_are_the_programs_blocks_member_by_member() {
        // every member a vec4 or an array of them, so the std140 layout is the members one after the other
        let count = |members: &[String]| -> usize {
            members
                .iter()
                .map(|m| {
                    let n = m
                        .split('[')
                        .nth(1)
                        .map_or(1, |n| n.trim_end_matches(']').parse::<usize>().unwrap());
                    let per = match m.split_whitespace().next() {
                        Some("vec4") => 4,
                        // the clips' struct: three vec4s
                        Some("Clip") => 12,
                        other => panic!("a member of type {other:?}"),
                    };
                    n * per
                })
                .sum()
        };
        let fill = members(&UI_FILL);
        assert_eq!(fill.len(), 16, "{fill:?}");
        assert!(fill[15].contains("corners"), "{fill:?}");
        assert_eq!(count(&fill), FillBlock::FLOATS, "{fill:?}");
        assert!(fill[11].contains("clip_info"), "{fill:?}");
        assert_eq!(count(&fill[..11]), FillBlock::CLIPS);
        let texture = members(&UI_TEXTURE);
        assert_eq!(count(&texture), TextureBlock::FLOATS, "{texture:?}");
        assert!(texture[9].contains("clip_info"), "{texture:?}");
        assert_eq!(count(&texture[..9]), TextureBlock::CLIPS);
    }

    #[test]
    fn a_block_s_floats_are_its_members_in_order() {
        let mut b = FillBlock {
            frame: [1.0, 2.0, 3.0, 4.0],
            ..FillBlock::default()
        };
        b.clips.info[2] = 5.0;
        b.clips.clips[3].corner[1] = 6.0;
        let f = b.floats();
        assert_eq!(&f[..4], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(f[FillBlock::CLIPS + 2], 5.0);
        assert_eq!(f[FillBlock::CLIPS + ClipsBlock::FLOATS - 3], 6.0);
        b.floats_mut()[12] = 7.0;
        assert_eq!(b.shape[0], 7.0);
        assert_eq!(b.bytes().len(), FillBlock::FLOATS * 4);
        let mut t = TextureBlock::default();
        t.floats_mut()[TextureBlock::CLIPS] = 2.0;
        assert_eq!(t.clips.info[0], 2.0);
    }
}
