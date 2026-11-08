//! Glass pass on GLES, part of the material pipeline ([`MaterialGl`]): one element per glass container.
//!
//! The element covers what of the backdrop the container reads, its shapes grown by the reach of the blur and the
//! refraction (`Container::sample_area`). When it draws with a new backdrop, that area of the bound framebuffer is
//! captured at half scale, blurred into the levels its recipe uses and the blur fill, and kept; the composite then
//! shades the union of the shapes from those textures with the GLSL ES glass shader (`shaders/glass.frag`, a copy of
//! the one in `cupertino-glass`). The textures are kept per container and made again only when the backdrop below the
//! sampled area changed: [`area_generation`] signs the elements below that overlap it, so a change elsewhere on the
//! output, or above the glass, leaves them alone.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use cupertino_glass::blur::plan_blur;
use cupertino_glass::glass::{Container, GlassUniform};
use cupertino_glass::ops::PassParams;
use cupertino_theme::glass::{BLUR_LEVELS, GLASS_BACKDROP_SCALE};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{ffi, GlesError, GlesFrame, GlesRenderer};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size};

use super::{
    bind_block, link, uniform_loc, GLint, GLuint, MaterialGl, SavedState, SharedMaterial, Target,
};
use crate::render::shader_rect::{to_physical_rect, ElementCache, ElementKey, Sig};

const GLASS_VERT: &str = include_str!("../../shaders/glass.vert");
const GLASS_FRAG: &str = include_str!("../../shaders/glass.frag");

/// Uniform block binding of the glass composite.
const BIND_GLASS: GLuint = 2;
/// Most containers whose textures are kept; the least recently drawn goes first.
const MAX_CACHED: usize = 16;
/// Draws after which the textures of a container that no longer draws are released.
const STALE_AFTER: u64 = 2048;

/// A glass container of a frame: its shapes (output-local points), blend width and mode.
#[derive(Clone, Debug)]
pub struct GlassContainer {
    pub container: Container,
    /// Identity of everything that changes how it looks besides the backdrop.
    pub key: u64,
}

impl GlassContainer {
    pub fn new(container: Container) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        container.mode.hash(&mut h);
        container.spacing.to_bits().hash(&mut h);
        for s in &container.shapes {
            for v in s.rect.iter().chain([&s.radius]) {
                v.to_bits().hash(&mut h);
            }
        }
        Self {
            key: h.finish(),
            container,
        }
    }

    /// What of the backdrop the container reads, in output-local points.
    pub fn sample_area(&self) -> Option<Rectangle<f64, Logical>> {
        let [x, y, w, h] = self.container.sample_area()?;
        (w > 0.0 && h > 0.0).then(|| {
            Rectangle::new(
                (f64::from(x), f64::from(y)).into(),
                (f64::from(w), f64::from(h)).into(),
            )
        })
    }
}

/// The signature of the backdrop of an element covering `area`: of the elements below it (`below`, bottom to top:
/// identity, commit and geometry) those that overlap `area`, in order. Elements that do not touch it do not count, so
/// the glass keeps its blurred textures while the rest of the output changes.
pub fn area_generation(
    area: Rectangle<i32, Physical>,
    below: impl IntoIterator<Item = (u64, u64, Rectangle<i32, Physical>)>,
) -> u64 {
    let mut sig = Sig::new().u(0x67_6c_61_73_73);
    for (id, commit, g) in below {
        if g.overlaps(area) {
            sig = sig
                .u(id)
                .u(commit)
                .u(g.loc.x as u64)
                .u(g.loc.y as u64)
                .u(g.size.w as u64)
                .u(g.size.h as u64);
        }
    }
    sig.finish()
}

