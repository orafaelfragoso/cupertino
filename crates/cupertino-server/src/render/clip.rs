//! Rounded-corner mask for window surface elements: wraps a surface element and draws it with
//! the clip texture shader. The mask rectangle is given in output pixels.
//!
//! The commit of a masked element carries the inner element's commit in its low half and the
//! generation of the mask shape in its high half, so a shape change alone (an animated corner
//! radius) is a new commit and damages the whole element.

use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};

use super::programs::{invert3, u2, u4, umat3};
use smithay::backend::renderer::gles::Uniform;

/// Mask description in output pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipShape {
    pub rect: Rectangle<i32, Physical>,
    /// Corner extent in pixels (nominal radius times the continuous-corner expansion).
    pub extent: f32,
    pub exponent: f32,
    /// Framebuffer size in pixels.
    pub viewport: (i32, i32),
    /// The mask is drawn unscaled: the cross-shaped interior can be reported as opaque.
    pub exact: bool,
}

impl ClipShape {
    /// Interior rectangles that are fully covered by the mask: the clip rectangle without its
    /// corner squares, as two overlapping bars.
    pub fn opaque_cross(&self) -> [Rectangle<i32, Physical>; 2] {
        let e = self.extent.ceil() as i32;
        let r = self.rect;
        let w = (r.size.w - 2 * e).max(0);
        let h = (r.size.h - 2 * e).max(0);
        [
            Rectangle::new((r.loc.x + e, r.loc.y).into(), (w, r.size.h).into()),
            Rectangle::new((r.loc.x, r.loc.y + e).into(), (r.size.w, h).into()),
        ]
    }
}

/// Bits of a commit that carry the inner element's commit; the rest carry the shape generation.
const HALF: u32 = usize::BITS / 2;
const LOW: usize = (1 << HALF) - 1;

/// The value of `c` (a commit's distance from the first commit).
fn count(c: CommitCounter) -> usize {
    c.distance(Some(CommitCounter::default()))
        .expect("every commit is at or after the first")
}

/// Commit of a masked element whose shape is at `generation` (already within the high half's
/// width) and whose inner element is at `inner`.
fn masked_commit(generation: usize, inner: CommitCounter) -> CommitCounter {
    CommitCounter::from((generation << HALF) | (count(inner) & LOW))
}

/// For damage since the masked commit `since`: `None` when the shape changed after it (the whole
/// element is damaged), else the inner element's commit to ask for its own damage.
fn inner_since(generation: usize, since: Option<CommitCounter>) -> Option<Option<CommitCounter>> {
    match since.map(count) {
        Some(c) if c >> HALF != generation => None,
        Some(c) => Some(Some(CommitCounter::from(c & LOW))),
        None => Some(None),
    }
}

/// What an element samples as it draws the part `src` of its buffer over `dst`: the size of its texture in texels and
/// how many of them a pixel covers a side; `None` for an element that draws no texture, or one the minifying filter
/// does not apply to.
pub trait Sampled {
    fn sampled(
        &self,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
    ) -> Option<((f64, f64), f64)> {
        None
    }
}

impl Sampled
    for smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<GlesRenderer>
{
}

/// How many texels a pixel covers when `part` texels are drawn over `dst` pixels: the side of the square of equal area,
/// so a turned buffer is covered as an upright one.
pub fn texels_per_pixel(part: (f64, f64), dst: (f64, f64)) -> f64 {
    if dst.0 <= 0.0 || dst.1 <= 0.0 {
        return 1.0;
    }
    ((part.0 * part.1) / (dst.0 * dst.1)).max(0.0).sqrt()
}

/// A texture is drawn through the minifying filter once a pixel covers this many texels: below it plain filtered
/// lookups show no shimmer.
pub const MINIFY_FROM: f64 = 1.05;

/// The most texels a side the minifying filter averages for a pixel.
pub const MINIFY_MOST: f64 = 16.0;

