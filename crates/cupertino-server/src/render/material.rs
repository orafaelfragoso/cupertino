//! Material pass on GLES: backdrop capture → downsample pyramid → separable Gaussian → colour
//! operations → composite with a rounded mask, using the GLSL ES 3.00 ports of the
//! `cupertino-glass` shaders (compositor copies in `shaders/, see the README for the changes).
//!
//! All GL work happens inside [`MaterialElement::draw`], on the renderer's own context, while the
//! frame's target framebuffer is bound. The capture reads that framebuffer, so the element must be
//! drawn after everything that should appear in the backdrop; the scene builder guarantees this
//! by element order. The processed backdrop is cached by the caller-supplied generation counter:
//! while the scene below the element does not change, no blur pass is executed.
//!
//! recipes (see `cupertino-glass`), blur σ in points,
//! σ → texel mapping, tint layer colour approximate.

use std::cell::RefCell;
use std::ffi::CString;
use std::rc::Rc;

use cupertino_geometry::CornerStyle;
use cupertino_glass::blur::{plan_blur, BlurPlan};
use cupertino_glass::ops::{color_ops, fill_ops, Op, PassParams, RegionParams};
use cupertino_theme::recipe::Recipe;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{ffi, GlesError, GlesFrame, GlesRenderer};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size};

use super::shader_rect::{to_physical_rect, ElementCache, ElementKey, Sig};

const FULLSCREEN_VERT: &str = include_str!("../../shaders/fullscreen.vert");
const DOWNSAMPLE_FRAG: &str = include_str!("../../shaders/downsample.frag");
const BLUR_FRAG: &str = include_str!("../../shaders/blur.frag");
const COLOR_FRAG: &str = include_str!("../../shaders/color.frag");
const MATERIAL_VERT: &str = include_str!("../../shaders/material.vert");
const MATERIAL_FRAG: &str = include_str!("../../shaders/material.frag");

type GLuint = ffi::types::GLuint;
type GLint = ffi::types::GLint;
type GLenum = ffi::types::GLenum;

/// Uniform block bindings.
const BIND_PASS: GLuint = 0;
const BIND_REGION: GLuint = 1;

/// A recipe flattened for the GPU, shared by every element using the same material.
#[derive(Clone, Debug)]
pub struct GpuRecipe {
    pub key: u64,
    /// Capture scale, σ in points, colour operations.
    pub backdrop: Option<(f32, f32, Vec<Op>)>,
    pub fills: Vec<Op>,
}

impl GpuRecipe {
    pub fn new(recipe: &Recipe) -> Self {
        use std::hash::{Hash, Hasher};
        let backdrop = recipe
            .backdrop
            .as_ref()
            .map(|b| (b.scale, b.sigma_pt, color_ops(&b.ops)));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some((s, sigma, ops)) = &backdrop {
            s.to_bits().hash(&mut h);
            sigma.to_bits().hash(&mut h);
            for o in ops {
                for v in o.a.iter().chain(o.b.iter()) {
                    v.to_bits().hash(&mut h);
                }
            }
        } else {
            0u8.hash(&mut h);
        }
        Self {
            key: h.finish(),
            backdrop,
            fills: fill_ops(recipe),
        }
    }
}

struct Target {
    tex: GLuint,
    fbo: GLuint,
    size: (i32, i32),
}

struct Programs {
    downsample: GLuint,
    blur: GLuint,
    color: GLuint,
    material: GLuint,
    material_loc: MaterialLocs,
    src_loc: [GLint; 3],
}

#[derive(Clone, Copy, Default)]
struct MaterialLocs {
    proj: GLint,
    quad: GLint,
    backdrop: GLint,
}

/// Identity of a processed backdrop: recipe, capture size, pixel scale and, for backdrops captured
/// by a [`MaterialStage::Capture`] element, the owner they were captured for.
type BackdropKey = (u64, i32, i32, u32, Option<u64>);

struct CachedBackdrop {
    key: BackdropKey,
    generation: u64,
    target: Target,
    last_used: u64,
}

/// Counters for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialStats {
    pub backdrop_hits: u64,
    pub backdrop_misses: u64,
    pub passes: u64,
    pub composites: u64,
}

