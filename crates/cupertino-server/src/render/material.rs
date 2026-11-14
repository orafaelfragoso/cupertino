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

#[path = "glass.rs"]
pub mod glass;

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

/// Released render targets always kept for reuse, the most recently released first.
const FREE_KEPT: usize = 12;
/// Bytes of released render targets kept for reuse past [`FREE_KEPT`]: glass that moves blurs again each frame at a
/// size that can differ by a pixel from one frame to the next, and keeps its targets of every such size instead of
/// creating new ones.
const FREE_BYTES: usize = 48 << 20;

/// How many of the released targets (`sizes` in texels, oldest first, `texel_bytes` each texel) to delete so that what
/// is kept is the [`FREE_KEPT`] most recent and, past them, the most recent that fit in [`FREE_BYTES`] altogether.
fn free_excess(sizes: &[(i32, i32)], texel_bytes: usize) -> usize {
    let mut bytes = 0usize;
    for (i, size) in sizes.iter().rev().enumerate() {
        bytes += size.0.max(0) as usize * size.1.max(0) as usize * texel_bytes;
        if i >= FREE_KEPT && bytes > FREE_BYTES {
            return sizes.len() - i;
        }
    }
    0
}

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

/// Identity of a processed backdrop: recipe, capture size, pixel scale, for backdrops captured by a
/// [`MaterialStage::Capture`] element the owner they were captured for, and for a [`MaterialStage::Full`] element the
/// element itself, so regions of one recipe at different depths of the scene keep a backdrop each instead of taking
/// one another's place every frame.
type BackdropKey = (u64, i32, i32, u32, Option<u64>, u64);

/// The [`BackdropKey`] of a draw of `element` (a hash of its [`ElementKey`]).
fn backdrop_key(
    recipe: u64,
    output_px: (i32, i32),
    scale: f32,
    stage: MaterialStage,
    element: u64,
) -> BackdropKey {
    let full = match stage {
        MaterialStage::Full => element,
        // a composite finds the backdrop its capture element made
        MaterialStage::Capture { .. } | MaterialStage::Composite { .. } => 0,
    };
    (
        recipe,
        output_px.0,
        output_px.1,
        scale.to_bits(),
        stage_owner(stage),
        full,
    )
}

struct CachedBackdrop {
    key: BackdropKey,
    generation: u64,
    target: Target,
    /// The last level of the pyramid the backdrop was filtered from, when it was filtered.
    pyramid: Option<Target>,
    /// The part of the framebuffer the backdrop was captured and blurred from.
    crop: Crop,
    last_used: u64,
}

impl CachedBackdrop {
    fn targets(self) -> impl Iterator<Item = Target> {
        std::iter::once(self.target).chain(self.pyramid)
    }
}

/// A rectangle of the framebuffer, in its pixels (row 0 where the framebuffer has it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Crop {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Crop {
    fn whole(size: (i32, i32)) -> Self {
        Crop {
            x: 0,
            y: 0,
            w: size.0,
            h: size.1,
        }
    }

    fn contains(&self, other: &Crop) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + other.w <= self.x + self.w
            && other.y + other.h <= self.y + self.h
    }
}

/// The part of a `size` framebuffer a region drawing over `area` (output pixels, placed by `projection`) needs
/// blurred: the area with a margin of `radius` texels of the blur's last level, and one more for the filtered lookup,
/// at the `levels` halvings of the blur, so every pixel the region shows is blurred from the same pixels as in a blur
/// of the whole framebuffer. Its edges lie on the grid of the last level, so each level halves it exactly; a
/// framebuffer that does not halve exactly `levels` times is blurred whole.
fn backdrop_crop(
    size: (i32, i32),
    levels: usize,
    radius: usize,
    area: Rectangle<i32, Physical>,
    projection: &[f32; 9],
) -> Crop {
    let whole = Crop::whole(size);
    let factor = 1i32 << levels.min(16);
    if size.0 <= 0 || size.1 <= 0 || size.0 % factor != 0 || size.1 % factor != 0 {
        return whole;
    }
    let margin = (radius as i32 + 2) * factor;
    let grown = Rectangle::new(
        (area.loc.x - margin, area.loc.y - margin).into(),
        (area.size.w + 2 * margin, area.size.h + 2 * margin).into(),
    );
    let Some((min_x, min_y, max_x, max_y)) = framebuffer_bounds(size, grown, projection) else {
        return whole;
    };
    let down = |v: f32, max: i32| ((v.floor() as i32).clamp(0, max) / factor) * factor;
    let up = |v: f32, max: i32| {
        (((v.ceil() as i32).clamp(0, max) + factor - 1) / factor * factor).min(max)
    };
    let (cx0, cy0) = (down(min_x, size.0), down(min_y, size.1));
    let (cx1, cy1) = (up(max_x, size.0), up(max_y, size.1));
    if cx1 <= cx0 || cy1 <= cy0 {
        return whole;
    }
    Crop {
        x: cx0,
        y: cy0,
        w: cx1 - cx0,
        h: cy1 - cy0,
    }
}

/// Where `rect` (output pixels) lands in a `size` framebuffer through `projection`: its least and greatest x and y in
/// framebuffer pixels, unclamped; none when the projection sends it nowhere.
fn framebuffer_bounds(
    size: (i32, i32),
    rect: Rectangle<i32, Physical>,
    projection: &[f32; 9],
) -> Option<(f32, f32, f32, f32)> {
    let (x0, y0) = (rect.loc.x, rect.loc.y);
    let (x1, y1) = (rect.loc.x + rect.size.w, rect.loc.y + rect.size.h);
    // the corners in framebuffer pixels, through the projection the composite draws with
    let m = projection;
    let corner = |x: i32, y: i32| {
        let (x, y) = (x as f32, y as f32);
        let cx = m[0] * x + m[3] * y + m[6];
        let cy = m[1] * x + m[4] * y + m[7];
        (
            (cx * 0.5 + 0.5) * size.0 as f32,
            (cy * 0.5 + 0.5) * size.1 as f32,
        )
    };
    let corners = [
        corner(x0, y0),
        corner(x1, y0),
        corner(x0, y1),
        corner(x1, y1),
    ];
    let min_x = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min);
    let max_x = corners
        .iter()
        .map(|c| c.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min);
    let max_y = corners
        .iter()
        .map(|c| c.1)
        .fold(f32::NEG_INFINITY, f32::max);
    (min_x.is_finite() && max_x.is_finite() && min_y.is_finite() && max_y.is_finite())
        .then_some((min_x, min_y, max_x, max_y))
}