/// The uniforms of the minifying filter for a texture of `texture` texels of which each pixel covers `texels` a side:
/// the box it covers, in texels (at most [`MINIFY_MOST`]), and the texture's size.
pub fn minify_uniforms(texture: (f64, f64), texels: f64) -> [Uniform<'static>; 2] {
    let side = texels.clamp(1.0, MINIFY_MOST) as f32;
    [
        u2("footprint", side, side),
        u2(
            "texels",
            texture.0.max(1.0) as f32,
            texture.1.max(1.0) as f32,
        ),
    ]
}

#[derive(Debug)]
pub struct ClipElement<E> {
    inner: E,
    program: GlesTexProgram,
    /// The program for an inner texture drawn smaller than its texels ([`minify_uniforms`]).
    minify: Option<GlesTexProgram>,
    shape: ClipShape,
    /// Generation of `shape`, kept to the high half's width.
    generation: usize,
    /// Drawn as a vibrant layer: its coverage alone, blended with what lies behind it.
    vibrant: Option<cupertino_scene::VibrantBlend>,
}

impl<E> ClipElement<E> {
    /// `generation` advances whenever `shape` changes (an [`super::ElementCache`] slot keyed by
    /// the inner element, with the shape as its signature).
    pub fn new(
        inner: E,
        program: GlesTexProgram,
        shape: ClipShape,
        generation: CommitCounter,
    ) -> Self {
        Self {
            inner,
            program,
            minify: None,
            shape,
            generation: count(generation) & LOW,
            vibrant: None,
        }
    }

    /// Draw an inner texture smaller than its texels through `program`, the clip program with the minifying filter.
    pub fn minified(mut self, program: GlesTexProgram) -> Self {
        self.minify = Some(program);
        self
    }

    /// The element drawn as a vibrant layer through `program`, the vibrant clip program.
    pub fn vibrant(
        mut self,
        program: GlesTexProgram,
        blend: cupertino_scene::VibrantBlend,
    ) -> Self {
        self.program = program;
        self.vibrant = Some(blend);
        self
    }

    /// The vibrant program's uniform and whether the coverage is screened in (over a dark face, not plain).
    fn vibrant_uniform(&self) -> Option<([f32; 4], bool)> {
        self.vibrant.map(|v| {
            let dark = if v.dark { 1.0 } else { 0.0 };
            let plain = if v.plain { 1.0 } else { 0.0 };
            (
                [dark, f32::from(v.weight) / 1000.0, plain, 0.0],
                v.dark && !v.plain,
            )
        })
    }

    pub fn shape(&self) -> &ClipShape {
        &self.shape
    }
}

