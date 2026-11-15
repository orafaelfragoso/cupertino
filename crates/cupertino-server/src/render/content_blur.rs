//! A layer's content blurred by a Gaussian (`cupertino_layer_v1.content_blur`, [`SurfaceRole::Blurred`]): each surface
//! of the layer's tree is drawn at full opacity into a texture of its own with room for the blur's reach around it,
//! that texture is blurred by the material passes' pyramid and separable Gaussian (the definition backdrops are
//! blurred by), and the blurred texture is drawn where the surface lies, larger by the reach, through the window
//! content's mask and rescale at the layer's opacity. So the content softens past its own edges as a picture does, and
//! the client draws no frame for it. A blur made for a surface's commit, deviation and scale is drawn again as it is
//! until one of them changes.
//!
//! The copy holds only what can show: the part of the blurred surface the output and the content's mask let through,
//! with the content the blur's reach around it reads ([`copy_rect`]). A copy that would still hold more than
//! [`MAX_COPY_PIXELS`] or be longer than [`MAX_COPY_SIDE`] is not made, and the surface is drawn sharp, so no client
//! makes the server allocate more than that for a blur, however large it makes its content.
//!
//! [`SurfaceRole::Blurred`]: cupertino_scene::SurfaceRole::Blurred

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, Ordering};

use cupertino_scene::{ContentBlur, SceneItem};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, WaylandSurfaceTexture,
};
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer, Texture};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform};

use super::clip::{texels_per_pixel, ClipShape, Sampled};
use super::compose::TreeSlice;
use super::lower::{clipped, mask, physical, surface_elements, Lowering};
use super::shader_rect::{ElementKey, Sig};
use super::{CupertinoElement, SharedMaterial};

/// A blur whose deviation is under this many device pixels changes no pixel: the content is drawn as it is.
pub const SHARP_BELOW_PX: f64 = 0.3;

/// Lowerings a blurred copy is kept for after the last that drew it.
const KEEP: u64 = 8;

/// The most device pixels one blurred copy of a layer's content holds. Two textures this large and the blur's pass
/// targets are made for a copy, so this bounds what a layer's blur allocates; it holds a whole 4K output drawn at a
/// scale of 2 (as a fractional scale of 1.5 is drawn) with the widest blur's reach around it. Content whose visible part
/// needs more is drawn sharp.
pub const MAX_COPY_PIXELS: i64 = 6144 * 4096;

/// The longest side, in device pixels, of a blurred copy, within what every GPU's textures take; a longer copy is not
/// made and the content is drawn sharp.
pub const MAX_COPY_SIDE: i32 = 8192;

/// The most pixels of a blurred copy made in this process, for tests and diagnostics.
static LARGEST_COPY: AtomicI64 = AtomicI64::new(0);

/// The most pixels of any blurred copy made in this process; never more than [`MAX_COPY_PIXELS`].
pub fn largest_copy() -> i64 {
    LARGEST_COPY.load(Ordering::Relaxed)
}

/// How far, in device pixels, a Gaussian of deviation `sigma_px` reaches past what it blurs: three deviations, where
/// what is left of it is under a tenth of a percent.
pub fn reach_px(sigma_px: f64) -> i32 {
    (3.0 * sigma_px.max(0.0)).ceil() as i32
}

/// The deviation in device pixels of `blur` drawn at `scale` device pixels a point, or `None` when it changes nothing.
pub fn sigma_px(blur: ContentBlur, scale: f64) -> Option<f64> {
    let px = blur.sigma() * scale;
    (px >= SHARP_BELOW_PX && px.is_finite()).then_some(px)
}

/// Where the blurred copy of content lying at `geometry` (device pixels) is drawn: larger by the reach on every side.
pub fn spread(geometry: Rectangle<i32, Physical>, reach: i32) -> Rectangle<i32, Physical> {
    Rectangle::new(
        (geometry.loc.x - reach, geometry.loc.y - reach).into(),
        (geometry.size.w + 2 * reach, geometry.size.h + 2 * reach).into(),
    )
}

/// The rectangle (device pixels) a blurred copy of content lying at `geometry` covers when only `visible` of the output
/// can show: all of where the blur is drawn ([`spread`]) when that lies in `visible`, else its part in `visible` with
/// the reach around it, all the content that part reads. `None` when none of it shows.
pub fn copy_rect(
    geometry: Rectangle<i32, Physical>,
    reach: i32,
    visible: Rectangle<i32, Physical>,
) -> Option<Rectangle<i32, Physical>> {
    let drawn = spread(geometry, reach);
    let shown = drawn
        .intersection(visible)
        .filter(|r| r.size.w > 0 && r.size.h > 0)?;
    if shown == drawn {
        return Some(drawn);
    }
    spread(shown, reach).intersection(drawn)
}