/// GL objects of the material pipeline. Created through [`MaterialGl::new`] on a renderer.
pub struct MaterialGl {
    progs: Option<Programs>,
    ubo_pass: GLuint,
    ubo_region: GLuint,
    float_targets: bool,
    free: Vec<Target>,
    cache: Vec<CachedBackdrop>,
    tick: u64,
    pub stats: MaterialStats,
    failure: Option<String>,
}

pub type SharedMaterial = Rc<RefCell<MaterialGl>>;

unsafe fn compile(gl: &ffi::Gles2, kind: GLenum, src: &str) -> Result<GLuint, String> {
    let sh = gl.CreateShader(kind);
    let ptr = src.as_ptr() as *const ffi::types::GLchar;
    let len = src.len() as GLint;
    gl.ShaderSource(sh, 1, &ptr, &len);
    gl.CompileShader(sh);
    let mut ok = 0;
    gl.GetShaderiv(sh, ffi::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut n = 0;
        gl.GetShaderiv(sh, ffi::INFO_LOG_LENGTH, &mut n);
        let mut buf = vec![0u8; n.max(1) as usize];
        let mut w = 0;
        gl.GetShaderInfoLog(sh, n, &mut w, buf.as_mut_ptr() as *mut _);
        gl.DeleteShader(sh);
        return Err(String::from_utf8_lossy(&buf[..w.max(0) as usize]).into_owned());
    }
    Ok(sh)
}

unsafe fn link(gl: &ffi::Gles2, vert: &str, frag: &str) -> Result<GLuint, String> {
    let v = compile(gl, ffi::VERTEX_SHADER, vert).map_err(|e| format!("vertex: {e}"))?;
    let f = compile(gl, ffi::FRAGMENT_SHADER, frag).map_err(|e| format!("fragment: {e}"))?;
    let p = gl.CreateProgram();
    gl.AttachShader(p, v);
    gl.AttachShader(p, f);
    gl.LinkProgram(p);
    gl.DetachShader(p, v);
    gl.DetachShader(p, f);
    gl.DeleteShader(v);
    gl.DeleteShader(f);
    let mut ok = 0;
    gl.GetProgramiv(p, ffi::LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut n = 0;
        gl.GetProgramiv(p, ffi::INFO_LOG_LENGTH, &mut n);
        let mut buf = vec![0u8; n.max(1) as usize];
        let mut w = 0;
        gl.GetProgramInfoLog(p, n, &mut w, buf.as_mut_ptr() as *mut _);
        gl.DeleteProgram(p);
        return Err(String::from_utf8_lossy(&buf[..w.max(0) as usize]).into_owned());
    }
    Ok(p)
}

unsafe fn uniform_loc(gl: &ffi::Gles2, p: GLuint, name: &str) -> GLint {
    let c = CString::new(name).unwrap();
    gl.GetUniformLocation(p, c.as_ptr())
}

unsafe fn bind_block(
    gl: &ffi::Gles2,
    p: GLuint,
    block: &str,
    binding: GLuint,
) -> Result<(), String> {
    let c = CString::new(block).unwrap();
    let idx = gl.GetUniformBlockIndex(p, c.as_ptr());
    if idx == ffi::INVALID_INDEX {
        return Err(format!("uniform block {block} missing"));
    }
    gl.UniformBlockBinding(p, idx, binding);
    Ok(())
}

unsafe fn gl_int(gl: &ffi::Gles2, name: GLenum) -> GLint {
    let mut v = 0;
    gl.GetIntegerv(name, &mut v);
    v
}

/// GL state the material pass touches; restored after every draw.
struct SavedState {
    draw_fbo: GLint,
    read_fbo: GLint,
    viewport: [GLint; 4],
    scissor: bool,
    blend: bool,
    blend_func: [GLint; 4],
    active_tex: GLint,
    tex0: GLint,
    tex1: GLint,
    program: GLint,
}

