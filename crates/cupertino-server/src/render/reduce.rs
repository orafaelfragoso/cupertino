//! Reduced copies of window surfaces shown well below their size (the overview, the app switcher, a zoom), and the
//! element that draws a surface, from its copy or through the minifying filter. A client's texture holds no smaller
//! levels, so a surface drawn at half its size or less is copied once at a power-of-two reduction of its size, each
//! pixel the average of the texels it covers ([`super::clip::minify_uniforms`]), and drawn from that copy until the
//! surface commits again; the copy itself is drawn at more than half its size, through the same filter.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, WaylandSurfaceTexture,
};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer, Texture};
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform};

use super::clip::{minify_uniforms, texels_per_pixel, Sampled, MINIFY_FROM};
use super::programs::{u2, u4, umat3};

/// The most halvings a copy takes: a 64th of the size a side.
const MAX_HALVINGS: u32 = 6;

/// Lowerings a copy is kept for after the last that drew it: a window that leaves the overview for a moment is not
/// copied again.
const KEEP: u64 = 8;

/// The halvings of the copy a surface drawn at `scale` of its size is drawn from: as many as leave it drawn at more
/// than half the copy's size; none for a surface drawn at more than half its own.
pub fn halvings(scale: f64) -> u32 {
    let mut k = 0;
    while k < MAX_HALVINGS && scale > 0.0 && scale * f64::from(2u32 << k) <= 1.0 {
        k += 1;
    }
    k
}

/// The size of the copy of a surface of `size` pixels after `halvings`, rounded up, at least a pixel.
pub fn copy_size(size: (i32, i32), halvings: u32) -> (i32, i32) {
    let d = 1i32 << halvings;
    (((size.0 + d - 1) / d).max(1), ((size.1 + d - 1) / d).max(1))
}

/// The part of a copy (texels) that holds the part `src` of a surface's buffer, of which the copy holds `of`.
fn copy_part(
    src: Rectangle<f64, Buffer>,
    of: Rectangle<f64, Buffer>,
    copy: (i32, i32),
) -> Rectangle<f64, Buffer> {
    let kx = f64::from(copy.0) / of.size.w.max(f64::MIN_POSITIVE);
    let ky = f64::from(copy.1) / of.size.h.max(f64::MIN_POSITIVE);
    Rectangle::new(
        Point::from(((src.loc.x - of.loc.x) * kx, (src.loc.y - of.loc.y) * ky)),
        Size::from((src.size.w * kx, src.size.h * ky)),
    )
}