/// The framebuffer rectangle (x, y, w, h in framebuffer pixels, rows from the bottom) that `area` (output pixels)
/// lands on through `projection` (column-major, output pixels to clip space) and `viewport`, clamped to the viewport.
pub fn framebuffer_rect(
    projection: &[f32; 9],
    viewport: [i32; 4],
    area: Rectangle<i32, Physical>,
) -> Option<[i32; 4]> {
    let p = projection;
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for (x, y) in [
        (area.loc.x, area.loc.y),
        (area.loc.x + area.size.w, area.loc.y),
        (area.loc.x, area.loc.y + area.size.h),
        (area.loc.x + area.size.w, area.loc.y + area.size.h),
    ] {
        let (x, y) = (x as f32, y as f32);
        let c = [p[0] * x + p[3] * y + p[6], p[1] * x + p[4] * y + p[7]];
        for i in 0..2 {
            let v = (c[i] + 1.0) * 0.5 * viewport[2 + i] as f32 + viewport[i] as f32;
            lo[i] = lo[i].min(v);
            hi[i] = hi[i].max(v);
        }
    }
    // a corner on a pixel edge stays there despite rounding in the projection
    const SNAP: f32 = 1e-3;
    let x0 = ((lo[0] + SNAP).floor() as i32).max(viewport[0]);
    let y0 = ((lo[1] + SNAP).floor() as i32).max(viewport[1]);
    let x1 = ((hi[0] - SNAP).ceil() as i32).min(viewport[0] + viewport[2]);
    let y1 = ((hi[1] - SNAP).ceil() as i32).min(viewport[1] + viewport[3]);
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// The affine map from output pixels to the texture coordinates of a capture of framebuffer rectangle `fb`.
pub fn capture_uv(projection: &[f32; 9], viewport: [i32; 4], fb: [i32; 4]) -> [[f32; 3]; 2] {
    let p = projection;
    std::array::from_fn(|i| {
        let (vs, vo, fo, fs) = (
            viewport[2 + i] as f32,
            viewport[i] as f32,
            fb[i] as f32,
            fb[2 + i] as f32,
        );
        [
            p[i] * 0.5 * vs / fs,
            p[3 + i] * 0.5 * vs / fs,
            ((p[6 + i] + 1.0) * 0.5 * vs + vo - fo) / fs,
        ]
    })
}

/// GL objects of the glass pass and the textures it keeps per container.
#[derive(Default)]
pub(super) struct GlassGl {
    program: GLuint,
    proj: GLint,
    quad: GLint,
    ubo: GLuint,
    cache: Vec<CachedGlass>,
}

/// The blurred backdrop of one container: its textures and which of them each level and the blur fill reads.
struct CachedGlass {
    owner: u64,
    sig: u64,
    targets: Vec<Target>,
    slots: [usize; BLUR_LEVELS + 1],
    last_used: u64,
}

impl GlassGl {
    pub(super) unsafe fn init(gl: &ffi::Gles2) -> Result<Self, String> {
        let program = link(gl, GLASS_VERT, GLASS_FRAG).map_err(|e| format!("glass: {e}"))?;
        bind_block(gl, program, "Glass", BIND_GLASS)?;
        gl.UseProgram(program);
        for (unit, name) in ["u_level0", "u_level1", "u_level2", "u_level3", "u_fill"]
            .iter()
            .enumerate()
        {
            gl.Uniform1i(uniform_loc(gl, program, name), unit as GLint);
        }
        gl.UseProgram(0);
        let mut ubo = 0;
        gl.GenBuffers(1, &mut ubo);
        Ok(Self {
            program,
            proj: uniform_loc(gl, program, "u_proj"),
            quad: uniform_loc(gl, program, "u_quad"),
            ubo,
            cache: Vec::new(),
        })
    }

    pub(super) fn targets(&mut self) -> Vec<Target> {
        std::mem::take(&mut self.cache)
            .into_iter()
            .flat_map(|c| c.targets)
            .collect()
    }

    pub(super) unsafe fn destroy(&mut self, gl: &ffi::Gles2) {
        if self.program != 0 {
            gl.DeleteProgram(self.program);
            gl.DeleteBuffers(1, &self.ubo);
        }
        self.program = 0;
    }
}

/// Parameters of one glass composite.
pub struct GlassDraw<'a> {
    pub projection: [f32; 9],
    pub glass: &'a GlassContainer,
    /// Identity of the element, which keeps the container's textures.
    pub owner: u64,
    pub generation: u64,
    /// Output pixels per point.
    pub scale: f32,
    /// The sampled area in output pixels.
    pub area: Rectangle<i32, Physical>,
    pub opacity: f32,
    /// Rectangles (output pixels) to draw.
    pub damage: &'a [Rectangle<i32, Physical>],
}