impl SavedState {
    unsafe fn capture(gl: &ffi::Gles2) -> Self {
        let mut viewport = [0; 4];
        gl.GetIntegerv(ffi::VIEWPORT, viewport.as_mut_ptr());
        let active_tex = gl_int(gl, ffi::ACTIVE_TEXTURE);
        gl.ActiveTexture(ffi::TEXTURE0);
        let tex0 = gl_int(gl, ffi::TEXTURE_BINDING_2D);
        gl.ActiveTexture(ffi::TEXTURE1);
        let tex1 = gl_int(gl, ffi::TEXTURE_BINDING_2D);
        gl.ActiveTexture(active_tex as GLenum);
        Self {
            draw_fbo: gl_int(gl, ffi::DRAW_FRAMEBUFFER_BINDING),
            read_fbo: gl_int(gl, ffi::READ_FRAMEBUFFER_BINDING),
            viewport,
            scissor: gl.IsEnabled(ffi::SCISSOR_TEST) != 0,
            blend: gl.IsEnabled(ffi::BLEND) != 0,
            blend_func: [
                gl_int(gl, ffi::BLEND_SRC_RGB),
                gl_int(gl, ffi::BLEND_DST_RGB),
                gl_int(gl, ffi::BLEND_SRC_ALPHA),
                gl_int(gl, ffi::BLEND_DST_ALPHA),
            ],
            active_tex,
            tex0,
            tex1,
            program: gl_int(gl, ffi::CURRENT_PROGRAM),
        }
    }

    unsafe fn restore(&self, gl: &ffi::Gles2) {
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, self.draw_fbo as GLuint);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, self.read_fbo as GLuint);
        gl.Viewport(
            self.viewport[0],
            self.viewport[1],
            self.viewport[2],
            self.viewport[3],
        );
        let set = |cap: GLenum, on: bool| if on { gl.Enable(cap) } else { gl.Disable(cap) };
        set(ffi::SCISSOR_TEST, self.scissor);
        set(ffi::BLEND, self.blend);
        gl.BlendFuncSeparate(
            self.blend_func[0] as GLenum,
            self.blend_func[1] as GLenum,
            self.blend_func[2] as GLenum,
            self.blend_func[3] as GLenum,
        );
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, self.tex0 as GLuint);
        gl.ActiveTexture(ffi::TEXTURE1);
        gl.BindTexture(ffi::TEXTURE_2D, self.tex1 as GLuint);
        gl.ActiveTexture(self.active_tex as GLenum);
        gl.UseProgram(self.program as GLuint);
    }
}

pub use cupertino_scene::MaterialStage;

/// The owner a capture and its composite share.
fn stage_owner(stage: MaterialStage) -> Option<u64> {
    match stage {
        MaterialStage::Full => None,
        MaterialStage::Capture { owner } | MaterialStage::Composite { owner } => Some(owner),
    }
}

/// Parameters of one composite draw.
pub struct DrawParams<'a> {
    pub stage: MaterialStage,
    pub projection: [f32; 9],
    pub recipe: &'a GpuRecipe,
    pub generation: u64,
    /// Output size in pixels (capture size).
    pub output_px: (i32, i32),
    /// Pixel scale of the output (points to pixels).
    pub scale: f32,
    /// Region rectangle in output pixels.
    pub rect: Rectangle<i32, Physical>,
    /// Corner extent (px), superellipse exponent.
    pub corner: (f32, f32),
    pub opacity: f32,
    pub tint: [f32; 3],
    /// Rectangles (absolute output pixels) to draw.
    pub damage: &'a [Rectangle<i32, Physical>],
}

impl MaterialGl {
    /// Compile the programs on `renderer`'s context. Fails (with a description) when the context
    /// does not provide GLES 3.0 features.
    pub fn new(renderer: &mut GlesRenderer) -> Result<SharedMaterial, String> {
        let mut me = MaterialGl {
            progs: None,
            ubo_pass: 0,
            ubo_region: 0,
            float_targets: false,
            free: Vec::new(),
            cache: Vec::new(),
            tick: 0,
            stats: MaterialStats::default(),
            failure: None,
        };
        let res = renderer
            .with_context(|gl| unsafe { me.init(gl) })
            .map_err(|e| format!("making the context current failed: {e}"))?;
        res?;
        Ok(Rc::new(RefCell::new(me)))
    }