impl<E: Element> Element for ClipElement<E> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        masked_commit(self.generation, self.inner.current_commit())
    }

    fn location(&self, scale: Scale<f64>) -> smithay::utils::Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    /// Everything after a shape change; otherwise the inner element's damage. An inner commit past
    /// the low half's width compares as a large distance, which the inner element reports as full
    /// damage.
    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match inner_since(self.generation, commit) {
            Some(inner) => self.inner.damage_since(scale, inner),
            None => DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)]),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        if !self.shape.exact || self.vibrant.is_some() {
            return OpaqueRegions::default();
        }
        let geo = self.inner.geometry(scale);
        let cross = self.shape.opaque_cross();
        let mut out = Vec::new();
        for r in self.inner.opaque_regions(scale).iter() {
            let abs = Rectangle::new(r.loc + geo.loc, r.size);
            for c in &cross {
                if let Some(i) = abs.intersection(*c) {
                    out.push(Rectangle::new(i.loc - geo.loc, i.size));
                }
            }
        }
        OpaqueRegions::from_slice(&out)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<E: RenderElement<GlesRenderer> + Sampled> RenderElement<GlesRenderer> for ClipElement<E> {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let Some(inv) = invert3(frame.projection()) else {
            return self.inner.draw(frame, src, dst, damage, opaque_regions);
        };
        let r = self.shape.rect;
        // a texture drawn smaller than its texels: each pixel averages those it covers
        // a vibrant layer draws through its own program, never the minifying one
        let minify = self
            .minify
            .as_ref()
            .filter(|_| self.vibrant.is_none())
            .and_then(|program| {
                let (texture, texels) = self.inner.sampled(src, dst)?;
                (texels > MINIFY_FROM).then(|| (program.clone(), minify_uniforms(texture, texels)))
            });
        let (program, extra) = match minify {
            Some((program, extra)) => (program, Some(extra)),
            None => (self.program.clone(), None),
        };
        let vibrant = self.vibrant_uniform();
        let mut uniforms = vec![
            umat3("inv_proj", inv),
            u2(
                "viewport",
                self.shape.viewport.0 as f32,
                self.shape.viewport.1 as f32,
            ),
            u4(
                "clip_rect",
                [
                    r.loc.x as f32,
                    r.loc.y as f32,
                    r.size.w as f32,
                    r.size.h as f32,
                ],
            ),
            u4(
                "clip_shape",
                [self.shape.extent, self.shape.exponent, 0.0, 0.0],
            ),
        ];
        let Some((v, screened)) = vibrant else {
            uniforms.extend(extra.into_iter().flatten());
            frame.override_default_tex_program(program, uniforms);
            let res = self.inner.draw(frame, src, dst, damage, opaque_regions);
            frame.clear_tex_program_override();
            return res;
        };
        uniforms.push(u4("vibrant", v));
        frame.override_default_tex_program(self.program.clone(), uniforms);
        use smithay::backend::renderer::gles::ffi;
        if screened {
            // what lies behind plus the lift times what is not white yet; its alpha stays
            frame.with_context(|gl| unsafe {
                gl.BlendFuncSeparate(ffi::ONE_MINUS_DST_COLOR, ffi::ONE, ffi::ZERO, ffi::ONE)
            })?;
        }
        // no opaque region: the renderer would draw it without blending
        let res = self.inner.draw(frame, src, dst, damage, &[]);
        if screened {
            frame.with_context(|gl| unsafe { gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA) })?;
        }
        frame.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(w: i32, h: i32, extent: f32) -> ClipShape {
        ClipShape {
            rect: Rectangle::new((100, 50).into(), (w, h).into()),
            extent,
            exponent: 2.0,
            viewport: (1920, 1080),
            exact: true,
        }
    }

    #[test]
    fn opaque_cross_excludes_corner_squares() {
        let s = shape(200, 100, 12.0);
        let [a, b] = s.opaque_cross();
        assert_eq!(a, Rectangle::new((112, 50).into(), (176, 100).into()));
        assert_eq!(b, Rectangle::new((100, 62).into(), (200, 76).into()));
        // the corner pixel is in neither bar
        let corner = (100, 50);
        for r in [a, b] {
            assert!(!r.contains(corner));
        }
    }

    #[test]
    fn a_shape_change_alone_is_a_new_commit_that_damages_everything() {
        let inner = CommitCounter::from(7);
        let (at_one, at_two) = (masked_commit(1, inner), masked_commit(2, inner));
        assert_ne!(at_one, at_two, "same content, new corner radius");
        assert_eq!(
            inner_since(2, Some(at_one)),
            None,
            "the whole element is damaged"
        );
        assert_eq!(
            inner_since(1, Some(at_one)),
            Some(Some(inner)),
            "same shape: the inner damage since its own commit"
        );
        assert_eq!(
            inner_since(1, Some(masked_commit(1, CommitCounter::from(5)))),
            Some(Some(CommitCounter::from(5)))
        );
        assert_eq!(
            inner_since(1, None),
            Some(None),
            "a first draw asks the inner element for everything"
        );
    }

    #[test]
    fn opaque_cross_handles_tiny_windows() {
        let [a, b] = shape(10, 10, 12.0).opaque_cross();
        assert_eq!(a.size.w, 0);
        assert_eq!(b.size.h, 0);
    }
}