/// Whether a copy covering `rect` keeps within [`MAX_COPY_PIXELS`] and [`MAX_COPY_SIDE`].
pub fn fits(rect: Rectangle<i32, Physical>) -> bool {
    let (w, h) = (rect.size.w, rect.size.h);
    w > 0
        && h > 0
        && w <= MAX_COPY_SIDE
        && h <= MAX_COPY_SIDE
        && i64::from(w) * i64::from(h) <= MAX_COPY_PIXELS
}

/// The output's device pixels `(0, 0)` to `viewport`, mapped back through a rescale about `pivot` by `rescale`: the
/// rectangle, before the rescale, whose drawing lands on the output. `None` for a rescale that draws nothing.
pub fn before_rescale(
    viewport: (i32, i32),
    pivot: Point<i32, Physical>,
    rescale: (f64, f64),
) -> Option<Rectangle<i32, Physical>> {
    const FAR: f64 = (1 << 30) as f64;
    let axis = |p: i32, size: i32, k: f64| -> Option<(i32, i32)> {
        (k > 0.0 && k.is_finite()).then(|| {
            let p = f64::from(p);
            let lo = (p - p / k).floor().clamp(-FAR, FAR);
            let hi = (p + (f64::from(size) - p) / k).ceil().clamp(-FAR, FAR);
            (lo as i32, hi as i32)
        })
    };
    let (x0, x1) = axis(pivot.x, viewport.0, rescale.0)?;
    let (y0, y1) = axis(pivot.y, viewport.1, rescale.1)?;
    Some(Rectangle::new(
        (x0, y0).into(),
        (x1.saturating_sub(x0), y1.saturating_sub(y0)).into(),
    ))
}

struct Kept {
    sig: u64,
    texture: GlesTexture,
    used: u64,
}

#[derive(Default)]
struct Inner {
    kept: HashMap<Id, Kept>,
    tick: u64,
    made: u64,
}

/// The blurred copies of one renderer, by surface element; clones share them.
#[derive(Clone, Default)]
pub struct ContentBlurs(Rc<RefCell<Inner>>);

impl ContentBlurs {
    /// How many blurred copies were made since the renderer started.
    pub fn made(&self) -> u64 {
        self.0.borrow().made
    }

    /// One lowering is over: forget the copies none of the last [`KEEP`] drew.
    pub fn end_lowering(&self) {
        let mut inner = self.0.borrow_mut();
        inner.tick += 1;
        let tick = inner.tick;
        inner.kept.retain(|_, k| tick - k.used <= KEEP);
    }

    fn kept(&self, id: &Id, sig: u64) -> Option<GlesTexture> {
        let mut inner = self.0.borrow_mut();
        let tick = inner.tick;
        let k = inner.kept.get_mut(id).filter(|k| k.sig == sig)?;
        k.used = tick;
        Some(k.texture.clone())
    }

    fn keep(&self, id: Id, sig: u64, texture: GlesTexture) {
        let mut inner = self.0.borrow_mut();
        inner.made += 1;
        let used = inner.tick;
        inner.kept.insert(id, Kept { sig, texture, used });
    }
}