    unsafe fn init(&mut self, gl: &ffi::Gles2) -> Result<(), String> {
        let version = {
            let p = gl.GetString(ffi::VERSION);
            if p.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(p as *const _)
                    .to_string_lossy()
                    .into_owned()
            }
        };
        let es3 = version.contains("OpenGL ES 3") || version.contains("OpenGL ES 4");
        if !es3 {
            return Err(format!(
                "material pass needs OpenGL ES 3.0, context reports {version:?}"
            ));
        }
        let pass = |frag: &str| link(gl, FULLSCREEN_VERT, frag);
        let downsample = pass(DOWNSAMPLE_FRAG).map_err(|e| format!("downsample: {e}"))?;
        let blur = pass(BLUR_FRAG).map_err(|e| format!("blur: {e}"))?;
        let color = pass(COLOR_FRAG).map_err(|e| format!("color: {e}"))?;
        let material =
            link(gl, MATERIAL_VERT, MATERIAL_FRAG).map_err(|e| format!("material: {e}"))?;
        for p in [downsample, blur, color] {
            bind_block(gl, p, "PassParams", BIND_PASS)?;
        }
        bind_block(gl, material, "Region", BIND_REGION)?;
        let src_loc = [downsample, blur, color].map(|p| uniform_loc(gl, p, "u_src"));
        let material_loc = MaterialLocs {
            proj: uniform_loc(gl, material, "u_proj"),
            quad: uniform_loc(gl, material, "u_quad"),
            backdrop: uniform_loc(gl, material, "u_backdrop"),
        };
        self.progs = Some(Programs {
            downsample,
            blur,
            color,
            material,
            material_loc,
            src_loc,
        });
        let mut bufs = [0; 2];
        gl.GenBuffers(2, bufs.as_mut_ptr());
        self.ubo_pass = bufs[0];
        self.ubo_region = bufs[1];
        self.float_targets = self.probe_float_targets(gl);
        tracing::info!(version = %version, float_targets = self.float_targets, "material pipeline initialised");
        Ok(())
    }

    /// Whether half-float render targets are usable (otherwise 8-bit targets are used).
    unsafe fn probe_float_targets(&mut self, gl: &ffi::Gles2) -> bool {
        let saved_fbo = gl_int(gl, ffi::FRAMEBUFFER_BINDING);
        let saved_tex = gl_int(gl, ffi::TEXTURE_BINDING_2D);
        let t = self.alloc_target(gl, (4, 4), true);
        let ok = t.is_some();
        if let Some(t) = t {
            self.destroy_target(gl, t);
        }
        gl.BindFramebuffer(ffi::FRAMEBUFFER, saved_fbo as GLuint);
        gl.BindTexture(ffi::TEXTURE_2D, saved_tex as GLuint);
        ok
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// A pipeline without GL objects, for tests of the scene builder (never drawn).
    #[cfg(test)]
    pub(crate) fn detached() -> SharedMaterial {
        Rc::new(RefCell::new(MaterialGl {
            progs: None,
            ubo_pass: 0,
            ubo_region: 0,
            float_targets: false,
            free: Vec::new(),
            cache: Vec::new(),
            tick: 0,
            stats: MaterialStats::default(),
            failure: None,
        }))
    }

    pub fn is_ready(&self) -> bool {
        self.progs.is_some() && self.failure.is_none()
    }

    unsafe fn alloc_target(
        &mut self,
        gl: &ffi::Gles2,
        size: (i32, i32),
        float: bool,
    ) -> Option<Target> {
        let mut tex = 0;
        gl.GenTextures(1, &mut tex);
        gl.BindTexture(ffi::TEXTURE_2D, tex);
        if float {
            gl.TexImage2D(
                ffi::TEXTURE_2D,
                0,
                ffi::RGBA16F as GLint,
                size.0,
                size.1,
                0,
                ffi::RGBA,
                ffi::HALF_FLOAT,
                std::ptr::null(),
            );
        } else {
            gl.TexImage2D(
                ffi::TEXTURE_2D,
                0,
                ffi::RGBA8 as GLint,
                size.0,
                size.1,
                0,
                ffi::RGBA,
                ffi::UNSIGNED_BYTE,
                std::ptr::null(),
            );
        }
        for (p, v) in [
            (ffi::TEXTURE_MIN_FILTER, ffi::LINEAR),
            (ffi::TEXTURE_MAG_FILTER, ffi::LINEAR),
            (ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE),
            (ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE),
        ] {
            gl.TexParameteri(ffi::TEXTURE_2D, p, v as GLint);
        }
        let mut fbo = 0;
        gl.GenFramebuffers(1, &mut fbo);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
        gl.FramebufferTexture2D(
            ffi::FRAMEBUFFER,
            ffi::COLOR_ATTACHMENT0,
            ffi::TEXTURE_2D,
            tex,
            0,
        );
        if gl.CheckFramebufferStatus(ffi::FRAMEBUFFER) != ffi::FRAMEBUFFER_COMPLETE {
            gl.DeleteFramebuffers(1, &fbo);
            gl.DeleteTextures(1, &tex);
            return None;
        }
        Some(Target { tex, fbo, size })
    }

    unsafe fn destroy_target(&self, gl: &ffi::Gles2, t: Target) {
        gl.DeleteFramebuffers(1, &t.fbo);
        gl.DeleteTextures(1, &t.tex);
    }

    unsafe fn take_target(&mut self, gl: &ffi::Gles2, size: (i32, i32)) -> Option<Target> {
        if let Some(i) = self.free.iter().position(|t| t.size == size) {
            return Some(self.free.swap_remove(i));
        }
        if self.free.len() > 12 {
            let t = self.free.remove(0);
            self.destroy_target(gl, t);
        }
        let float = self.float_targets;
        self.alloc_target(gl, size, float)
    }

    unsafe fn upload_ubo(gl: &ffi::Gles2, buf: GLuint, binding: GLuint, data: &[u8]) {
        gl.BindBuffer(ffi::UNIFORM_BUFFER, buf);
        gl.BufferData(
            ffi::UNIFORM_BUFFER,
            data.len() as isize,
            data.as_ptr() as *const _,
            ffi::DYNAMIC_DRAW,
        );
        gl.BindBufferBase(ffi::UNIFORM_BUFFER, binding, buf);
    }

    /// One full-screen pass: `src` texture → `dst` target using `program` and `params`.
    unsafe fn pass(
        &mut self,
        gl: &ffi::Gles2,
        program: GLuint,
        src_loc: GLint,
        src: GLuint,
        dst: &Target,
        params: &PassParams,
    ) {
        gl.BindFramebuffer(ffi::FRAMEBUFFER, dst.fbo);
        gl.Viewport(0, 0, dst.size.0, dst.size.1);
        gl.UseProgram(program);
        Self::upload_ubo(gl, self.ubo_pass, BIND_PASS, bytemuck::bytes_of(params));
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, src);
        gl.Uniform1i(src_loc, 0);
        gl.DrawArrays(ffi::TRIANGLES, 0, 3);
        self.stats.passes += 1;
    }

    /// Process the backdrop for `recipe`: capture the bound framebuffer and filter it.
    unsafe fn process_backdrop(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        src_fbo: GLuint,
    ) -> Option<Target> {
        let (w, h) = p.output_px;
        let (scale, sigma, ops) = match &p.recipe.backdrop {
            Some((s, sg, ops)) => (*s, *sg, ops.as_slice()),
            None => (1.0, 0.0, &[][..]),
        };
        let plan: BlurPlan = plan_blur(w as u32, h as u32, scale, sigma, p.scale);
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        // capture: the first level is a linear blit (a 2x2 box average at half size)
        let first = plan
            .levels
            .first()
            .map(|l| (l.0 as i32, l.1 as i32))
            .unwrap_or((w, h));
        let mut cur = self.take_target(gl, first)?;
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, src_fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, cur.fbo);
        let filter = if plan.levels.is_empty() {
            ffi::NEAREST
        } else {
            ffi::LINEAR
        };
        gl.BlitFramebuffer(
            0,
            0,
            w,
            h,
            0,
            0,
            first.0,
            first.1,
            ffi::COLOR_BUFFER_BIT,
            filter,
        );
        self.stats.passes += 1;
        let progs = self.progs.as_ref()?;
        let (ds, ds_loc, bl, bl_loc, co, co_loc) = (
            progs.downsample,
            progs.src_loc[0],
            progs.blur,
            progs.src_loc[1],
            progs.color,
            progs.src_loc[2],
        );
        // pyramid
        for lvl in plan.levels.iter().skip(1) {
            let size = (lvl.0 as i32, lvl.1 as i32);
            let next = self.take_target(gl, size)?;
            let params = PassParams::new(
                (cur.size.0 as u32, cur.size.1 as u32),
                (size.0 as u32, size.1 as u32),
            );
            self.pass(gl, ds, ds_loc, cur.tex, &next, &params);
            self.free.push(cur);
            cur = next;
        }
        // separable Gaussian
        if plan.weights.len() > 1 {
            let mut params = PassParams::new(
                (cur.size.0 as u32, cur.size.1 as u32),
                (cur.size.0 as u32, cur.size.1 as u32),
            );
            params.set_weights(&plan.weights);
            let radius = (plan.weights.len() - 1) as i32;
            let tmp = self.take_target(gl, cur.size)?;
            params.dir_radius = [1, 0, radius, 0];
            self.pass(gl, bl, bl_loc, cur.tex, &tmp, &params);
            params.dir_radius = [0, 1, radius, 0];
            self.pass(gl, bl, bl_loc, tmp.tex, &cur, &params);
            self.free.push(tmp);
        }
        // colour operations
        if !ops.is_empty() {
            let mut params = PassParams::new(
                (cur.size.0 as u32, cur.size.1 as u32),
                (cur.size.0 as u32, cur.size.1 as u32),
            );
            params.set_ops(ops);
            let out = self.take_target(gl, cur.size)?;
            self.pass(gl, co, co_loc, cur.tex, &out, &params);
            self.free.push(cur);
            cur = out;
        }
        Some(cur)
    }

    /// Composite one region. The target framebuffer must be bound as draw framebuffer.
    ///
    /// # Safety
    /// Must be called with the renderer's context current (inside `GlesFrame::with_context`).
    pub unsafe fn draw(&mut self, gl: &ffi::Gles2, p: &DrawParams<'_>) -> Result<(), String> {
        if self.progs.is_none() {
            return Err(self
                .failure
                .clone()
                .unwrap_or_else(|| "material pipeline not initialised".into()));
        }
        self.tick += 1;
        let saved = SavedState::capture(gl);
        let src_fbo = saved.draw_fbo as GLuint;
        let key: BackdropKey = (
            p.recipe.key,
            p.output_px.0,
            p.output_px.1,
            p.scale.to_bits(),
            stage_owner(p.stage),
        );
        let hit = self
            .cache
            .iter()
            .position(|c| c.key == key && c.generation == p.generation);
        let idx = match hit {
            Some(i) => {
                self.stats.backdrop_hits += 1;
                i
            }
            None if matches!(p.stage, MaterialStage::Composite { .. }) => {
                saved.restore(gl);
                let msg = "the backdrop captured below the windows is missing".to_string();
                self.failure = Some(msg.clone());
                return Err(msg);
            }
            None => {
                self.stats.backdrop_misses += 1;
                if let Some(i) = self.cache.iter().position(|c| c.key == key) {
                    let old = self.cache.swap_remove(i);
                    self.free.push(old.target);
                }
                let Some(target) = self.process_backdrop(gl, p, src_fbo) else {
                    saved.restore(gl);
                    let msg = "backdrop processing failed (framebuffer incomplete?)".to_string();
                    self.failure = Some(msg.clone());
                    return Err(msg);
                };
                if self.cache.len() >= 8 {
                    // a capture is only redrawn with its scene below, so evicting it would leave its
                    // composite without a backdrop: evict the other backdrops first
                    let oldest = (0..self.cache.len())
                        .min_by_key(|i| (self.cache[*i].key.4.is_some(), self.cache[*i].last_used))
                        .unwrap();
                    let old = self.cache.swap_remove(oldest);
                    self.free.push(old.target);
                }
                self.cache.push(CachedBackdrop {
                    key,
                    generation: p.generation,
                    target,
                    last_used: self.tick,
                });
                self.cache.len() - 1
            }
        };
        self.cache[idx].last_used = self.tick;
        if matches!(p.stage, MaterialStage::Capture { .. }) {
            saved.restore(gl);
            return Ok(());
        }
        let backdrop_tex = self.cache[idx].target.tex;

        let progs = self.progs.as_ref().unwrap();
        let mut region: RegionParams = bytemuck::Zeroable::zeroed();
        region.rect = [
            p.rect.loc.x as f32,
            p.rect.loc.y as f32,
            p.rect.size.w as f32,
            p.rect.size.h as f32,
        ];
        region.shape = [p.corner.0, p.corner.1, p.opacity, 1.0];
        region.info = [0.0, 1.0, 0.0, 0.0];
        region.sizes = [
            p.output_px.0 as f32,
            p.output_px.1 as f32,
            p.output_px.0 as f32,
            p.output_px.1 as f32,
        ];
        region.tint = [p.tint[0], p.tint[1], p.tint[2], 1.0];
        region.set_fills(&p.recipe.fills);

        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, saved.draw_fbo as GLuint);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, saved.read_fbo as GLuint);
        gl.Viewport(
            saved.viewport[0],
            saved.viewport[1],
            saved.viewport[2],
            saved.viewport[3],
        );
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
        gl.UseProgram(progs.material);
        Self::upload_ubo(
            gl,
            self.ubo_region,
            BIND_REGION,
            bytemuck::bytes_of(&region),
        );
        gl.UniformMatrix3fv(
            progs.material_loc.proj,
            1,
            ffi::FALSE,
            p.projection.as_ptr(),
        );
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, backdrop_tex);
        gl.Uniform1i(progs.material_loc.backdrop, 0);
        let bounds = Rectangle::new(
            (p.rect.loc.x - 1, p.rect.loc.y - 1).into(),
            (p.rect.size.w + 2, p.rect.size.h + 2).into(),
        );
        for d in p.damage {
            let Some(q) = d.intersection(bounds) else {
                continue;
            };
            gl.Uniform4f(
                progs.material_loc.quad,
                q.loc.x as f32,
                q.loc.y as f32,
                q.size.w as f32,
                q.size.h as f32,
            );
            gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
            self.stats.composites += 1;
        }
        saved.restore(gl);
        Ok(())
    }

    /// Release all GL objects. Call with the renderer's context current.
    pub fn destroy(&mut self, renderer: &mut GlesRenderer) {
        let free = std::mem::take(&mut self.free);
        let cache = std::mem::take(&mut self.cache);
        let progs = self.progs.take();
        let bufs = [self.ubo_pass, self.ubo_region];
        let _ = renderer.with_context(|gl| unsafe {
            for t in free.into_iter().chain(cache.into_iter().map(|c| c.target)) {
                gl.DeleteFramebuffers(1, &t.fbo);
                gl.DeleteTextures(1, &t.tex);
            }
            if let Some(p) = progs {
                for prog in [p.downsample, p.blur, p.color, p.material] {
                    gl.DeleteProgram(prog);
                }
            }
            gl.DeleteBuffers(2, bufs.as_ptr());
        });
    }
}