/// A rectangle of texels of a blur's last level, in the framebuffer's texel grid: from `x0`, `y0` up to, not
/// including, `x1`, `y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Texels {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Texels {
    /// The texels of `crop`, whose edges lie on the grid of `factor` pixels.
    fn of(crop: Crop, factor: i32) -> Self {
        Texels {
            x0: crop.x / factor,
            y0: crop.y / factor,
            x1: (crop.x + crop.w) / factor,
            y1: (crop.y + crop.h) / factor,
        }
    }

    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    fn count(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            i64::from(self.x1 - self.x0) * i64::from(self.y1 - self.y0)
        }
    }

    fn meet(&self, o: &Texels) -> Option<Texels> {
        let t = Texels {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        };
        (!t.is_empty()).then_some(t)
    }

    fn join(&self, o: &Texels) -> Texels {
        Texels {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    /// These texels but those of `o`: up to four rectangles, the rows above and below `o` and the columns beside it.
    fn minus(&self, o: &Texels) -> Vec<Texels> {
        let Some(o) = self.meet(o) else {
            return vec![*self];
        };
        [
            Texels { y1: o.y0, ..*self },
            Texels { y0: o.y1, ..*self },
            Texels {
                x1: o.x0,
                y0: o.y0,
                y1: o.y1,
                ..*self
            },
            Texels {
                x0: o.x1,
                y0: o.y0,
                y1: o.y1,
                ..*self
            },
        ]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect()
    }

    /// The part of the framebuffer (pixels) these texels are made from.
    fn crop(&self, factor: i32) -> Crop {
        Crop {
            x: self.x0 * factor,
            y: self.y0 * factor,
            w: (self.x1 - self.x0) * factor,
            h: (self.y1 - self.y0) * factor,
        }
    }
}

/// How a backdrop's pyramid is brought up to date: which of its last level's texels it keeps and which are made again.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Reblur {
    /// The texels taken from the pyramid as it was.
    kept: Option<Texels>,
    /// The texels made again, each part from its own pixels of the framebuffer.
    parts: Vec<Texels>,
}

/// The most parts a pyramid is made again in: each runs the capture and the pyramid's passes.
const REBLUR_PARTS: usize = 8;

/// `parts` with those that overlap or lie close joined, while the joined rectangle holds little more than the two, and
/// all of them joined in one when they stay many.
fn merged(mut parts: Vec<Texels>) -> Vec<Texels> {
    'joining: loop {
        for i in 0..parts.len() {
            for j in i + 1..parts.len() {
                let join = parts[i].join(&parts[j]);
                if join.count() * 4 <= (parts[i].count() + parts[j].count()) * 5 {
                    parts[i] = join;
                    parts.swap_remove(j);
                    continue 'joining;
                }
            }
        }
        break;
    }
    if parts.len() > REBLUR_PARTS {
        let all = parts.iter().copied().reduce(|a, b| a.join(&b));
        return all.into_iter().collect();
    }
    parts
}

/// How to bring the pyramid of `old` (framebuffer pixels) up to date for `new`, in a `size` framebuffer halved
/// `levels` times (none: a copy of the pixels, for a recipe of colour operations alone), given the rectangles where
/// the framebuffer changed since (`changed`, framebuffer pixels: least and greatest x and y). Each texel of the last
/// level is the mean of the pixels of its own square, so a texel of `new`
/// is kept when `old` had it and no change touches its square, and made again from its square otherwise; the
/// Gaussian then runs over the whole level, as it would after a whole capture. None when the parts made again cover
/// most of `new`, which is then captured whole.
fn plan_reblur(
    size: (i32, i32),
    levels: usize,
    old: Crop,
    new: Crop,
    changed: &[(f32, f32, f32, f32)],
) -> Option<Reblur> {
    let factor = 1i32 << levels.min(16);
    if size.0 % factor != 0 || size.1 % factor != 0 {
        return None;
    }
    let (new_t, old_t) = (Texels::of(new, factor), Texels::of(old, factor));
    let kept = old_t.meet(&new_t);
    let mut parts = match kept {
        Some(k) => new_t.minus(&k),
        None => vec![new_t],
    };
    if let Some(k) = kept {
        // the squares a change touches
        let touched: Vec<Texels> = changed
            .iter()
            .filter_map(|&(x0, y0, x1, y1)| {
                Texels {
                    x0: (x0.floor() as i32).div_euclid(factor),
                    y0: (y0.floor() as i32).div_euclid(factor),
                    x1: (x1.ceil() as i32 + factor - 1).div_euclid(factor),
                    y1: (y1.ceil() as i32 + factor - 1).div_euclid(factor),
                }
                .meet(&k)
            })
            .collect();
        parts.extend(merged(touched));
    }
    let made: i64 = parts.iter().map(Texels::count).sum();
    (made * 4 < new_t.count() * 3).then_some(Reblur { kept, parts })
}
/// Counters for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialStats {
    pub backdrop_hits: u64,
    pub backdrop_misses: u64,
    pub passes: u64,
    pub composites: u64,
    /// Render targets (texture and framebuffer) created: a pass that keeps reusing its targets creates none.
    pub allocations: u64,
    /// Passes of a backdrop blur: the pyramid's downsamples and the Gaussian's two directions.
    pub blur_passes: u64,
    /// Texels the backdrop blurs wrote: the capture's first level, the pyramid's and the Gaussian's two directions.
    pub blur_texels: u64,
    /// Backdrops brought up to date by blurring again only where they changed or newly reach.
    pub reblurs: u64,
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
    /// Time the glass passes (the frame-rate benchmark only): with GPU timer queries ([`MaterialGl::gpu_timers`]) the
    /// GPU measures each pass; without them the GPU is waited for around each, which stalls the pipeline.
    pub timing: bool,
    /// The context has GPU timer queries.
    pub gpu_timers: bool,
    /// The timer queries of the passes timed and not yet read.
    timers: PassTimers,
    /// What the glass passes took since the benchmark last cleared it.
    pub glass_time: crate::frame_stats::GlassTime,
    failure: Option<String>,
    glass: glass::GlassGl,
}

pub type SharedMaterial = Rc<RefCell<MaterialGl>>;

/// `GL_TIME_ELAPSED_EXT` of the timer query extension.
const TIME_ELAPSED: GLenum = 0x88BF;