impl MaterialGl {
    /// `src` blurred with a Gaussian of `sigma_pt` points at `px_per_pt` texels per point, into a new target.
    unsafe fn blurred(
        &mut self,
        gl: &ffi::Gles2,
        src: (GLuint, (i32, i32)),
        sigma_pt: f32,
        px_per_pt: f32,
    ) -> Option<Target> {
        let (ds, ds_loc, bl, bl_loc) = {
            let progs = self.progs.as_ref()?;
            (
                progs.downsample,
                progs.src_loc[0],
                progs.blur,
                progs.src_loc[1],
            )
        };
        let plan = plan_blur(src.1 .0 as u32, src.1 .1 as u32, 1.0, sigma_pt, px_per_pt);
        let (mut tex, mut size) = src;
        let mut owned: Option<Target> = None;
        for lvl in &plan.levels {
            let next_size = (lvl.0 as i32, lvl.1 as i32);
            let next = self.take_target(gl, next_size)?;
            let params = PassParams::new(
                (size.0 as u32, size.1 as u32),
                (next_size.0 as u32, next_size.1 as u32),
            );
            self.pass(gl, ds, ds_loc, tex, &next, &params);
            (tex, size) = (next.tex, next.size);
            if let Some(o) = owned.replace(next) {
                self.free.push(o);
            }
        }
        let mut params = PassParams::new(
            (size.0 as u32, size.1 as u32),
            (size.0 as u32, size.1 as u32),
        );
        params.set_weights(&plan.weights);
        let radius = plan.weights.len() as i32 - 1;
        let tmp = self.take_target(gl, size)?;
        let out = self.take_target(gl, size)?;
        params.dir_radius = [1, 0, radius, 0];
        self.pass(gl, bl, bl_loc, tex, &tmp, &params);
        params.dir_radius = [0, 1, radius, 0];
        self.pass(gl, bl, bl_loc, tmp.tex, &out, &params);
        self.free.push(tmp);
        if let Some(o) = owned {
            self.free.push(o);
        }
        Some(out)
    }