/// The uniforms of the clip program for a draw that cuts nothing away: a rectangle far past any output.
fn uncut() -> Vec<Uniform<'static>> {
    vec![
        umat3("inv_proj", [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
        u2("viewport", 1.0, 1.0),
        u4("clip_rect", [-1.0e5, -1.0e5, 2.0e5, 2.0e5]),
        u4("clip_shape", [0.0, 2.0, 0.0, 0.0]),
    ]
}

struct Copy {
    commit: CommitCounter,
    of: Rectangle<f64, Buffer>,
    size: (i32, i32),
    texture: GlesTexture,
    used: u64,
}

#[derive(Default)]
struct Inner {
    copies: HashMap<Id, Copy>,
    tick: u64,
    made: u64,
}

/// The reduced copies of one renderer, by surface; clones share them.
#[derive(Clone, Default)]
pub struct Reductions(Rc<RefCell<Inner>>);

impl Reductions {
    /// How many copies were made since the renderer started.
    pub fn made(&self) -> u64 {
        self.0.borrow().made
    }

    /// How many copies are kept.
    pub fn live(&self) -> usize {
        self.0.borrow().copies.len()
    }

    /// One lowering is over: forget the copies none of the last [`KEEP`] drew.
    pub fn end_lowering(&self) {
        let mut inner = self.0.borrow_mut();
        inner.tick += 1;
        let tick = inner.tick;
        inner.copies.retain(|_, c| tick - c.used <= KEEP);
    }

    /// The copy of `element` at `size` pixels, if one is kept for its current commit and part.
    pub fn kept(
        &self,
        element: &WaylandSurfaceRenderElement<GlesRenderer>,
        size: (i32, i32),
    ) -> Option<(GlesTexture, Rectangle<f64, Buffer>)> {
        let mut inner = self.0.borrow_mut();
        let tick = inner.tick;
        let c = inner.copies.get_mut(element.id())?;
        if c.commit != element.current_commit() || c.of != element.src() || c.size != size {
            return None;
        }
        c.used = tick;
        Some((c.texture.clone(), c.of))
    }

    /// Copy `element` at `size` pixels, drawn at full opacity, each pixel the average of the texels it covers through
    /// `program` (the clip program with the minifying filter), and keep the copy for its commit. `None` for a surface
    /// whose buffer is a solid colour or turned, which is drawn as it is.
    pub fn make(
        &self,
        renderer: &mut GlesRenderer,
        program: &GlesTexProgram,
        element: &WaylandSurfaceRenderElement<GlesRenderer>,
        size: (i32, i32),
    ) -> Option<(GlesTexture, Rectangle<f64, Buffer>)> {
        let WaylandSurfaceTexture::Texture(source) = element.texture() else {
            return None;
        };
        if element.transform() != Transform::Normal {
            return None;
        }
        let texture_size = source.size();
        let of = element.src();
        let mut copy: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, Size::from(size))
            .ok()?;
        {
            let mut target = renderer.bind(&mut copy).ok()?;
            let out: Size<i32, Physical> = Size::from(size);
            let mut frame = renderer.render(&mut target, out, Transform::Normal).ok()?;
            let dst = Rectangle::from_size(out);
            frame.clear(Color32F::TRANSPARENT, &[dst]).ok()?;
            let texels = texels_per_pixel(
                (of.size.w, of.size.h),
                (f64::from(size.0), f64::from(size.1)),
            );
            let mut uniforms = uncut();
            uniforms.extend(minify_uniforms(
                (f64::from(texture_size.w), f64::from(texture_size.h)),
                texels,
            ));
            frame.override_default_tex_program(program.clone(), uniforms);
            let drawn = element.draw(&mut frame, of, dst, &[dst], &[]);
            frame.clear_tex_program_override();
            drawn.ok()?;
            // the copy is drawn from on this context, which keeps the order of its commands: nothing to wait for
            let _ = frame.finish().ok()?;
        }
        let mut inner = self.0.borrow_mut();
        inner.made += 1;
        let tick = inner.tick;
        inner.copies.insert(
            element.id().clone(),
            Copy {
                commit: element.current_commit(),
                of,
                size,
                texture: copy.clone(),
                used: tick,
            },
        );
        Some((copy, of))
    }
}

/// A window's surface, drawn from its reduced copy when it has one, else from its own texture; a surface drawn
/// without a clip element (a popup) takes the minifying filter itself.
pub struct ReducedElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    /// The copy and the part of the surface's buffer it holds.
    copy: Option<(GlesTexture, Rectangle<f64, Buffer>)>,
    minify: Option<GlesTexProgram>,
}

impl std::fmt::Debug for ReducedElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReducedElement")
            .field("inner", &self.inner)
            .field("reduced", &self.copy.is_some())
            .finish()
    }
}

impl ReducedElement {
    /// The surface drawn from its own texture.
    pub fn direct(inner: WaylandSurfaceRenderElement<GlesRenderer>) -> Self {
        Self {
            inner,
            copy: None,
            minify: None,
        }
    }

    /// The surface drawn from `copy`, which holds the part `of` of its buffer.
    pub fn reduced(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        copy: GlesTexture,
        of: Rectangle<f64, Buffer>,
    ) -> Self {
        Self {
            inner,
            copy: Some((copy, of)),
            minify: None,
        }
    }

    /// Drawn smaller than its texels through `program` (the clip program with the minifying filter), for a surface no
    /// clip element draws.
    pub fn minified(mut self, program: GlesTexProgram) -> Self {
        self.minify = Some(program);
        self
    }

    /// Whether it is drawn from a reduced copy.
    pub fn is_reduced(&self) -> bool {
        self.copy.is_some()
    }