/// The benchmark's GPU timer queries.
#[derive(Debug, Default)]
struct PassTimers {
    free: Vec<GLuint>,
    /// Ended queries and whether each timed a backdrop blur.
    pending: Vec<(GLuint, bool)>,
    open: Option<(GLuint, bool)>,
}

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
    /// The element drawing, a hash of its [`ElementKey`].
    pub element: u64,
    pub projection: [f32; 9],
    pub recipe: &'a GpuRecipe,
    pub generation: u64,
    /// Output size in pixels (capture size).
    pub output_px: (i32, i32),
    /// Pixel scale of the output (points to pixels).
    pub scale: f32,
    /// The rectangle of the region's rounded mask in output pixels: the region's own, or the outline of the window it
    /// is cut to ([`MaterialMask`]).
    pub rect: Rectangle<i32, Physical>,
    /// Corner extent (px), superellipse exponent of the mask.
    pub corner: (f32, f32),
    /// Where the region draws, in output pixels: the region's own rectangle, which the mask may reach past.
    pub area: Rectangle<i32, Physical>,
    pub opacity: f32,
    pub tint: [f32; 3],
    /// Rectangles (absolute output pixels) to draw.
    pub damage: &'a [Rectangle<i32, Physical>],
    /// A generation the backdrop had before and the rectangles (output pixels) where it changed since: a backdrop
    /// kept from then is blurred again only around them.
    pub changed: Option<(u64, &'a [Rectangle<i32, Physical>])>,
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
            timing: false,
            gpu_timers: false,
            timers: PassTimers::default(),
            glass_time: Default::default(),
            failure: None,
            glass: glass::GlassGl::default(),
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
        self.glass = glass::GlassGl::init(gl)?;
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
            timing: false,
            gpu_timers: false,
            timers: PassTimers::default(),
            glass_time: Default::default(),
            failure: None,
            glass: glass::GlassGl::default(),
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
        self.stats.allocations += 1;
        Some(Target { tex, fbo, size })
    }

    unsafe fn destroy_target(&self, gl: &ffi::Gles2, t: Target) {
        gl.DeleteFramebuffers(1, &t.fbo);
        gl.DeleteTextures(1, &t.tex);
    }

    unsafe fn take_target(&mut self, gl: &ffi::Gles2, size: (i32, i32)) -> Option<Target> {
        if let Some(i) = self.free.iter().rposition(|t| t.size == size) {
            return Some(self.free.remove(i));
        }
        self.trim_free(gl);
        let float = self.float_targets;
        self.alloc_target(gl, size, float)
    }

    /// Delete the released targets past what [`free_excess`] keeps, the oldest first.
    unsafe fn trim_free(&mut self, gl: &ffi::Gles2) {
        let texel_bytes = if self.float_targets { 8 } else { 4 };
        let sizes: Vec<(i32, i32)> = self.free.iter().map(|t| t.size).collect();
        let excess = free_excess(&sizes, texel_bytes);
        for t in self.free.drain(..excess).collect::<Vec<_>>() {
            self.destroy_target(gl, t);
        }
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
        if self
            .progs
            .as_ref()
            .is_some_and(|p| program == p.downsample || program == p.blur)
        {
            self.stats.blur_passes += 1;
            self.stats.blur_texels += (dst.size.0 as u64) * (dst.size.1 as u64);
        }
    }

    /// The last level of the pyramid of the `crop` part of the bound framebuffer (`src_fbo`) for `p`'s recipe: the
    /// first level a linear blit (a 2x2 box average at half size), each next one a 2x2 box average of the one before.
    unsafe fn pyramid(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        src_fbo: GLuint,
        crop: Crop,
    ) -> Option<Target> {
        let (w, h) = p.output_px;
        let mut plan: BlurPlan = self.plan(p);
        if crop != Crop::whole((w, h)) {
            // the same halvings of the part: its edges lie on the grid of the last level
            for (i, level) in plan.levels.iter_mut().enumerate() {
                *level = ((crop.w >> (i + 1)) as u32, (crop.h >> (i + 1)) as u32);
            }
        }
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        let first = plan
            .levels
            .first()
            .map(|l| (l.0 as i32, l.1 as i32))
            .unwrap_or((crop.w, crop.h));
        let mut cur = self.take_target(gl, first)?;
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, src_fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, cur.fbo);
        let filter = if plan.levels.is_empty() {
            ffi::NEAREST
        } else {
            ffi::LINEAR
        };
        gl.BlitFramebuffer(
            crop.x,
            crop.y,
            crop.x + crop.w,
            crop.y + crop.h,
            0,
            0,
            first.0,
            first.1,
            ffi::COLOR_BUFFER_BIT,
            filter,
        );
        self.stats.passes += 1;
        self.stats.blur_texels += (first.0 as u64) * (first.1 as u64);
        let (ds, ds_loc) = {
            let progs = self.progs.as_ref()?;
            (progs.downsample, progs.src_loc[0])
        };
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
        Some(cur)
    }

    /// Whether `p`'s recipe filters its pyramid (a Gaussian or colour operations) or shows it as it is.
    fn filters(&self, p: &DrawParams<'_>) -> bool {
        let ops = p.recipe.backdrop.as_ref().is_some_and(|b| !b.2.is_empty());
        ops || self.plan(p).weights.len() > 1
    }

    /// `pyramid` filtered into a new target for `p`'s recipe: the separable Gaussian, then the colour operations. The
    /// pyramid is left as it is. Only for a recipe that [`filters`](Self::filters).
    unsafe fn filtered(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        pyramid: &Target,
    ) -> Option<Target> {
        let plan = self.plan(p);
        let ops = p
            .recipe
            .backdrop
            .as_ref()
            .map_or(&[][..], |b| b.2.as_slice());
        let (bl, bl_loc, co, co_loc) = {
            let progs = self.progs.as_ref()?;
            (progs.blur, progs.src_loc[1], progs.color, progs.src_loc[2])
        };
        let size = pyramid.size;
        let dims = (size.0 as u32, size.1 as u32);
        let mut cur: Option<Target> = None;
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        // separable Gaussian
        if plan.weights.len() > 1 {
            let mut params = PassParams::new(dims, dims);
            params.set_weights(&plan.weights);
            let radius = (plan.weights.len() - 1) as i32;
            let tmp = self.take_target(gl, size)?;
            let out = self.take_target(gl, size)?;
            params.dir_radius = [1, 0, radius, 0];
            self.pass(gl, bl, bl_loc, pyramid.tex, &tmp, &params);
            params.dir_radius = [0, 1, radius, 0];
            self.pass(gl, bl, bl_loc, tmp.tex, &out, &params);
            self.free.push(tmp);
            cur = Some(out);
        }
        // colour operations
        if !ops.is_empty() {
            let mut params = PassParams::new(dims, dims);
            params.set_ops(ops);
            let out = self.take_target(gl, size)?;
            let src = cur.as_ref().map_or(pyramid.tex, |c| c.tex);
            self.pass(gl, co, co_loc, src, &out, &params);
            if let Some(c) = cur {
                self.free.push(c);
            }
            cur = Some(out);
        }
        cur
    }

    /// Process the backdrop for `recipe`: capture the `crop` part of the bound framebuffer and filter it. The
    /// backdrop, and the pyramid it was filtered from when it was ([`filters`](Self::filters)), kept so a later
    /// change makes again only the part of it that changed.
    unsafe fn process_backdrop(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        src_fbo: GLuint,
        crop: Crop,
    ) -> Option<(Target, Option<Target>)> {
        let pyramid = self.pyramid(gl, p, src_fbo, crop)?;
        if !self.filters(p) {
            return Some((pyramid, None));
        }
        let out = self.filtered(gl, p, &pyramid)?;
        Some((out, Some(pyramid)))
    }

    /// The blur of `p`'s recipe over its whole framebuffer.
    fn plan(&self, p: &DrawParams<'_>) -> BlurPlan {
        let (w, h) = p.output_px;
        let (scale, sigma) = p
            .recipe
            .backdrop
            .as_ref()
            .map_or((1.0, 0.0), |b| (b.0, b.1));
        plan_blur(w as u32, h as u32, scale, sigma, p.scale)
    }

    /// The part of the framebuffer `p` needs blurred: around its area for a region that blurs what is under it as it
    /// draws, all of it for a backdrop captured below the windows, which another element composites.
    fn crop(&self, p: &DrawParams<'_>) -> Crop {
        if !matches!(p.stage, MaterialStage::Full) {
            return Crop::whole(p.output_px);
        }
        let plan = self.plan(p);
        let radius = plan.weights.len().saturating_sub(1);
        backdrop_crop(
            p.output_px,
            plan.levels.len(),
            radius,
            p.area,
            &p.projection,
        )
    }

    /// How the backdrop `old` kept for `p`'s key is brought up to date for `crop`: when its pyramid was made from the
    /// framebuffer as it is now, or as it was at the generation `p` knows the changes since, its pyramid is made again
    /// only where those changes or `crop` reach past it. None to make it whole.
    fn reblur_plan(&self, p: &DrawParams<'_>, old: &CachedBackdrop, crop: Crop) -> Option<Reblur> {
        old.pyramid.as_ref()?;
        let changed: Vec<(f32, f32, f32, f32)> = if old.generation == p.generation {
            Vec::new()
        } else {
            let (_, rects) = p.changed.filter(|(since, _)| *since == old.generation)?;
            rects
                .iter()
                .map(|r| framebuffer_bounds(p.output_px, *r, &p.projection))
                .collect::<Option<_>>()?
        };
        plan_reblur(
            p.output_px,
            self.plan(p).levels.len(),
            old.crop,
            crop,
            &changed,
        )
    }

    /// One texel rectangle `at` of `from` (whose first texel is `from_at`) into `to` (first texel `to_at`).
    unsafe fn copy_texels(
        &mut self,
        gl: &ffi::Gles2,
        from: &Target,
        from_at: (i32, i32),
        to: &Target,
        to_at: (i32, i32),
        at: Texels,
    ) {
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, from.fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, to.fbo);
        let (w, h) = (at.x1 - at.x0, at.y1 - at.y0);
        let (sx, sy) = (at.x0 - from_at.0, at.y0 - from_at.1);
        let (dx, dy) = (at.x0 - to_at.0, at.y0 - to_at.1);
        gl.BlitFramebuffer(
            sx,
            sy,
            sx + w,
            sy + h,
            dx,
            dy,
            dx + w,
            dy + h,
            ffi::COLOR_BUFFER_BIT,
            ffi::NEAREST,
        );
        self.stats.passes += 1;
    }

    /// The backdrop for `crop` made from `old`'s: its pyramid's texels `plan` keeps copied, each of its parts made
    /// again from its own pixels of the framebuffer, and the whole filtered as after a whole capture.
    unsafe fn reblur(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        src_fbo: GLuint,
        old: CachedBackdrop,
        crop: Crop,
        plan: &Reblur,
    ) -> Option<(Target, Option<Target>)> {
        let factor = 1i32 << self.plan(p).levels.len().min(16);
        let (new_t, old_t) = (Texels::of(crop, factor), Texels::of(old.crop, factor));
        let at = (new_t.x0, new_t.y0);
        self.free.push(old.target);
        let old_pyramid = old.pyramid?;
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        let pyramid = if old.crop == crop {
            old_pyramid
        } else {
            let Some(pyramid) = self.take_target(gl, (new_t.x1 - new_t.x0, new_t.y1 - new_t.y0))
            else {
                self.free.push(old_pyramid);
                return None;
            };
            if let Some(kept) = plan.kept {
                self.copy_texels(gl, &old_pyramid, (old_t.x0, old_t.y0), &pyramid, at, kept);
            }
            self.free.push(old_pyramid);
            pyramid
        };
        for &part in &plan.parts {
            let Some(made) = self.pyramid(gl, p, src_fbo, part.crop(factor)) else {
                self.free.push(pyramid);
                return None;
            };
            self.copy_texels(gl, &made, (part.x0, part.y0), &pyramid, at, part);
            self.free.push(made);
        }
        let Some(out) = self.filtered(gl, p, &pyramid) else {
            self.free.push(pyramid);
            return None;
        };
        self.stats.reblurs += 1;
        Some((out, Some(pyramid)))
    }

    /// Composite one region. The target framebuffer must be bound as draw framebuffer.
    ///
    /// # Safety
    /// Must be called with the renderer's context current (inside `GlesFrame::with_context`).
    pub unsafe fn draw(&mut self, gl: &ffi::Gles2, p: &DrawParams<'_>) -> Result<(), String> {
        if !self.timing || self.gpu_timers {
            return self.draw_now(gl, p);
        }
        gl.Finish();
        let start = std::time::Instant::now();
        let result = self.draw_now(gl, p);
        gl.Finish();
        self.glass_time.total += start.elapsed();
        result
    }

    unsafe fn draw_now(&mut self, gl: &ffi::Gles2, p: &DrawParams<'_>) -> Result<(), String> {
        if self.progs.is_none() {
            return Err(self
                .failure
                .clone()
                .unwrap_or_else(|| "material pipeline not initialised".into()));
        }
        self.tick += 1;
        let saved = SavedState::capture(gl);
        let src_fbo = saved.draw_fbo as GLuint;
        let key = backdrop_key(p.recipe.key, p.output_px, p.scale, p.stage, p.element);
        let crop = self.crop(p);
        let hit = self
            .cache
            .iter()
            .position(|c| c.key == key && c.generation == p.generation && c.crop.contains(&crop));
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
                let old = self
                    .cache
                    .iter()
                    .position(|c| c.key == key)
                    .map(|i| self.cache.swap_remove(i));
                let reblur = old.as_ref().and_then(|old| self.reblur_plan(p, old, crop));
                let blurring = (self.timing && !self.gpu_timers).then(std::time::Instant::now);
                self.timer_begin(gl, true);
                let processed = match (old, reblur) {
                    (Some(old), Some(plan)) => self.reblur(gl, p, src_fbo, old, crop, &plan),
                    (old, _) => {
                        if let Some(old) = old {
                            self.free.extend(old.targets());
                        }
                        self.process_backdrop(gl, p, src_fbo, crop)
                    }
                };
                self.timer_end(gl);
                if let Some(blurring) = blurring {
                    gl.Finish();
                    self.glass_time.blur += blurring.elapsed();
                }
                let Some((target, pyramid)) = processed else {
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
                    self.free.extend(old.targets());
                }
                self.cache.push(CachedBackdrop {
                    key,
                    generation: p.generation,
                    target,
                    pyramid,
                    crop,
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
        let held = self.cache[idx].crop;
        self.timer_begin(gl, false);

        let progs = self.progs.as_ref().unwrap();
        let mut region: RegionParams = bytemuck::Zeroable::zeroed();
        region.rect = [
            p.rect.loc.x as f32,
            p.rect.loc.y as f32,
            p.rect.size.w as f32,
            p.rect.size.h as f32,
        ];
        region.shape = [p.corner.0, p.corner.1, p.opacity, 1.0];
        // the backdrop covers the part of the framebuffer it was blurred from: its origin and size
        region.info = [0.0, 1.0, held.x as f32, held.y as f32];
        region.sizes = [
            p.output_px.0 as f32,
            p.output_px.1 as f32,
            held.w as f32,
            held.h as f32,
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
        // a pixel more for the mask's antialiased edge, where the mask is the region's own; a region cut to a window
        // ends at its own rectangle
        let grow = i32::from(p.area == p.rect);
        let bounds = Rectangle::new(
            (p.area.loc.x - grow, p.area.loc.y - grow).into(),
            (p.area.size.w + 2 * grow, p.area.size.h + 2 * grow).into(),
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
        self.timer_end(gl);
        saved.restore(gl);
        Ok(())
    }

    /// Start timing a pass on the GPU (the benchmark with timer queries only); `blur` for a backdrop blur.
    unsafe fn timer_begin(&mut self, gl: &ffi::Gles2, blur: bool) {
        if !(self.timing && self.gpu_timers) || self.timers.open.is_some() {
            return;
        }
        let q = self.timers.free.pop().unwrap_or_else(|| {
            let mut q = 0;
            gl.GenQueries(1, &mut q);
            q
        });
        gl.BeginQuery(TIME_ELAPSED, q);
        self.timers.open = Some((q, blur));
    }

    /// End the pass timed by [`MaterialGl::timer_begin`].
    unsafe fn timer_end(&mut self, gl: &ffi::Gles2) {
        if let Some(open) = self.timers.open.take() {
            gl.EndQuery(TIME_ELAPSED);
            self.timers.pending.push(open);
        }
    }

    /// Once the frame's GPU work finished: add what the timed passes took to [`MaterialGl::glass_time`].
    ///
    /// # Safety
    /// Must be called with the renderer's context current.
    pub unsafe fn collect_pass_times(&mut self, gl: &ffi::Gles2) {
        self.timer_end(gl);
        for (q, blur) in std::mem::take(&mut self.timers.pending) {
            let mut ns: GLuint = 0;
            gl.GetQueryObjectuiv(q, ffi::QUERY_RESULT, &mut ns);
            let took = std::time::Duration::from_nanos(u64::from(ns));
            if blur {
                self.glass_time.blur += took;
            }
            self.glass_time.total += took;
            self.timers.free.push(q);
        }
    }

    /// Release all GL objects. Call with the renderer's context current.
    pub fn destroy(&mut self, renderer: &mut GlesRenderer) {
        let mut free = std::mem::take(&mut self.free);
        free.extend(self.glass.targets());
        let cache = std::mem::take(&mut self.cache);
        let progs = self.progs.take();
        let bufs = [self.ubo_pass, self.ubo_region];
        let _ = renderer.with_context(|gl| unsafe {
            for t in free
                .into_iter()
                .chain(cache.into_iter().flat_map(CachedBackdrop::targets))
            {
                gl.DeleteFramebuffers(1, &t.fbo);
                gl.DeleteTextures(1, &t.tex);
            }
            if let Some(p) = progs {
                for prog in [p.downsample, p.blur, p.color, p.material] {
                    gl.DeleteProgram(prog);
                }
            }
            gl.DeleteBuffers(2, bufs.as_ptr());
            for q in self
                .timers
                .free
                .drain(..)
                .chain(self.timers.pending.drain(..).map(|(q, _)| q))
            {
                gl.DeleteQueries(1, &q);
            }
            self.glass.destroy(gl);
        });
    }
}

/// The outline a material region or glass container is cut to besides its own shape: the rounded outline of the window
/// whose backdrop it is, so a backdrop that reaches the window's corners follows them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialMask {
    pub rect: Rectangle<f64, Logical>,
    pub radius_pt: f64,
    pub continuous: bool,
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
    mask: Option<MaterialMask>,
    /// The last frame's backdrop generation and where the backdrop changed since (output pixels), when known.
    changed: Option<(u64, Vec<Rectangle<i32, Physical>>)>,
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
            mask: None,
            changed: None,
        };
        // the id until `finalize`, which sets the commit once a frame: asking with a provisional signature would
        // advance it on every frame, and the damage tracker would repaint the whole region each time
        (e.id, e.commit) = cache.peek(key, e.sig(0));
        e
    }

    /// The region cut to `mask` as well as to its own rectangle; the mask counts from the next [`Self::finalize`].
    pub fn masked(mut self, mask: MaterialMask) -> Self {
        self.mask = Some(mask);
        self
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

    /// The key its id, commit and backdrop record are kept under.
    pub fn key(&self) -> ElementKey {
        self.key
    }

    /// The part of the output (output pixels) whose pixels reach what the region shows: its rectangle with the reach
    /// of its blur around it ([`backdrop_crop`]'s margin) for a region that blurs what is under it, the whole output
    /// for a backdrop captured below the windows.
    pub fn sampled_area(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let whole = Rectangle::from_size(Size::from(self.output_px));
        if !matches!(self.stage, MaterialStage::Full) {
            return whole;
        }
        let own = to_physical_rect(self.area, scale);
        let Some((capture, sigma, _)) = self.recipe.backdrop.as_ref() else {
            return own;
        };
        let (w, h) = self.output_px;
        let plan = plan_blur(
            w.max(0) as u32,
            h.max(0) as u32,
            *capture,
            *sigma,
            self.scale as f32,
        );
        let factor = 1i32 << plan.levels.len().min(16);
        let margin = (plan.weights.len().saturating_sub(1) as i32 + 2) * factor;
        Rectangle::new(
            (own.loc.x - margin, own.loc.y - margin).into(),
            (own.size.w + 2 * margin, own.size.h + 2 * margin).into(),
        )
    }

    /// Set the backdrop generation (a signature of everything drawn below) and refresh the id and
    /// commit counter from the cache.
    /// Where the backdrop changed since the last frame's generation, so the pass blurs again only that part of it.
    pub fn changed_since(&mut self, since: Option<(u64, Vec<Rectangle<i32, Physical>>)>) {
        self.changed = since;
    }

    pub fn finalize(&mut self, generation: u64, cache: &mut ElementCache) {
        self.generation = generation;
        let (id, commit) = cache.slot(self.key, self.sig(generation));
        self.id = id;
        self.commit = commit;
    }

    /// Everything that changes how the region looks: its shape, opacity, recipe and the backdrop generation.
    fn sig(&self, generation: u64) -> u64 {
        let mut sig = Sig::new()
            .rect(self.area)
            .f(self.radius_pt)
            .f(self.opacity as f64)
            .u(self.recipe.key)
            .u(generation)
            .u(self.continuous as u64);
        if let Some(mask) = self.mask {
            sig = sig
                .rect(mask.rect)
                .f(mask.radius_pt)
                .u(mask.continuous as u64);
        }
        sig.finish()
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
        let corner = |radius_pt: f64, continuous: bool| {
            let style = if continuous {
                CornerStyle::continuous()
            } else {
                CornerStyle::Circular
            };
            (
                (radius_pt * style.expansion() * self.scale) as f32,
                style.exponent() as f32,
            )
        };
        let (rect, corner) = match self.mask {
            // the mask where the region's own rectangle is drawn, `dst`
            Some(mask) => {
                let scale = Scale::from(self.scale);
                let own = to_physical_rect(self.area, scale);
                let outline = to_physical_rect(mask.rect, scale);
                (
                    Rectangle::new(outline.loc + (dst.loc - own.loc), outline.size),
                    corner(mask.radius_pt, mask.continuous),
                )
            }
            None => (dst, corner(self.radius_pt, self.continuous)),
        };
        let abs: Vec<Rectangle<i32, Physical>> = damage
            .iter()
            .map(|d| Rectangle::new(d.loc + dst.loc, d.size))
            .collect();
        let params = DrawParams {
            stage: self.stage,
            element: {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                self.key.hash(&mut h);
                h.finish()
            },
            projection,
            recipe: &self.recipe,
            generation: self.generation,
            output_px: self.output_px,
            scale: self.scale as f32,
            rect,
            corner,
            area: dst,
            opacity: self.opacity,
            tint: self.tint,
            damage: &abs,
            changed: self.changed.as_ref().map(|(g, r)| (*g, r.as_slice())),
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

    /// The projection of a `size` output onto its framebuffer, rows flipped (`flip`) or not.
    fn ortho(size: (i32, i32), flip: bool) -> [f32; 9] {
        let (w, h) = (size.0 as f32, size.1 as f32);
        let sy = if flip { -2.0 / h } else { 2.0 / h };
        let ty = if flip { 1.0 } else { -1.0 };
        [2.0 / w, 0.0, 0.0, 0.0, sy, 0.0, -1.0, ty, 1.0]
    }

    /// The blur of the pipeline on the CPU: `levels` 2x2 box halvings, each texel read clamped to the image, then the
    /// separable `weights` with reads clamped to the image too.
    fn blur(
        img: &[f32],
        size: (usize, usize),
        levels: usize,
        weights: &[f32],
    ) -> (Vec<f32>, (usize, usize)) {
        let (mut cur, mut s) = (img.to_vec(), size);
        for _ in 0..levels {
            let n = (s.0 / 2, s.1 / 2);
            let at = |v: &[f32], x: usize, y: usize| v[y.min(s.1 - 1) * s.0 + x.min(s.0 - 1)];
            let mut next = vec![0.0; n.0 * n.1];
            for y in 0..n.1 {
                for x in 0..n.0 {
                    let (px, py) = (2 * x, 2 * y);
                    next[y * n.0 + x] = (at(&cur, px, py)
                        + at(&cur, px + 1, py)
                        + at(&cur, px, py + 1)
                        + at(&cur, px + 1, py + 1))
                        * 0.25;
                }
            }
            (cur, s) = (next, n);
        }
        let r = weights.len() as i64 - 1;
        for dir in [(1i64, 0i64), (0, 1)] {
            let at = |v: &[f32], x: i64, y: i64| {
                v[y.clamp(0, s.1 as i64 - 1) as usize * s.0 + x.clamp(0, s.0 as i64 - 1) as usize]
            };
            let mut out = vec![0.0; s.0 * s.1];
            for y in 0..s.1 as i64 {
                for x in 0..s.0 as i64 {
                    let mut acc = at(&cur, x, y) * weights[0];
                    for i in 1..=r {
                        acc += (at(&cur, x + dir.0 * i, y + dir.1 * i)
                            + at(&cur, x - dir.0 * i, y - dir.1 * i))
                            * weights[i as usize];
                    }
                    out[y as usize * s.0 + x as usize] = acc;
                }
            }
            cur = out;
        }
        (cur, s)
    }

    #[test]
    fn a_region_blurs_only_the_part_of_the_framebuffer_it_shows_and_finds_the_same_blur_there() {
        let size = (1024, 640);
        let plan = plan_blur(size.0 as u32, size.1 as u32, 0.5, 20.0, 2.0);
        let (levels, radius) = (plan.levels.len(), plan.weights.len() - 1);
        assert!(
            levels >= 2 && radius >= 2,
            "a blur of several levels and a wide kernel"
        );
        let factor = 1 << levels;
        // a framebuffer of noise
        let mut seed = 0x9e37_79b9u32;
        let img: Vec<f32> = (0..size.0 * size.1)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed % 1000) as f32 / 1000.0
            })
            .collect();
        let (whole, ws) = blur(
            &img,
            (size.0 as usize, size.1 as usize),
            levels,
            &plan.weights,
        );
        for flip in [false, true] {
            for area in [
                Rectangle::new((300, 200).into(), (150, 260).into()),
                Rectangle::new((0, 0).into(), (90, 60).into()),
                Rectangle::new((900, 560).into(), (124, 80).into()),
                Rectangle::new((501, 333).into(), (7, 9).into()),
            ] {
                let crop = backdrop_crop(size, levels, radius, area, &ortho(size, flip));
                assert_eq!(
                    (crop.x % factor, crop.y % factor),
                    (0, 0),
                    "{crop:?} starts on the last level's grid"
                );
                assert!(
                    crop.w % factor == 0 || crop.x + crop.w == size.0,
                    "{crop:?}"
                );
                assert!(
                    crop.x >= 0
                        && crop.y >= 0
                        && crop.x + crop.w <= size.0
                        && crop.y + crop.h <= size.1
                );
                assert!(
                    crop.w * crop.h < size.0 * size.1,
                    "{crop:?} is smaller than the framebuffer"
                );
                // the area in framebuffer rows
                let (ay0, ay1) = if flip {
                    (size.1 - area.loc.y - area.size.h, size.1 - area.loc.y)
                } else {
                    (area.loc.y, area.loc.y + area.size.h)
                };
                let (ax0, ax1) = (area.loc.x, area.loc.x + area.size.w);
                let part: Vec<f32> = (crop.y..crop.y + crop.h)
                    .flat_map(|y| (crop.x..crop.x + crop.w).map(move |x| (x, y)))
                    .map(|(x, y)| img[(y * size.0 + x) as usize])
                    .collect();
                let (cut, cs) = blur(
                    &part,
                    (crop.w as usize, crop.h as usize),
                    levels,
                    &plan.weights,
                );
                // every last-level texel a lookup over the area reads, with its filtered neighbour
                let (tx0, tx1) = (
                    (ax0 / factor - 1).max(0),
                    ((ax1 + factor - 1) / factor + 1).min(ws.0 as i32),
                );
                let (ty0, ty1) = (
                    (ay0 / factor - 1).max(0),
                    ((ay1 + factor - 1) / factor + 1).min(ws.1 as i32),
                );
                for ty in ty0..ty1 {
                    for tx in tx0..tx1 {
                        let (cx, cy) = (tx - crop.x / factor, ty - crop.y / factor);
                        assert!(cx >= 0 && cy >= 0 && (cx as usize) < cs.0 && (cy as usize) < cs.1);
                        let a = whole[ty as usize * ws.0 + tx as usize];
                        let b = cut[cy as usize * cs.0 + cx as usize];
                        assert!(
                            (a - b).abs() < 1e-5,
                            "texel {tx},{ty} of {area:?} (flip {flip}): {a} vs {b}"
                        );
                    }
                }
            }
        }
        // a framebuffer that does not halve exactly is blurred whole
        let odd = (1022, 640);
        let area = Rectangle::new((40, 30).into(), (50, 70).into());
        assert_eq!(
            backdrop_crop(odd, levels, radius, area, &ortho(odd, false)),
            Crop::whole(odd)
        );
        // a backdrop blurred for a larger part serves a region inside it, not one reaching past it
        let big = Crop {
            x: 0,
            y: 0,
            w: 128,
            h: 64,
        };
        assert!(big.contains(&Crop {
            x: 32,
            y: 16,
            w: 64,
            h: 32
        }));
        assert!(!big.contains(&Crop {
            x: 96,
            y: 16,
            w: 64,
            h: 32
        }));
    }

    /// A framebuffer of noise, different for each `seed`.
    fn noise(size: (i32, i32), mut seed: u32) -> Vec<f32> {
        (0..size.0 * size.1)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed % 1000) as f32 / 1000.0
            })
            .collect()
    }

    /// The last pyramid level of the `crop` part of `img` (a `size` framebuffer) after `levels` halvings, as the pass
    /// makes it.
    fn pyramid_of(img: &[f32], size: (i32, i32), crop: Crop, levels: usize) -> Vec<f32> {
        let part: Vec<f32> = (crop.y..crop.y + crop.h)
            .flat_map(|y| (crop.x..crop.x + crop.w).map(move |x| (x, y)))
            .map(|(x, y)| img[(y * size.0 + x) as usize])
            .collect();
        blur(&part, (crop.w as usize, crop.h as usize), levels, &[1.0]).0
    }

    #[test]
    fn a_backdrop_made_again_where_it_changed_or_newly_reaches_is_the_blur_of_the_whole() {
        let size = (1536, 960);
        let plan = plan_blur(size.0 as u32, size.1 as u32, 0.5, 20.0, 2.0);
        let (levels, radius) = (plan.levels.len(), plan.weights.len() - 1);
        let factor = 1i32 << levels;
        let projection = ortho(size, false);
        let at = |x: i32, y: i32, w: i32, h: i32| Rectangle::new((x, y).into(), (w, h).into());
        // a region moved over a still framebuffer, a framebuffer that changed under a still region, and both
        let before = noise(size, 0x9e37_79b9);
        let mut changed = before.clone();
        let window = at(603, 401, 117, 83);
        for y in window.loc.y..window.loc.y + window.size.h {
            for x in window.loc.x..window.loc.x + window.size.w {
                changed[(y * size.0 + x) as usize] = 1.0 - changed[(y * size.0 + x) as usize];
            }
        }
        let bounds = framebuffer_bounds(size, window, &projection).unwrap();
        let region = at(400, 240, 640, 480);
        type Case<'a> = (
            Rectangle<i32, Physical>,
            &'a [f32],
            &'a [(f32, f32, f32, f32)],
        );
        let cases: [Case<'_>; 3] = [
            (
                at(400 + 3 * factor, 240 - 2 * factor, 640, 480),
                &before,
                &[],
            ),
            (region, &changed, &[bounds]),
            (
                at(400 - 2 * factor, 240 + factor, 640, 480),
                &changed,
                &[bounds],
            ),
        ];
        let old = backdrop_crop(size, levels, radius, region, &projection);
        let old_t = Texels::of(old, factor);
        let held = pyramid_of(&before, size, old, levels);
        for (moved, now, changes) in cases {
            let new = backdrop_crop(size, levels, radius, moved, &projection);
            let new_t = Texels::of(new, factor);
            let reblur = plan_reblur(size, levels, old, new, changes)
                .unwrap_or_else(|| panic!("{moved:?} is made again in part"));
            // the pass's steps on the model: the kept texels copied, each part made from its own pixels
            let w = new_t.x1 - new_t.x0;
            let mut made = vec![f32::NAN; new_t.count() as usize];
            if let Some(k) = reblur.kept {
                for ty in k.y0..k.y1 {
                    for tx in k.x0..k.x1 {
                        let from = (ty - old_t.y0) * (old_t.x1 - old_t.x0) + tx - old_t.x0;
                        made[((ty - new_t.y0) * w + tx - new_t.x0) as usize] = held[from as usize];
                    }
                }
            }
            for part in &reblur.parts {
                let cut = pyramid_of(now, size, part.crop(factor), levels);
                for ty in part.y0..part.y1 {
                    for tx in part.x0..part.x1 {
                        let from = (ty - part.y0) * (part.x1 - part.x0) + tx - part.x0;
                        made[((ty - new_t.y0) * w + tx - new_t.x0) as usize] = cut[from as usize];
                    }
                }
            }
            let again: i64 = reblur.parts.iter().map(Texels::count).sum();
            assert!(
                again * 4 < new_t.count(),
                "{moved:?}: {again} of {} texels made again",
                new_t.count()
            );
            // the pyramid is the one a whole capture makes, texel for texel, so the Gaussian over it is too
            let whole = pyramid_of(now, size, new, levels);
            for (i, (m, w)) in made.iter().zip(&whole).enumerate() {
                assert!(
                    m.to_bits() == w.to_bits(),
                    "{moved:?}: texel {i}: {m} vs {w}"
                );
            }
        }
        // a change over most of the backdrop, or a part far from the last, is captured whole
        let everywhere = framebuffer_bounds(size, at(300, 100, 900, 800), &projection).unwrap();
        assert_eq!(plan_reblur(size, levels, old, old, &[everywhere]), None);
        let far = backdrop_crop(size, levels, radius, at(1300, 760, 200, 150), &projection);
        assert_eq!(plan_reblur(size, levels, old, far, &[]), None);
        // a framebuffer that does not halve exactly is captured whole
        assert_eq!(plan_reblur((1022, 640), levels, old, old, &[]), None);
        // a copy without halvings keeps every pixel no change touches
        let copy = Crop {
            x: 101,
            y: 50,
            w: 300,
            h: 200,
        };
        let moved = Crop { x: 99, ..copy };
        let pixels = plan_reblur(size, 0, copy, moved, &[(150.0, 60.0, 160.5, 70.0)]).unwrap();
        assert_eq!(
            pixels.kept,
            Some(Texels {
                x0: 101,
                y0: 50,
                x1: 399,
                y1: 250
            })
        );
        assert_eq!(
            pixels.parts,
            [
                Texels {
                    x0: 99,
                    y0: 50,
                    x1: 101,
                    y1: 250
                },
                Texels {
                    x0: 150,
                    y0: 60,
                    x1: 161,
                    y1: 70
                }
            ]
        );
    }

    #[test]
    fn regions_of_one_recipe_keep_a_backdrop_each_and_a_composite_finds_its_capture() {
        let key = |stage, element| backdrop_key(7, (2560, 1600), 2.0, stage, element);
        // two sidebars of one recipe at different depths: neither replaces the other's blurred backdrop
        assert_ne!(key(MaterialStage::Full, 1), key(MaterialStage::Full, 2));
        // the same region frame after frame finds its own
        assert_eq!(key(MaterialStage::Full, 1), key(MaterialStage::Full, 1));
        // a composite reads what the capture element of its owner made, whatever element draws it
        assert_eq!(
            key(MaterialStage::Capture { owner: 5 }, 1),
            key(MaterialStage::Composite { owner: 5 }, 2)
        );
        assert_ne!(
            key(MaterialStage::Capture { owner: 5 }, 1),
            key(MaterialStage::Composite { owner: 6 }, 1)
        );
    }

    fn recipe(a: Appearance, m: &str) -> Recipe {
        Theme::load()
            .unwrap()
            .recipe(a, m, &MaterialOptions::default())
            .unwrap()
    }

    /// A material element under `frames` builds of the same scene, `masked` or not: its commit after each.
    fn commits(cache: &mut ElementCache, frames: usize, masked: bool) -> Vec<CommitCounter> {
        let recipe = Rc::new(GpuRecipe::new(&recipe(Appearance::Dark, "Sidebar")));
        let area = Rectangle::<f64, Logical>::new((40.0, 30.0).into(), (200.0, 300.0).into());
        (0..frames)
            .map(|_| {
                let e = MaterialElement::new(
                    ElementKey::new("backdrop", 1),
                    MaterialStage::Full,
                    cache,
                    area,
                    recipe.clone(),
                    MaterialGl::detached(),
                    0.0,
                    true,
                    1.0,
                    [0.0; 3],
                    (1280, 800),
                    1.0,
                );
                let mut e = if masked {
                    e.masked(MaterialMask {
                        rect: area,
                        radius_pt: 16.0,
                        continuous: true,
                    })
                } else {
                    e
                };
                e.finalize(42, cache);
                cache.end_frame();
                e.current_commit()
            })
            .collect()
    }

    #[test]
    fn an_unchanged_material_element_keeps_its_commit_so_the_region_is_not_repainted() {
        for masked in [false, true] {
            let mut cache = ElementCache::default();
            let c = commits(&mut cache, 3, masked);
            assert_eq!(c[1], c[0], "masked {masked}");
            assert_eq!(c[2], c[0], "masked {masked}");
        }
    }

    #[test]
    fn a_material_element_repaints_when_what_lies_below_it_changed() {
        let mut cache = ElementCache::default();
        let before = commits(&mut cache, 1, false)[0];
        let recipe = Rc::new(GpuRecipe::new(&recipe(Appearance::Dark, "Sidebar")));
        let area = Rectangle::<f64, Logical>::new((40.0, 30.0).into(), (200.0, 300.0).into());
        let mut e = MaterialElement::new(
            ElementKey::new("backdrop", 1),
            MaterialStage::Full,
            &mut cache,
            area,
            recipe,
            MaterialGl::detached(),
            0.0,
            true,
            1.0,
            [0.0; 3],
            (1280, 800),
            1.0,
        );
        e.finalize(43, &mut cache);
        assert_ne!(e.current_commit(), before);
    }

    /// The source of a shader of `cupertino-glass`'s GLSL ES port, which the server's shaders copy.
    fn glass_shader(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../cupertino-glass/shaders/gles")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The text of `source` from the line holding `start` through the first line after it holding `end`.
    fn span<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let from = source.find(start).unwrap_or_else(|| panic!("no {start:?}"));
        let to = source[from..]
            .find(end)
            .unwrap_or_else(|| panic!("no {end:?} after {start:?}"))
            + from
            + end.len();
        &source[from..to]
    }

    #[test]
    fn the_shaders_shared_with_the_glass_crate_stay_its_copies() {
        // drawn by the server exactly as the glass crate draws them
        for (name, server) in [
            ("fullscreen.vert", FULLSCREEN_VERT),
            ("downsample.frag", DOWNSAMPLE_FRAG),
            ("blur.frag", BLUR_FRAG),
            ("color.frag", COLOR_FRAG),
            ("glass.vert", super::glass::GLASS_VERT),
            ("glass.frag", super::glass::GLASS_FRAG),
        ] {
            assert!(
                server == glass_shader(name),
                "shaders/{name} is no longer a copy of cupertino-glass/shaders/gles/{name}"
            );
        }
        // the material region is placed and sampled through the compositor's frame projection, but its uniform block,
        // mask, blend modes and fill layers are the glass crate's
        let (vert, frag) = (glass_shader("material.vert"), glass_shader("material.frag"));
        let block = |s: &str| span(s, "layout(std140) uniform Region", "} R;").to_owned();
        assert_eq!(block(MATERIAL_VERT), block(&vert), "material.vert block");
        assert_eq!(block(MATERIAL_FRAG), block(&frag), "material.frag block");
        for (start, end) in [
            ("float region_sdf(", "\n}\n"),
            ("float soft_light(", "\n}\n"),
            ("vec3 blend(", "\n}\n"),
            ("int n = int(R.info.x);", "frag_color = "),
        ] {
            assert_eq!(
                span(MATERIAL_FRAG, start, end),
                span(&frag, start, end),
                "material.frag: {start}"
            );
        }
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
    fn released_targets_are_kept_for_reuse_up_to_a_count_and_past_it_up_to_a_size() {
        // a few full-output targets: the most recent are kept, however large
        let whole = vec![(3840, 2160); FREE_KEPT + 3];
        assert_eq!(free_excess(&whole, 8), 3, "past the count, the oldest go");
        assert_eq!(free_excess(&whole[..FREE_KEPT], 8), 0);
        // the targets of glass blurred again as it moves, at sizes a pixel apart: all of them stay, so none is made anew
        let sizes: Vec<(i32, i32)> = (0..48).map(|i| (172 + i % 2, 337 + i / 2 % 2)).collect();
        assert_eq!(free_excess(&sizes, 8), 0);
        // past the count, what does not fit in the budget goes, the oldest first
        let side = ((FREE_BYTES / 4 / (FREE_KEPT + 4)) as f64).sqrt() as i32 + 1;
        let many = vec![(side, side); 2 * FREE_KEPT];
        let excess = free_excess(&many, 4);
        assert!(excess > 0 && excess <= FREE_KEPT, "{excess}");
        let kept = (many.len() - excess) * (side * side) as usize * 4;
        assert!(kept <= FREE_BYTES && kept + (side * side) as usize * 4 > FREE_BYTES);
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