    /// Capture framebuffer rectangle `fb` of `src_fbo` at half scale and blur it into the levels and the blur fill
    /// `glass` reads.
    unsafe fn process_glass(
        &mut self,
        gl: &ffi::Gles2,
        src_fbo: GLuint,
        fb: [i32; 4],
        glass: &Container,
        scale: f32,
    ) -> Option<(Vec<Target>, [usize; BLUR_LEVELS + 1])> {
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        let size = (
            ((fb[2] as f32 * GLASS_BACKDROP_SCALE).ceil() as i32).max(1),
            ((fb[3] as f32 * GLASS_BACKDROP_SCALE).ceil() as i32).max(1),
        );
        let capture = self.take_target(gl, size)?;
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, src_fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, capture.fbo);
        gl.BlitFramebuffer(
            fb[0],
            fb[1],
            fb[0] + fb[2],
            fb[1] + fb[3],
            0,
            0,
            size.0,
            size.1,
            ffi::COLOR_BUFFER_BIT,
            ffi::LINEAR,
        );
        self.stats.passes += 1;
        // texels of the capture per point
        let px_per_pt = scale * size.0 as f32 / fb[2] as f32;
        let src = (capture.tex, capture.size);
        let mut targets = vec![capture];
        let mut slots = [usize::MAX; BLUR_LEVELS + 1];
        for (i, (radius, used)) in glass.levels().into_iter().enumerate() {
            if used {
                targets.push(self.blurred(gl, src, radius, px_per_pt)?);
                slots[i] = targets.len() - 1;
            }
        }
        if let Some(radius) = glass.fill_radius() {
            targets.push(self.blurred(gl, src, radius, px_per_pt)?);
            slots[BLUR_LEVELS] = targets.len() - 1;
        }
        // a level no shape reads has no weight: any texture will do
        let any = slots
            .iter()
            .copied()
            .find(|s| *s != usize::MAX)
            .unwrap_or(0);
        for s in &mut slots {
            if *s == usize::MAX {
                *s = any;
            }
        }
        Some((targets, slots))
    }

    /// Composite one glass container. The target framebuffer must be bound as draw framebuffer.
    ///
    /// # Safety
    /// Must be called with the renderer's context current (inside `GlesFrame::with_context`).
    pub unsafe fn draw_glass(&mut self, gl: &ffi::Gles2, p: &GlassDraw<'_>) -> Result<(), String> {
        if self.progs.is_none() || self.glass.program == 0 {
            return Err(self
                .failure
                .clone()
                .unwrap_or_else(|| "glass pass not initialised".into()));
        }
        self.tick += 1;
        let saved = SavedState::capture(gl);
        let viewport = saved.viewport;
        let Some(fb) = framebuffer_rect(&p.projection, viewport, p.area) else {
            return Ok(());
        };
        let sig = Sig::new()
            .u(p.generation)
            .u(p.glass.key)
            .u(p.scale.to_bits() as u64)
            .u(fb[0] as u64)
            .u(fb[1] as u64)
            .u(fb[2] as u64)
            .u(fb[3] as u64)
            .finish();
        let tick = self.tick;
        // release what containers that stopped drawing kept
        let mut i = 0;
        while i < self.glass.cache.len() {
            if tick.saturating_sub(self.glass.cache[i].last_used) > STALE_AFTER {
                let old = self.glass.cache.swap_remove(i);
                self.free.extend(old.targets);
            } else {
                i += 1;
            }
        }
        let hit = self
            .glass
            .cache
            .iter()
            .position(|c| c.owner == p.owner && c.sig == sig);
        let idx = match hit {
            Some(i) => {
                self.stats.backdrop_hits += 1;
                i
            }
            None => {
                self.stats.backdrop_misses += 1;
                if let Some(i) = self.glass.cache.iter().position(|c| c.owner == p.owner) {
                    let old = self.glass.cache.swap_remove(i);
                    self.free.extend(old.targets);
                }
                let Some((targets, slots)) = self.process_glass(
                    gl,
                    saved.draw_fbo as GLuint,
                    fb,
                    &p.glass.container,
                    p.scale,
                ) else {
                    saved.restore(gl);
                    return Err("glass backdrop processing failed (framebuffer incomplete?)".into());
                };
                if self.glass.cache.len() >= MAX_CACHED {
                    let oldest = (0..self.glass.cache.len())
                        .min_by_key(|i| self.glass.cache[*i].last_used)
                        .unwrap();
                    let old = self.glass.cache.swap_remove(oldest);
                    self.free.extend(old.targets);
                }
                self.glass.cache.push(CachedGlass {
                    owner: p.owner,
                    sig,
                    targets,
                    slots,
                    last_used: tick,
                });
                self.glass.cache.len() - 1
            }
        };
        self.glass.cache[idx].last_used = tick;
        let textures: Vec<GLuint> = self.glass.cache[idx]
            .slots
            .iter()
            .map(|s| self.glass.cache[idx].targets[*s].tex)
            .collect();

        let uv = capture_uv(&p.projection, viewport, fb);
        let quad = [
            p.area.loc.x as f32,
            p.area.loc.y as f32,
            p.area.size.w as f32,
            p.area.size.h as f32,
        ];
        let target = [viewport[2] as f32, viewport[3] as f32];
        let uniform = GlassUniform::new(&p.glass.container, p.scale, uv, quad, target, p.opacity);

        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, saved.draw_fbo as GLuint);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, saved.read_fbo as GLuint);
        gl.Viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
        if saved.scissor {
            gl.Enable(ffi::SCISSOR_TEST);
        }
        gl.Enable(ffi::BLEND);
        gl.BlendFuncSeparate(
            ffi::SRC_ALPHA,
            ffi::ONE_MINUS_SRC_ALPHA,
            ffi::ZERO,
            ffi::ONE,
        );
        gl.UseProgram(self.glass.program);
        Self::upload_ubo(gl, self.glass.ubo, BIND_GLASS, bytemuck::bytes_of(&uniform));
        gl.UniformMatrix3fv(self.glass.proj, 1, ffi::FALSE, p.projection.as_ptr());
        for (unit, tex) in textures.iter().enumerate() {
            gl.ActiveTexture(ffi::TEXTURE0 + unit as u32);
            gl.BindTexture(ffi::TEXTURE_2D, *tex);
        }
        for d in p.damage {
            let Some(q) = d.intersection(p.area) else {
                continue;
            };
            gl.Uniform4f(
                self.glass.quad,
                q.loc.x as f32,
                q.loc.y as f32,
                q.size.w as f32,
                q.size.h as f32,
            );
            gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
            self.stats.composites += 1;
        }
        for unit in 2..textures.len() {
            gl.ActiveTexture(ffi::TEXTURE0 + unit as u32);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
        }
        saved.restore(gl);
        Ok(())
    }
}