    /// The texture drawn, the part of it for `src` (the part of the surface's buffer asked for) and its turn.
    fn drawn(
        &self,
        src: Rectangle<f64, Buffer>,
    ) -> Option<(&GlesTexture, Rectangle<f64, Buffer>, Transform)> {
        match &self.copy {
            Some((copy, of)) => {
                let size = copy.size();
                Some((
                    copy,
                    copy_part(src, *of, (size.w, size.h)),
                    Transform::Normal,
                ))
            }
            None => match self.inner.texture() {
                WaylandSurfaceTexture::Texture(t) => Some((t, src, self.inner.transform())),
                WaylandSurfaceTexture::SolidColor(_) => None,
            },
        }
    }
}

impl Sampled for ReducedElement {
    fn sampled(
        &self,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
    ) -> Option<((f64, f64), f64)> {
        let (texture, part, _) = self.drawn(src)?;
        let size = texture.size();
        Some((
            (f64::from(size.w), f64::from(size.h)),
            texels_per_pixel(
                (part.size.w, part.size.h),
                (f64::from(dst.size.w), f64::from(dst.size.h)),
            ),
        ))
    }
}

impl Element for ReducedElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
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

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for ReducedElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        // a surface no clip element draws minifies itself
        let own = self.minify.as_ref().and_then(|program| {
            let (texture, texels) = self.sampled(src, dst)?;
            (texels > MINIFY_FROM).then(|| {
                let mut uniforms = uncut();
                uniforms.extend(minify_uniforms(texture, texels));
                (program, uniforms)
            })
        });
        if self.copy.is_none() && own.is_none() {
            return self.inner.draw(frame, src, dst, damage, opaque_regions);
        }
        let Some((texture, part, transform)) = self.drawn(src) else {
            return self.inner.draw(frame, src, dst, damage, opaque_regions);
        };
        let (program, uniforms) = match &own {
            Some((program, uniforms)) => (Some(*program), uniforms.as_slice()),
            None => (None, &[][..]),
        };
        frame.render_texture_from_to(
            texture,
            part,
            dst,
            damage,
            opaque_regions,
            transform,
            self.inner.alpha(),
            program,
            uniforms,
        )
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // a copy is the compositor's own texture: only the surface itself goes on a plane
        if self.copy.is_some() {
            return None;
        }
        self.inner.underlying_storage(renderer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_drawn_at_half_its_size_or_less_is_drawn_from_a_copy_at_more_than_half_the_copys() {
        assert_eq!(halvings(1.0), 0);
        assert_eq!(halvings(0.51), 0);
        assert_eq!(halvings(0.5), 1);
        assert_eq!(halvings(0.3), 1);
        assert_eq!(halvings(0.25), 2);
        assert_eq!(halvings(0.2), 2);
        assert_eq!(halvings(0.1), 3);
        assert_eq!(halvings(0.0), 0);
        assert_eq!(halvings(1e-6), MAX_HALVINGS);
        for s in [0.5, 0.4, 0.3, 0.26, 0.25, 0.2, 0.13, 0.1, 0.05] {
            let drawn = s * f64::from(1u32 << halvings(s));
            assert!(
                drawn > 0.5 && drawn <= 1.0,
                "{s}: the copy is drawn at {drawn}"
            );
        }
        assert_eq!(copy_size((1601, 1000), 2), (401, 250));
        assert_eq!(copy_size((3, 3), 6), (1, 1));
    }

    #[test]
    fn the_part_of_a_copy_follows_the_part_of_the_buffer_asked_for() {
        let of = Rectangle::new(Point::from((0.0, 0.0)), Size::from((800.0, 600.0)));
        assert_eq!(
            copy_part(of, of, (200, 150)),
            Rectangle::new(Point::from((0.0, 0.0)), Size::from((200.0, 150.0)))
        );
        let half = Rectangle::new(Point::from((400.0, 0.0)), Size::from((400.0, 300.0)));
        assert_eq!(
            copy_part(half, of, (200, 150)),
            Rectangle::new(Point::from((100.0, 0.0)), Size::from((100.0, 75.0)))
        );
    }
}