/// Material region element. Draw order matters: everything below it in the element list is part
/// of its backdrop. Its id/commit are assigned by [`MaterialElement::finalize`] once the elements
/// below it are known: the commit advances whenever they changed, so the damage tracker repaints
/// the whole region and the backdrop is captured from a freshly drawn scene.
pub struct MaterialElement {
    key: ElementKey,
    stage: MaterialStage,
    id: Id,
    commit: CommitCounter,
    area: Rectangle<f64, Logical>,
    recipe: Rc<GpuRecipe>,
    shared: SharedMaterial,
    generation: u64,
    radius_pt: f64,
    continuous: bool,
    opacity: f32,
    tint: [f32; 3],
    output_px: (i32, i32),
    scale: f64,
}

impl std::fmt::Debug for MaterialElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaterialElement")
            .field("area", &self.area)
            .field("generation", &self.generation)
            .finish()
    }
}

impl MaterialElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: ElementKey,
        stage: MaterialStage,
        cache: &mut ElementCache,
        area: Rectangle<f64, Logical>,
        recipe: Rc<GpuRecipe>,
        shared: SharedMaterial,
        radius_pt: f64,
        continuous: bool,
        opacity: f32,
        tint: [f32; 3],
        output_px: (i32, i32),
        scale: f64,
    ) -> Self {
        let mut e = Self {
            key,
            stage,
            id: Id::new(),
            commit: CommitCounter::default(),
            area,
            recipe,
            shared,
            generation: 0,
            radius_pt,
            continuous,
            opacity,
            tint,
            output_px,
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

    pub fn stage(&self) -> MaterialStage {
        self.stage
    }

    /// Set the backdrop generation (a signature of everything drawn below) and refresh the id and
    /// commit counter from the cache.
    pub fn finalize(&mut self, generation: u64, cache: &mut ElementCache) {
        self.generation = generation;
        let sig = Sig::new()
            .rect(self.area)
            .f(self.radius_pt)
            .f(self.opacity as f64)
            .u(self.recipe.key)
            .u(generation)
            .u(self.continuous as u64)
            .finish();
        let (id, commit) = cache.slot(self.key, sig);
        self.id = id;
        self.commit = commit;
    }
}

impl Element for MaterialElement {
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

impl RenderElement<GlesRenderer> for MaterialElement {
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
        let projection = *frame.projection();
        let (extent, exponent) = {
            let style = if self.continuous {
                CornerStyle::continuous()
            } else {
                CornerStyle::Circular
            };
            (
                (self.radius_pt * style.expansion() * self.scale) as f32,
                style.exponent() as f32,
            )
        };
        let abs: Vec<Rectangle<i32, Physical>> = damage
            .iter()
            .map(|d| Rectangle::new(d.loc + dst.loc, d.size))
            .collect();
        let params = DrawParams {
            stage: self.stage,
            projection,
            recipe: &self.recipe,
            generation: self.generation,
            output_px: self.output_px,
            scale: self.scale as f32,
            rect: dst,
            corner: (extent, exponent),
            opacity: self.opacity,
            tint: self.tint,
            damage: &abs,
        };
        let shared = self.shared.clone();
        let res = frame.with_context(move |gl| unsafe { shared.borrow_mut().draw(gl, &params) })?;
        if let Err(e) = res {
            let mut s = self.shared.borrow_mut();
            if s.failure.is_none() {
                s.failure = Some(e.clone());
            }
            tracing::error!(error = %e, "material pass failed");
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
    use cupertino_theme::{Appearance, MaterialOptions, Theme};

    fn recipe(a: Appearance, m: &str) -> Recipe {
        Theme::load()
            .unwrap()
            .recipe(a, m, &MaterialOptions::default())
            .unwrap()
    }

    #[test]
    fn gpu_recipe_flattens_the_menu_bar() {
        let g = GpuRecipe::new(&recipe(Appearance::Light, "MenuBar"));
        let (scale, sigma, ops) = g.backdrop.clone().unwrap();
        assert_eq!(scale, 0.5);
        assert!(sigma > 0.0);
        assert_eq!(ops.len(), 2);
        let dark = GpuRecipe::new(&recipe(Appearance::Dark, "MenuBar"));
        assert_ne!(g.key, dark.key, "light and dark recipes hash differently");
        assert_eq!(
            g.key,
            GpuRecipe::new(&recipe(Appearance::Light, "MenuBar")).key
        );
    }

    #[test]
    fn uniform_layouts_match_the_shader_blocks() {
        assert_eq!(std::mem::size_of::<PassParams>(), 432);
        assert_eq!(std::mem::size_of::<RegionParams>(), 336);
    }

    #[test]
    fn shaders_declare_the_expected_blocks() {
        assert!(MATERIAL_VERT.contains("uniform mat3 u_proj") && MATERIAL_VERT.contains("u_quad"));
        assert!(
            MATERIAL_FRAG.contains("in vec2 v_uv")
                && !MATERIAL_FRAG.contains("texelFetch(u_source")
        );
        for s in [DOWNSAMPLE_FRAG, BLUR_FRAG, COLOR_FRAG] {
            assert!(s.contains("PassParams") && s.contains("u_src"));
        }
    }
}
