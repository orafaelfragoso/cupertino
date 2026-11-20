//! Material pass on GLES: backdrop capture → downsample pyramid → separable Gaussian → colour
//! operations → composite with a rounded mask, with the programs `cupertino-glass` generates from WGSL
//! ([`cupertino_glass::shaders`]), their blocks and samplers bound by the names generated beside them.
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
use cupertino_glass::ops::{
    color_ops, fill_ops, Op, PassParams, Placement, RegionParams, PASS_DITHER,
};
use cupertino_glass::shaders::{
    Program, PASS_BLOCK, PASS_SOURCE, PLACEMENT, REGION_BACKDROP, REGION_BLOCK,
};
use cupertino_raster_math::blur::{plan_blur, BlurPlan};
use cupertino_theme::recipe::Recipe;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{ffi, GlesError, GlesFrame, GlesRenderer};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size};

use super::shader_rect::{to_physical_rect, ElementCache, ElementKey, Sig};

#[path = "glass.rs"]
pub mod glass;

type GLuint = ffi::types::GLuint;
type GLint = ffi::types::GLint;
type GLenum = ffi::types::GLenum;

/// Uniform block bindings.
const BIND_PASS: GLuint = 0;
const BIND_REGION: GLuint = 1;
/// The binding of the placement block of the material and glass programs.
const BIND_PLACE: GLuint = 3;

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
    /// The location of the material program's backdrop sampler.
    material_backdrop: GLint,
    src_loc: [GLint; 3],
    /// The blur dithers its result by pixel: its levels are 8 bits a channel and dithering is on.
    blur_dither: bool,
    /// The material and glass composites dither their results by pixel.
    place_dither: bool,
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

/// The side of the target a part's level of `n` texels is made in: `n` rounded up to a multiple of 64, or of a quarter
/// of the power of two below it when that is more. The parts of a drag change size every frame, and making a texture costs the
/// CPU more than a frame's passes; in targets of few sizes each part finds one kept from the frames before.
fn part_room(n: i32) -> i32 {
    let n = n.max(1);
    let below = 1i32 << (31 - n.leading_zeros());
    let step = (below / 4).max(64);
    (n + step - 1) / step * step
}

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
    /// Quads the material and glass composites drew.
    pub composites: u64,
    /// Of those, the glass composite's.
    pub glass_composites: u64,
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
    /// The placement block of the material and glass composites, written for each quad drawn.
    ubo_place: GLuint,
    float_targets: bool,
    free: Vec<Target>,
    cache: Vec<CachedBackdrop>,
    tick: u64,
    pub stats: MaterialStats,
    /// Time the glass passes (the frame-rate benchmark only): with GPU timer queries ([`MaterialGl::gpu_timers`]) the
    /// GPU measures each pass; without them the GPU is waited for around each, which stalls the pipeline. Only
    /// `--frame-stats` sets it, which takes the headless backend: these waits without a deadline are the benchmark's
    /// own and never run in a session.
    pub timing: bool,
    /// The context has GPU timer queries.
    pub gpu_timers: bool,
    /// The timer queries of the passes timed and not yet read.
    timers: PassTimers,
    /// What the glass passes took since the benchmark last cleared it.
    pub glass_time: cupertino_server_backend::frame_stats::GlassTime,
    failure: Option<String>,
    glass: glass::GlassGl,
    /// Per-frame scratch arena for temporary backdrop reblur bounds.
    scratch: bumpalo::Bump,
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

/// Link a generated program and bind each of its blocks, by the names generated beside it, to the binding point given
/// for its WGSL group and binding.
unsafe fn link_generated(
    gl: &ffi::Gles2,
    program: &Program,
    blocks: &[((u32, u32), GLuint)],
) -> Result<GLuint, String> {
    let name = program.source.name;
    let p = link(gl, program.source.vertex_glsl, program.source.fragment_glsl)
        .map_err(|e| format!("{name}: {e}"))?;
    for &((group, binding), point) in blocks {
        let names = program.blocks(group, binding);
        if names.is_empty() {
            gl.DeleteProgram(p);
            return Err(format!("{name}: no block at {group}, {binding}"));
        }
        for block in names {
            if let Err(e) = bind_block(gl, p, &block, point) {
                gl.DeleteProgram(p);
                return Err(format!("{name}: {e}"));
            }
        }
    }
    Ok(p)
}

/// The location of the sampler of a generated program that reads the texture at `at` (group, binding).
unsafe fn sampler_loc(
    gl: &ffi::Gles2,
    p: GLuint,
    program: &Program,
    at: (u32, u32),
) -> Result<GLint, String> {
    let sampler = program
        .sampler(at.0, at.1)
        .ok_or_else(|| format!("{}: no sampler at {}, {}", program.source.name, at.0, at.1))?;
    Ok(uniform_loc(gl, p, &sampler))
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

mod draw;
mod element;
pub use element::{MaterialElement, MaterialMask};