/// The blurred copy of `element` (drawn at full opacity) by `sigma_px` device pixels at `scale` device pixels a point,
/// of the part [`copy_rect`] keeps when only `visible` (device pixels, before the layer's rescale) can show, kept or
/// made now: the texture, the rectangle it is drawn over and the signature of what it blurred. `None` for a surface
/// whose buffer is a solid colour or turned, one none of which shows, a copy past [`fits`], or when a pass is
/// unavailable: it is drawn sharp.
pub fn blurred(
    renderer: &mut GlesRenderer,
    material: Option<&SharedMaterial>,
    blurs: &ContentBlurs,
    element: &WaylandSurfaceRenderElement<GlesRenderer>,
    sigma_px: f64,
    scale: f64,
    visible: Rectangle<i32, Physical>,
) -> Option<(GlesTexture, Rectangle<i32, Physical>, u64)> {
    let material = material?;
    if !matches!(element.texture(), WaylandSurfaceTexture::Texture(_))
        || element.transform() != Transform::Normal
    {
        return None;
    }
    let geometry = element.geometry(Scale::from(scale));
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return None;
    }
    let reach = reach_px(sigma_px);
    let drawn = copy_rect(geometry, reach, visible).filter(|r| fits(*r))?;
    let commit = element
        .current_commit()
        .distance(Some(CommitCounter::default()))
        .unwrap_or(0);
    let sig = Sig::new()
        .u(commit as u64)
        .f(sigma_px)
        .f(scale)
        .f(element.src().loc.x)
        .f(element.src().loc.y)
        .f(element.src().size.w)
        .f(element.src().size.h)
        .u(geometry.size.w as u64)
        .u(geometry.size.h as u64)
        .u((geometry.loc.x - drawn.loc.x) as u64)
        .u((geometry.loc.y - drawn.loc.y) as u64)
        .u(drawn.size.w as u64)
        .u(drawn.size.h as u64)
        .finish();
    if let Some(texture) = blurs.kept(element.id(), sig) {
        return Some((texture, drawn, sig));
    }
    let size = (drawn.size.w, drawn.size.h);
    let mut copy: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, Size::from(size))
        .ok()?;
    {
        let mut target = renderer.bind(&mut copy).ok()?;
        let out: Size<i32, Physical> = Size::from(size);
        let mut frame = renderer.render(&mut target, out, Transform::Normal).ok()?;
        frame
            .clear(Color32F::TRANSPARENT, &[Rectangle::from_size(out)])
            .ok()?;
        // the content where it lies in the copy, cut by the copy's edges where the copy is cropped
        let dst = Rectangle::new(
            Point::from((geometry.loc.x - drawn.loc.x, geometry.loc.y - drawn.loc.y)),
            geometry.size,
        );
        // the damage is the element's own, relative to where it is drawn
        element
            .draw(
                &mut frame,
                element.src(),
                dst,
                &[Rectangle::from_size(geometry.size)],
                &[],
            )
            .ok()?;
        let _ = frame.finish().ok()?;
    }
    let out: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, Size::from(size))
        .ok()?;
    let (src, dst) = (copy.tex_id(), out.tex_id());
    let done = renderer
        .with_context(|gl| unsafe {
            material
                .borrow_mut()
                .blur_texture(gl, src, dst, size, sigma_px as f32, 1.0)
        })
        .ok()?;
    if !done {
        return None;
    }
    LARGEST_COPY.fetch_max(i64::from(size.0) * i64::from(size.1), Ordering::Relaxed);
    blurs.keep(element.id().clone(), sig, out.clone());
    Some((out, drawn, sig))
}

/// The elements of the layer content `surface` (with `only`, the slice's surfaces) blurred by `blur`, as `item`
/// places, fades and clips it, appended to `out`: each surface from its blurred copy, a surface that cannot be blurred
/// (or a blur too small to show) as it is.
#[allow(clippy::too_many_arguments)]
pub fn lower(
    ctx: &mut Lowering<'_>,
    surface: &WlSurface,
    only: Option<&TreeSlice>,
    blur: ContentBlur,
    item: &SceneItem,
    viewport: (i32, i32),
    out: &mut Vec<CupertinoElement>,
) {
    let place = item.placement;
    let origin = physical(place.origin, ctx.scale);
    let alpha = item.opacity as f32;
    let scale = Scale::from(ctx.scale);
    let pivot = physical(place.pivot, ctx.scale);
    let rescale = Scale::from((place.scale_x, place.scale_y));
    // unclipped, the mask is the whole framebuffer and more
    let shape = mask(ctx, item.clip, viewport).unwrap_or(ClipShape {
        rect: Rectangle::new((-(1 << 20), -(1 << 20)).into(), (1 << 21, 1 << 21).into()),
        extent: 0.0,
        exponent: 2.0,
        viewport,
        exact: false,
    });
    let shape = ClipShape {
        exact: false,
        ..shape
    };
    // what can show of the content before its rescale: the output's part the mask lets through
    let visible = before_rescale(viewport, pivot, (place.scale_x, place.scale_y))
        .and_then(|v| v.intersection(shape.rect));
    let sigma = sigma_px(blur, ctx.scale);
    // copies are drawn at full opacity: the layer's fades with its drawing
    let full = surface_elements(ctx, surface, only, origin, scale, 1.0, Kind::Unspecified);
    for e in full {
        let made = sigma.zip(visible).and_then(|(sigma, visible)| {
            blurred(
                ctx.renderer,
                ctx.material.as_ref(),
                &ctx.programs.blurred,
                &e,
                sigma,
                ctx.scale,
                visible,
            )
        });
        let Some((texture, rect, sig)) = made else {
            // drawn sharp, at the layer's opacity
            let sharp =
                surface_elements(ctx, surface, only, origin, scale, alpha, Kind::Unspecified)
                    .into_iter()
                    .find(|s| s.id() == e.id());
            if let Some(sharp) = sharp {
                let clip = clipped(ctx, super::ReducedElement::direct(sharp), shape);
                out.push(RescaleRenderElement::from_element(clip, pivot, rescale).into());
            }
            continue;
        };
        let (id, commit) = ctx.cache.slot(element_key(e.id()), sig);
        let copy = BlurredContent::new(id, commit, texture, rect, alpha);
        let clip = clipped(ctx, copy, shape);
        out.push(RescaleRenderElement::from_element(clip, pivot, rescale).into());
    }
}