/// A glass container's element. It covers the container's sampled area; everything below it in the element list is
/// part of its backdrop. [`GlassElement::finalize`] gives it the signature of the elements below that overlap the area
/// ([`area_generation`]): its commit advances when they changed, so the damage tracker repaints the whole area and the
/// backdrop is captured from a freshly drawn scene.
pub struct GlassElement {
    key: ElementKey,
    id: Id,
    commit: CommitCounter,
    area: Rectangle<f64, Logical>,
    glass: Rc<GlassContainer>,
    shared: SharedMaterial,
    generation: u64,
    opacity: f32,
    scale: f64,
}

impl std::fmt::Debug for GlassElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlassElement")
            .field("area", &self.area)
            .field("generation", &self.generation)
            .finish()
    }
}

impl GlassElement {
    pub fn new(
        key: ElementKey,
        cache: &mut ElementCache,
        area: Rectangle<f64, Logical>,
        glass: Rc<GlassContainer>,
        shared: SharedMaterial,
        opacity: f32,
        scale: f64,
    ) -> Self {
        let mut e = Self {
            key,
            id: Id::new(),
            commit: CommitCounter::default(),
            area,
            glass,
            shared,
            generation: 0,
            opacity,
            scale,
        };
        e.finalize(0, cache);
        e
    }

    pub fn area(&self) -> Rectangle<f64, Logical> {
        self.area
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Set the backdrop signature ([`area_generation`]) and refresh the id and commit counter from the cache.
    pub fn finalize(&mut self, generation: u64, cache: &mut ElementCache) {
        self.generation = generation;
        let sig = Sig::new()
            .rect(self.area)
            .f(self.opacity as f64)
            .u(self.glass.key)
            .u(generation)
            .finish();
        let (id, commit) = cache.slot(self.key, sig);
        self.id = id;
        self.commit = commit;
    }

    fn owner(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.key.hash(&mut h);
        h.finish()
    }
}

impl Element for GlassElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(Size::from((1.0, 1.0)))
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        to_physical_rect(self.area, scale)
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for GlassElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if damage.is_empty() || !self.shared.borrow().is_ready() {
            return Ok(());
        }
        let abs: Vec<Rectangle<i32, Physical>> = damage
            .iter()
            .map(|d| Rectangle::new(d.loc + dst.loc, d.size))
            .collect();
        // the shapes are output-local points; the element is drawn where its area is
        let params = GlassDraw {
            projection: *frame.projection(),
            glass: &self.glass,
            owner: self.owner(),
            generation: self.generation,
            scale: self.scale as f32,
            area: dst,
            opacity: self.opacity,
            damage: &abs,
        };
        let shared = self.shared.clone();
        let res =
            frame.with_context(move |gl| unsafe { shared.borrow_mut().draw_glass(gl, &params) })?;
        if let Err(e) = res {
            let mut s = self.shared.borrow_mut();
            if s.failure.is_none() {
                s.failure = Some(e.clone());
            }
            tracing::error!(error = %e, "glass pass failed");
        }
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_glass::glass::GlassShape;
    use cupertino_theme::glass::GlassMode;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// The projection of an output `w`×`h` pixels drawn into a framebuffer whose rows start at the bottom.
    fn projection(w: f32, h: f32) -> [f32; 9] {
        [2.0 / w, 0.0, 0.0, 0.0, 2.0 / h, 0.0, -1.0, -1.0, 1.0]
    }