/// The key of the element that draws the blurred copy of the surface element `id`.
pub fn element_key(id: &Id) -> ElementKey {
    ElementKey::new("content-blur", super::lower::id_hash(id))
}

/// A surface of a layer drawn from its blurred copy, over its place larger by the blur's reach.
#[derive(Debug)]
pub struct BlurredContent {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    /// Where the copy is drawn, device pixels of the output.
    rect: Rectangle<i32, Physical>,
    alpha: f32,
}

impl BlurredContent {
    /// The copy `texture` drawn over `rect` at `alpha`, under the element cache's `id` and `commit`, which advance with
    /// each new copy.
    pub fn new(
        id: Id,
        commit: CommitCounter,
        texture: GlesTexture,
        rect: Rectangle<i32, Physical>,
        alpha: f32,
    ) -> Self {
        Self {
            id,
            commit,
            texture,
            rect,
            alpha,
        }
    }

    fn whole(&self) -> Rectangle<f64, Buffer> {
        let size = self.texture.size();
        Rectangle::from_size((f64::from(size.w), f64::from(size.h)).into())
    }
}

impl Sampled for BlurredContent {
    fn sampled(
        &self,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
    ) -> Option<((f64, f64), f64)> {
        let size = self.texture.size();
        Some((
            (f64::from(size.w), f64::from(size.h)),
            texels_per_pixel(
                (src.size.w, src.size.h),
                (f64::from(dst.size.w), f64::from(dst.size.h)),
            ),
        ))
    }
}

impl Element for BlurredContent {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.whole()
    }

    fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> {
        self.rect
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for BlurredContent {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let _blurred = tracing::trace_span!("server.content_blur").entered();
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            None,
            &[],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blur_reaches_three_deviations_past_the_content_and_one_under_a_third_of_a_pixel_is_sharp()
    {
        assert_eq!(reach_px(0.0), 0);
        assert_eq!(reach_px(2.0), 6);
        assert_eq!(reach_px(2.1), 7);
        let blur = ContentBlur::of_points(8.0);
        assert_eq!(sigma_px(blur, 2.0), Some(16.0));
        assert_eq!(sigma_px(ContentBlur::of_points(0.1), 2.0), None);
        assert_eq!(sigma_px(ContentBlur::of_points(0.25), 1.5), Some(0.375));
        let at = Rectangle::new((100, 50).into(), (40, 20).into());
        assert_eq!(
            spread(at, 6),
            Rectangle::new((94, 44).into(), (52, 32).into())
        );
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn a_copy_holds_what_shows_with_the_reach_around_it_and_is_capped() {
        let output = rect(0, 0, 1200, 900);
        // content inside the output: all of where its blur is drawn, as ever
        let inside = rect(100, 50, 40, 20);
        assert_eq!(copy_rect(inside, 6, output), Some(spread(inside, 6)));
        // content far larger than the output: the output and the reach around it
        let huge = rect(0, 0, 30_000, 30_000);
        assert_eq!(
            copy_rect(huge, 288, output),
            Some(rect(-288, -288, 1776, 1476))
        );
        assert!(fits(copy_rect(huge, 288, output).unwrap()));
        // content off the output makes no copy
        assert_eq!(copy_rect(rect(5000, 5000, 40, 20), 6, output), None);
        // a whole 4K output drawn at a scale of 2 with the widest blur's reach around it fits; past the cap no copy is made
        let reach = reach_px(64.0 * 2.0);
        assert!(fits(rect(0, 0, 5120 + 2 * reach, 2880 + 2 * reach)));
        assert!(fits(rect(0, 0, 6144, 4096)));
        assert!(!fits(rect(0, 0, 6145, 4096)));
        assert!(!fits(rect(0, 0, MAX_COPY_SIDE + 1, 10)));
        // the output seen through a rescale: about the origin by a half, twice as much of the content shows
        assert_eq!(
            before_rescale((1200, 900), Point::from((0, 0)), (0.5, 0.5)),
            Some(rect(0, 0, 2400, 1800))
        );
        assert_eq!(
            before_rescale((1200, 900), Point::from((600, 450)), (2.0, 2.0)),
            Some(rect(300, 225, 600, 450))
        );
        assert_eq!(
            before_rescale((1200, 900), Point::from((0, 0)), (0.0, 1.0)),
            None
        );
        // a tenth of a 30000-pixel layer through a rescale of a twentieth needs a copy past the cap: drawn sharp
        let seen = before_rescale((1200, 900), Point::from((0, 0)), (0.05, 0.05)).unwrap();
        assert!(!fits(copy_rect(huge, 288, seen).unwrap()));
    }
}