    #[test]
    fn the_capture_covers_the_area_and_maps_it_to_the_whole_texture() {
        let proj = projection(200.0, 100.0);
        let viewport = [0, 0, 200, 100];
        let fb = framebuffer_rect(&proj, viewport, rect(20, 10, 40, 30)).unwrap();
        assert_eq!(fb, [20, 10, 40, 30]);
        let uv = capture_uv(&proj, viewport, fb);
        let at = |x: f32, y: f32| {
            [
                uv[0][0] * x + uv[0][1] * y + uv[0][2],
                uv[1][0] * x + uv[1][1] * y + uv[1][2],
            ]
        };
        assert_eq!(at(20.0, 10.0), [0.0, 0.0]);
        let far = at(60.0, 40.0);
        assert!((far[0] - 1.0).abs() < 1e-5 && (far[1] - 1.0).abs() < 1e-5);
        // past the output the capture is clamped to the framebuffer
        assert_eq!(
            framebuffer_rect(&proj, viewport, rect(-30, 90, 60, 40)).unwrap(),
            [0, 90, 30, 10]
        );
        assert_eq!(
            framebuffer_rect(&proj, viewport, rect(300, 0, 10, 10)),
            None
        );
    }

    #[test]
    fn a_flipped_projection_still_maps_the_area_onto_its_capture() {
        // rows from the top: y is mirrored in clip space
        let proj = [
            2.0 / 200.0,
            0.0,
            0.0,
            0.0,
            -2.0 / 100.0,
            0.0,
            -1.0,
            1.0,
            1.0,
        ];
        let viewport = [0, 0, 200, 100];
        let fb = framebuffer_rect(&proj, viewport, rect(20, 10, 40, 30)).unwrap();
        assert_eq!(fb, [20, 60, 40, 30]);
        let uv = capture_uv(&proj, viewport, fb);
        let v = |y: f32| uv[1][1] * y + uv[1][2];
        assert!((v(10.0) - 1.0).abs() < 1e-5 && v(40.0).abs() < 1e-5);
    }

    #[test]
    fn the_backdrop_signature_changes_only_with_what_lies_below_the_sampled_area() {
        let area = rect(100, 100, 200, 100);
        let inside = |commit| (1, commit, rect(150, 120, 50, 50));
        let outside = |commit| (2, commit, rect(400, 400, 50, 50));
        let base = area_generation(area, [inside(0), outside(0)]);
        // a change outside the area keeps the blurred levels
        assert_eq!(base, area_generation(area, [inside(0), outside(5)]));
        assert_eq!(base, area_generation(area, [inside(0)]));
        // a change of content, a move or a new element under the area makes them again
        assert_ne!(base, area_generation(area, [inside(1), outside(0)]));
        assert_ne!(
            base,
            area_generation(area, [(1, 0, rect(160, 120, 50, 50)), outside(0)])
        );
        assert_ne!(
            base,
            area_generation(
                area,
                [inside(0), (3, 0, rect(290, 190, 20, 20)), outside(0)]
            )
        );
        // an element that moves into the area counts from then on
        assert_ne!(
            base,
            area_generation(area, [inside(0), (2, 0, rect(280, 150, 50, 50))])
        );
    }

    #[test]
    fn the_container_key_follows_shapes_spacing_and_mode() {
        let mode = GlassMode::default();
        let make = |x: f32, spacing: f32, mode: GlassMode| {
            GlassContainer::new(Container::new(
                vec![GlassShape::new([x, 0.0, 100.0, 40.0], 12.0)],
                spacing,
                mode,
            ))
            .key
        };
        let base = make(0.0, 0.0, mode);
        assert_eq!(base, make(0.0, 0.0, mode));
        assert_ne!(base, make(1.0, 0.0, mode));
        assert_ne!(base, make(0.0, 8.0, mode));
        assert_ne!(
            base,
            make(
                0.0,
                0.0,
                GlassMode {
                    reduce_transparency: true,
                    ..mode
                }
            )
        );
    }
}
