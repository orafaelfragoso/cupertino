//! Glass pass on GLES, part of the material pipeline ([`MaterialGl`]): one element per glass container.
//!
//! The element covers what of the backdrop the container reads, its shapes grown by the reach of the blur and the
//! refraction (`Container::sample_area`). When it draws with a new backdrop, that area of the bound framebuffer is
//! captured at half scale, blurred into the levels its recipe uses and the blur fill, and kept; the composite then
//! shades the union of the shapes from those textures with the GLSL ES glass shader (`shaders/glass.frag`, a copy of
//! the one in `cupertino-glass`). The textures are kept per container and made again only when the backdrop below the
//! sampled area changed: [`area_generation`] signs the elements below that overlap it, so a change elsewhere on the
//! output, or above the glass, leaves them alone.
//!
//! A capture is clean only when the whole area was drawn afresh below the glass; elsewhere the framebuffer still holds
//! the last frame, glass and all. So textures are made again only when the element's commit advanced (its backdrop or
//! its look changed, and the damage tracker repaints the whole area), and [`GlassCache`] never drops the textures of a
//! container the frame being built shows: past [`MAX_CACHED`] it keeps them all. Textures it does drop (the least
//! recently drawn past the limit, or after [`STALE_AFTER`] draws unused) mark their container lost, and the next build
//! advances that element's commit so it is captured clean again.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use cupertino_glass::blur::plan_blur;
use cupertino_glass::glass::{face_color, Container, GlassUniform};
use cupertino_glass::ops::PassParams;
use cupertino_theme::glass::{
    relative_luminance, GlassMode, GlassParams, BLUR_LEVELS, GLASS_BACKDROP_SCALE,
};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{ffi, GlesError, GlesFrame, GlesRenderer};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size};

use super::{
    bind_block, link, uniform_loc, GLint, GLuint, MaterialGl, SavedState, SharedMaterial, Target,
};
use crate::render::shader_rect::{to_physical_rect, ElementCache, ElementKey, Sig};

pub(super) const GLASS_VERT: &str = include_str!("../../shaders/glass.vert");
pub(super) const GLASS_FRAG: &str = include_str!("../../shaders/glass.frag");

/// Uniform block binding of the glass composite.
const BIND_GLASS: GLuint = 2;
/// Containers whose textures are kept once they no longer show; past it the least recently drawn goes first. Containers
/// the frame being built shows are kept even past it.
const MAX_CACHED: usize = 16;
/// Textures kept per container: one per output it shows on, as each output's capture differs.
const MAX_PER_OWNER: usize = 2;
/// Draws after which the textures of a container that no longer draws are released.
const STALE_AFTER: u64 = 2048;
/// Lost containers remembered until their element is built again; older ones are forgotten.
const MAX_LOST: usize = 4 * MAX_CACHED;
/// Released textures kept for reuse after a glass draw; the rest are deleted.
const FREE_KEPT: usize = 12;

/// A glass container of a frame: its shapes (output-local points), blend width and mode.
#[derive(Clone, Debug)]
pub struct GlassContainer {
    pub container: Container,
    /// Identity of everything that changes how it looks besides the backdrop.
    pub key: u64,
    /// The small shapes whose face the pass measures when it makes the container's blurred backdrop, for the tone of
    /// their text ([`ToneProbe`]).
    pub probes: Vec<ToneProbe>,
    /// For the glass of a window, the part of the window it reads (output-local points): what lies outside it is read
    /// as the nearest pixel inside, so the glass shows the window and not what is around it.
    pub within: Option<Rectangle<f64, Logical>>,
    /// For the glass of a subsurface, the subsurface's rectangle (output-local points): the glass reads and draws only
    /// there, so what changes below the subsurface repaints no more of the output than it.
    pub bound: Option<Rectangle<f64, Logical>>,
}

/// A glass shape whose text follows what lies behind it: the backdrop it was made for and its rectangle in
/// output-local points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneProbe {
    /// The serial of the client's backdrop.
    pub serial: u64,
    pub rect: [f32; 4],
}

/// The face luminance measured under one backdrop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneReading {
    pub serial: u64,
    pub luminance: f32,
}

/// The side, in texels, of the grid the widest blur level is read back at: it is blurred over tens of points, so a
/// coarse grid holds all it says.
const TONE_GRID: i32 = 32;
/// Readbacks waiting for the GPU at once; past it the oldest is given up.
const MAX_TONE_READS: usize = 8;

impl GlassContainer {
    pub fn new(container: Container) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        container.mode.hash(&mut h);
        container.spacing.to_bits().hash(&mut h);
        // a popup's glass lays its own face fill over the recipe of the mode
        for v in container.params.face.fill {
            v.to_bits().hash(&mut h);
        }
        for s in &container.shapes {
            for v in s.rect.iter().chain([&s.radius]) {
                v.to_bits().hash(&mut h);
            }
        }
        Self {
            key: h.finish(),
            container,
            probes: Vec::new(),
            within: None,
            bound: None,
        }
    }

    /// The container measuring the faces of `probes`. They are part of its identity, so a new backdrop over an
    /// unchanged shape is measured too.
    pub fn with_probes(mut self, probes: Vec<ToneProbe>) -> Self {
        if !probes.is_empty() {
            let mut sig = Sig::new().u(self.key);
            for p in &probes {
                sig = sig.u(p.serial);
            }
            self.key = sig.finish();
        }
        self.probes = probes;
        self
    }

    /// The container reading only `within` (output-local points) of what lies below it.
    pub fn within(mut self, within: Rectangle<f64, Logical>) -> Self {
        self.key = Sig::new()
            .u(self.key)
            .f(within.loc.x)
            .f(within.loc.y)
            .f(within.size.w)
            .f(within.size.h)
            .finish();
        self.within = Some(within);
        self
    }

    /// The container reading and drawing only inside `bound` (output-local points), and of that only what `within`
    /// lets it read.
    pub fn bounded(self, bound: Rectangle<f64, Logical>) -> Self {
        let within = match self.within {
            Some(w) => w
                .intersection(bound)
                .unwrap_or_else(|| Rectangle::new(bound.loc, (0.0, 0.0).into())),
            None => bound,
        };
        let mut c = self.within(within);
        c.bound = Some(bound);
        c
    }

    /// Identity of the textures the container needs: its look, the backdrop signature and the scale.
    pub fn state(&self, generation: u64, scale: f32) -> u64 {
        Sig::new()
            .u(generation)
            .u(self.key)
            .u(scale.to_bits() as u64)
            .finish()
    }

    /// What of the backdrop the container reads, in output-local points: no more than its bound.
    pub fn sample_area(&self) -> Option<Rectangle<f64, Logical>> {
        let [x, y, w, h] = self.container.sample_area()?;
        let area = (w > 0.0 && h > 0.0).then(|| {
            Rectangle::new(
                (f64::from(x), f64::from(y)).into(),
                (f64::from(w), f64::from(h)).into(),
            )
        })?;
        match self.bound {
            Some(bound) => area
                .intersection(bound)
                .filter(|a| a.size.w > 0.0 && a.size.h > 0.0),
            None => Some(area),
        }
    }
}

/// One element below a glass element, as its backdrop signature sees it.
#[derive(Clone, Copy, Debug)]
pub struct Below {
    pub id: u64,
    pub commit: u64,
    /// The opacity the element is drawn at: a window can fade without a new commit.
    pub alpha: f32,
    pub geometry: Rectangle<i32, Physical>,
}

/// The signature of the backdrop of an element covering `area`: of the elements below it (`below`, bottom to top)
/// those that overlap `area`, in order. Elements that do not touch it do not count, so the glass keeps its blurred
/// textures while the rest of the output changes.
pub fn area_generation(
    area: Rectangle<i32, Physical>,
    below: impl IntoIterator<Item = Below>,
) -> u64 {
    let mut sig = Sig::new().u(0x67_6c_61_73_73);
    for b in below {
        let g = b.geometry;
        if g.overlaps(area) {
            sig = sig
                .u(b.id)
                .u(b.commit)
                .u(u64::from(b.alpha.to_bits()))
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

/// The blits (source in framebuffer pixels, destination in capture texels, each `[x0, y0, x1, y1]`) that fill the part
/// of a capture of framebuffer rectangle `fb` at `size` texels lying outside framebuffer rectangle `within` with the
/// nearest pixels inside it: a row or column of its edge stretched over each side, a corner pixel over each corner.
/// None when `within` covers the capture, or misses it.
pub fn edge_blits(fb: [i32; 4], size: (i32, i32), within: [i32; 4]) -> Vec<([i32; 4], [i32; 4])> {
    let (x0, y0) = (within[0].max(fb[0]), within[1].max(fb[1]));
    let (x1, y1) = (
        (within[0] + within[2]).min(fb[0] + fb[2]),
        (within[1] + within[3]).min(fb[1] + fb[3]),
    );
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    // a framebuffer position as a capture texel, rounded to the nearest edge
    let tx = |x: i32| ((x - fb[0]) as f32 * size.0 as f32 / fb[2] as f32).round() as i32;
    let ty = |y: i32| ((y - fb[1]) as f32 * size.1 as f32 / fb[3] as f32).round() as i32;
    let (cx0, cy0, cx1, cy1) = (tx(x0), ty(y0), tx(x1), ty(y1));
    let (w, h) = size;
    let mut out = Vec::new();
    let mut add = |src: [i32; 4], dst: [i32; 4]| {
        if dst[2] > dst[0] && dst[3] > dst[1] {
            out.push((src, dst));
        }
    };
    add([x0, y0, x0 + 1, y1], [0, cy0, cx0, cy1]);
    add([x1 - 1, y0, x1, y1], [cx1, cy0, w, cy1]);
    add([x0, y0, x1, y0 + 1], [cx0, 0, cx1, cy0]);
    add([x0, y1 - 1, x1, y1], [cx0, cy1, cx1, h]);
    add([x0, y0, x0 + 1, y0 + 1], [0, 0, cx0, cy0]);
    add([x1 - 1, y0, x1, y0 + 1], [cx1, 0, w, cy0]);
    add([x0, y1 - 1, x0 + 1, y1], [0, cy1, cx0, h]);
    add([x1 - 1, y1 - 1, x1, y1], [cx1, cy1, w, h]);
    out
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
    cache: GlassCache<CachedGlass>,
    /// Readbacks of faces under small shapes, oldest first, until the GPU finished them.
    reads: Vec<ToneRead>,
    /// Faces measured since the server last took them ([`MaterialGl::take_tones`]).
    readings: Vec<ToneReading>,
    /// Glass backdrops that were drawn too large to follow what lies behind them since the server last took them.
    unadapted: Vec<u64>,
}

/// One readback of the widest blur level of a container, at most [`TONE_GRID`] texels on a side, into a pixel buffer
/// the GPU fills while the frame goes on.
struct ToneRead {
    pbo: GLuint,
    fence: ffi::types::GLsync,
    grid: (i32, i32),
    mode: GlassMode,
    /// Each probe's backdrop, its rectangle in texture coordinates of the grid (u0, v0, u1, v1) and its short side.
    probes: Vec<(u64, [f32; 4], f32)>,
}

/// The mean of the texels of an RGBA8 `grid` (`w` × `h`, rows from the bottom) whose centres lie in `uv` (u0, v0, u1,
/// v1 in texture coordinates), or of the texel nearest its centre when none does; encoded colours from 0 to 1.
pub fn grid_mean(grid: &[u8], (w, h): (i32, i32), uv: [f32; 4]) -> [f32; 3] {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let mut sum = [0.0f32; 3];
    let mut n = 0usize;
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        if v < uv[1] || v > uv[3] {
            continue;
        }
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            if u < uv[0] || u > uv[2] {
                continue;
            }
            let i = (y * w + x) * 4;
            for (c, s) in sum.iter_mut().enumerate() {
                *s += f32::from(grid.get(i + c).copied().unwrap_or(0));
            }
            n += 1;
        }
    }
    if n == 0 {
        let x = ((uv[0] + uv[2]) * 0.5 * w as f32).clamp(0.0, w as f32 - 1.0) as usize;
        let y = ((uv[1] + uv[3]) * 0.5 * h as f32).clamp(0.0, h as f32 - 1.0) as usize;
        let i = (y * w + x) * 4;
        return std::array::from_fn(|c| f32::from(grid.get(i + c).copied().unwrap_or(0)) / 255.0);
    }
    sum.map(|v| v / (n as f32 * 255.0))
}

/// The relative luminance of the face glass of `mode` puts over the blurred backdrop `backdrop` (encoded) on a shape
/// whose short side is `short_side` points.
pub fn face_luminance(backdrop: [f32; 3], mode: GlassMode, short_side: f32) -> f32 {
    let params = GlassParams::new(mode, short_side);
    let face = face_color(backdrop, &params, params.face.max_luma);
    relative_luminance(face)
}

/// Where `rect` (output-local points at `scale` pixels per point) lies in the texture coordinates `uv` maps output
/// pixels to (from [`capture_uv`]), as (u0, v0, u1, v1) clamped to the texture.
pub fn probe_uv(rect: [f32; 4], scale: f32, uv: &[[f32; 3]; 2]) -> [f32; 4] {
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for (x, y) in [
        (rect[0], rect[1]),
        (rect[0] + rect[2], rect[1]),
        (rect[0], rect[1] + rect[3]),
        (rect[0] + rect[2], rect[1] + rect[3]),
    ] {
        let (x, y) = (x * scale, y * scale);
        for i in 0..2 {
            let t = uv[i][0] * x + uv[i][1] * y + uv[i][2];
            lo[i] = lo[i].min(t);
            hi[i] = hi[i].max(t);
        }
    }
    [
        lo[0].clamp(0.0, 1.0),
        lo[1].clamp(0.0, 1.0),
        hi[0].clamp(0.0, 1.0),
        hi[1].clamp(0.0, 1.0),
    ]
}

/// The blurred backdrop of one container: its textures and which of them each level and the blur fill reads.
struct CachedGlass {
    targets: Vec<Target>,
    slots: [usize; BLUR_LEVELS + 1],
}

/// One container's kept textures (`T`).
struct Entry<T> {
    owner: u64,
    /// [`GlassContainer::state`] of what they were made for.
    state: u64,
    /// `state` and where in the framebuffer they were captured.
    sig: u64,
    payload: T,
    last_used: u64,
    /// The build that last showed the container.
    shown: u64,
}

/// What the glass pass keeps per container, and which containers lost theirs (see the module documentation).
///
/// A *build* is the lowering of one output's frame: [`GlassCache::check`] runs for every glass element while it is
/// built, before any of them draws ([`GlassCache::begin_draw`]).
pub struct GlassCache<T> {
    entries: Vec<Entry<T>>,
    lost: Vec<u64>,
    build: u64,
    drawn: bool,
    tick: u64,
}

impl<T> Default for GlassCache<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            lost: Vec::new(),
            build: 0,
            drawn: true,
            tick: 0,
        }
    }
}

impl<T> GlassCache<T> {
    /// While a frame is built: keep the textures of `owner` for `state` through it, and whether they are still
    /// whole. `false` (once) when they were dropped while the container still showed: the caller must have the whole
    /// area repainted, so that the next capture reads a freshly drawn backdrop.
    pub fn check(&mut self, owner: u64, state: u64) -> bool {
        if std::mem::replace(&mut self.drawn, false) {
            self.build += 1;
        }
        let build = self.build;
        for e in &mut self.entries {
            if e.owner == owner && e.state == state {
                e.shown = build;
            }
        }
        match self.lost.iter().position(|o| *o == owner) {
            Some(i) => {
                self.lost.remove(i);
                false
            }
            None => true,
        }
    }

    /// Before one container draws: count the draw and drop what containers that stopped drawing kept.
    pub fn begin_draw(&mut self) -> Vec<T> {
        self.drawn = true;
        self.tick += 1;
        let (tick, build) = (self.tick, self.build);
        let mut dropped = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            let e = &self.entries[i];
            if e.shown != build && tick.saturating_sub(e.last_used) > STALE_AFTER {
                let e = self.entries.swap_remove(i);
                self.mark_lost(e.owner);
                dropped.push(e.payload);
            } else {
                i += 1;
            }
        }
        dropped
    }

    /// The textures `owner` keeps for `sig`.
    pub fn get(&mut self, owner: u64, sig: u64) -> Option<&T> {
        let tick = self.tick;
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.owner == owner && e.sig == sig)?;
        e.last_used = tick;
        Some(&e.payload)
    }

    /// Keep `payload` for `owner`, made for `state` and captured as `sig`, and give back what that drops: the oldest
    /// textures of the same container past [`MAX_PER_OWNER`], and past [`MAX_CACHED`] the least recently drawn of
    /// containers this build does not show.
    pub fn insert(&mut self, owner: u64, state: u64, sig: u64, payload: T) -> Vec<T> {
        let mut dropped = Vec::new();
        loop {
            let same = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.owner == owner);
            if same.clone().count() < MAX_PER_OWNER {
                break;
            }
            let oldest = same
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
                .unwrap();
            dropped.push(self.entries.swap_remove(oldest).payload);
        }
        while self.entries.len() >= MAX_CACHED {
            let build = self.build;
            let Some(oldest) = (0..self.entries.len())
                .filter(|i| self.entries[*i].shown != build)
                .min_by_key(|i| self.entries[*i].last_used)
            else {
                break;
            };
            let e = self.entries.swap_remove(oldest);
            self.mark_lost(e.owner);
            dropped.push(e.payload);
        }
        self.entries.push(Entry {
            owner,
            state,
            sig,
            payload,
            last_used: self.tick,
            shown: self.build,
        });
        dropped
    }

    fn mark_lost(&mut self, owner: u64) {
        if self.entries.iter().any(|e| e.owner == owner) || self.lost.contains(&owner) {
            return;
        }
        if self.lost.len() >= MAX_LOST {
            self.lost.remove(0);
        }
        self.lost.push(owner);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Everything kept.
    pub fn drain(&mut self) -> Vec<T> {
        self.lost.clear();
        std::mem::take(&mut self.entries)
            .into_iter()
            .map(|e| e.payload)
            .collect()
    }
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
            ..Self::default()
        })
    }

    /// Poll the readbacks in flight: those the GPU finished give their faces. Whether some are still in flight.
    unsafe fn poll_reads(&mut self, gl: &ffi::Gles2) -> bool {
        let mut i = 0;
        while i < self.reads.len() {
            let r = &self.reads[i];
            match gl.ClientWaitSync(r.fence, 0, 0) {
                ffi::ALREADY_SIGNALED | ffi::CONDITION_SATISFIED => {}
                ffi::TIMEOUT_EXPIRED => {
                    i += 1;
                    continue;
                }
                _ => {
                    let r = self.reads.remove(i);
                    Self::delete_read(gl, r);
                    continue;
                }
            }
            let r = self.reads.remove(i);
            let len = (r.grid.0 * r.grid.1 * 4) as usize;
            gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, r.pbo);
            let ptr = gl.MapBufferRange(ffi::PIXEL_PACK_BUFFER, 0, len as isize, ffi::MAP_READ_BIT);
            if !ptr.is_null() {
                let grid = std::slice::from_raw_parts(ptr as *const u8, len).to_vec();
                gl.UnmapBuffer(ffi::PIXEL_PACK_BUFFER);
                for (serial, uv, short_side) in &r.probes {
                    let backdrop = grid_mean(&grid, r.grid, *uv);
                    self.readings.push(ToneReading {
                        serial: *serial,
                        luminance: face_luminance(backdrop, r.mode, *short_side),
                    });
                }
            }
            gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, 0);
            Self::delete_read(gl, r);
        }
        !self.reads.is_empty()
    }

    unsafe fn delete_read(gl: &ffi::Gles2, r: ToneRead) {
        gl.DeleteSync(r.fence);
        gl.DeleteBuffers(1, &r.pbo);
    }

    pub(super) fn targets(&mut self) -> Vec<Target> {
        self.cache
            .drain()
            .into_iter()
            .flat_map(|c| c.targets)
            .collect()
    }

    pub(super) unsafe fn destroy(&mut self, gl: &ffi::Gles2) {
        for r in std::mem::take(&mut self.reads) {
            Self::delete_read(gl, r);
        }
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

/// The Gaussian deviation of a glass blur per unit of its radius: a recipe's blur radius spans two deviations, so the
/// backdrop keeps the structure that shows through clear glass.
const SIGMA_PER_RADIUS: f32 = 0.5;

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
    /// `glass` reads. With `within`, a framebuffer rectangle, the capture outside it repeats its edge pixels.
    unsafe fn process_glass(
        &mut self,
        gl: &ffi::Gles2,
        src_fbo: GLuint,
        fb: [i32; 4],
        within: Option<[i32; 4]>,
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
        if let Some(within) = within {
            for (src, dst) in edge_blits(fb, size, within) {
                gl.BlitFramebuffer(
                    src[0],
                    src[1],
                    src[2],
                    src[3],
                    dst[0],
                    dst[1],
                    dst[2],
                    dst[3],
                    ffi::COLOR_BUFFER_BIT,
                    ffi::NEAREST,
                );
                self.stats.passes += 1;
            }
        }
        // texels of the capture per point
        let px_per_pt = scale * size.0 as f32 / fb[2] as f32;
        let src = (capture.tex, capture.size);
        let mut targets = vec![capture];
        let mut slots = [usize::MAX; BLUR_LEVELS + 1];
        for (i, (radius, used)) in glass.levels().into_iter().enumerate() {
            if used {
                targets.push(self.blurred(gl, src, radius * SIGMA_PER_RADIUS, px_per_pt)?);
                slots[i] = targets.len() - 1;
            }
        }
        if let Some(radius) = glass.fill_radius() {
            targets.push(self.blurred(gl, src, radius * SIGMA_PER_RADIUS, px_per_pt)?);
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
        // the capture itself is read only through the levels made from it
        if !slots.contains(&0) {
            self.free.push(targets.swap_remove(0));
            let last = targets.len();
            for s in &mut slots {
                if *s == last {
                    *s = 0;
                }
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
        let state = p.glass.state(p.generation, p.scale);
        let sig = Sig::new()
            .u(state)
            .u(fb[0] as u64)
            .u(fb[1] as u64)
            .u(fb[2] as u64)
            .u(fb[3] as u64)
            .finish();
        let released = self.glass.cache.begin_draw();
        self.free
            .extend(released.into_iter().flat_map(|c| c.targets));
        let kept = self
            .glass
            .cache
            .get(p.owner, sig)
            .map(|c| c.slots.map(|s| c.targets[s].tex));
        let textures = match kept {
            Some(textures) => {
                self.stats.backdrop_hits += 1;
                textures
            }
            None => {
                self.stats.backdrop_misses += 1;
                let within = p.glass.within.and_then(|w| {
                    let w = to_physical_rect(w, Scale::from(f64::from(p.scale)));
                    framebuffer_rect(&p.projection, viewport, w)
                });
                let Some((targets, slots)) = self.process_glass(
                    gl,
                    saved.draw_fbo as GLuint,
                    fb,
                    within,
                    &p.glass.container,
                    p.scale,
                ) else {
                    saved.restore(gl);
                    return Err("glass backdrop processing failed (framebuffer incomplete?)".into());
                };
                if !p.glass.probes.is_empty() {
                    self.read_tones(gl, p, &targets, &slots, viewport, fb);
                }
                let textures = slots.map(|s| targets[s].tex);
                let dropped =
                    self.glass
                        .cache
                        .insert(p.owner, state, sig, CachedGlass { targets, slots });
                self.free
                    .extend(dropped.into_iter().flat_map(|c| c.targets));
                textures
            }
        };

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
        // what dropped textures released goes back to the driver beyond a few for reuse
        while self.free.len() > FREE_KEPT {
            let t = self.free.remove(0);
            self.destroy_target(gl, t);
        }
        saved.restore(gl);
        Ok(())
    }

    /// Start reading back the widest blur level `glass` uses at a coarse grid, for the faces of its probes; the
    /// pixels arrive in a pixel buffer without stalling the frame ([`MaterialGl::poll_tones`]).
    unsafe fn read_tones(
        &mut self,
        gl: &ffi::Gles2,
        p: &GlassDraw<'_>,
        targets: &[Target],
        slots: &[usize; BLUR_LEVELS + 1],
        viewport: [i32; 4],
        fb: [i32; 4],
    ) {
        let Some(widest) = p
            .glass
            .container
            .levels()
            .iter()
            .rposition(|(_, used)| *used)
        else {
            return;
        };
        let level = &targets[slots[widest]];
        let grid = (
            level.size.0.clamp(1, TONE_GRID),
            level.size.1.clamp(1, TONE_GRID),
        );
        let Some(small) = self.alloc_target(gl, grid, false) else {
            return;
        };
        gl.Disable(ffi::SCISSOR_TEST);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, level.fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, small.fbo);
        gl.BlitFramebuffer(
            0,
            0,
            level.size.0,
            level.size.1,
            0,
            0,
            grid.0,
            grid.1,
            ffi::COLOR_BUFFER_BIT,
            ffi::LINEAR,
        );
        let mut pbo = 0;
        gl.GenBuffers(1, &mut pbo);
        gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, pbo);
        gl.BufferData(
            ffi::PIXEL_PACK_BUFFER,
            (grid.0 * grid.1 * 4) as isize,
            std::ptr::null(),
            ffi::STREAM_READ,
        );
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, small.fbo);
        gl.ReadPixels(
            0,
            0,
            grid.0,
            grid.1,
            ffi::RGBA,
            ffi::UNSIGNED_BYTE,
            std::ptr::null_mut(),
        );
        gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, 0);
        let fence = gl.FenceSync(ffi::SYNC_GPU_COMMANDS_COMPLETE, 0);
        // the copy into the buffer was queued; the small target can go
        self.destroy_target(gl, small);
        let uv = capture_uv(&p.projection, viewport, fb);
        let probes = p
            .glass
            .probes
            .iter()
            .map(|probe| {
                let side = probe.rect[2].min(probe.rect[3]);
                (probe.serial, probe_uv(probe.rect, p.scale, &uv), side)
            })
            .collect();
        if self.glass.reads.len() >= MAX_TONE_READS {
            let old = self.glass.reads.remove(0);
            GlassGl::delete_read(gl, old);
        }
        self.glass.reads.push(ToneRead {
            pbo,
            fence,
            grid,
            mode: p.glass.container.mode,
            probes,
        });
    }

    /// The faces measured since the last call, and whether readbacks are still in flight, after polling them. Call with
    /// the renderer's context current.
    ///
    /// # Safety
    /// Must be called with the renderer's context current.
    pub unsafe fn poll_tones(&mut self, gl: &ffi::Gles2) -> (Vec<ToneReading>, bool) {
        let pending = self.glass.poll_reads(gl);
        (std::mem::take(&mut self.glass.readings), pending)
    }

    /// Whether readbacks of faces are still in flight; the next frames poll them. A test that reads the tones waits for
    /// none before it does, so every face read back by then was measured.
    pub fn tone_reads_in_flight(&self) -> bool {
        !self.glass.reads.is_empty()
    }

    /// Note glass backdrops drawn too large to follow what lies behind them, while a frame is built.
    pub fn note_unadapted(&mut self, serials: impl IntoIterator<Item = u64>) {
        self.glass.unadapted.extend(serials);
    }

    /// The glass backdrops noted too large since the last call.
    pub fn take_unadapted(&mut self) -> Vec<u64> {
        let mut all = std::mem::take(&mut self.glass.unadapted);
        all.sort_unstable();
        all.dedup();
        all
    }

    /// While a frame is built: whether the textures of glass `owner` for `state` are whole, keeping them through the
    /// frame ([`GlassCache::check`]).
    pub fn glass_check(&mut self, owner: u64, state: u64) -> bool {
        self.glass.cache.check(owner, state)
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
        // the id until `finalize`, which sets the commit: asking with a provisional signature would advance it
        (e.id, e.commit) = cache.peek(key, e.sig(0));
        e
    }

    pub fn area(&self) -> Rectangle<f64, Logical> {
        self.area
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn sig(&self, generation: u64) -> u64 {
        Sig::new()
            .rect(self.area)
            .f(self.opacity as f64)
            .u(self.glass.key)
            .u(generation)
            .finish()
    }

    /// Set the backdrop signature ([`area_generation`]) and refresh the id and commit counter from the cache. Call
    /// once per frame: the commit also advances when the pass dropped the container's textures while it showed, so the
    /// whole area is repainted for the next capture.
    pub fn finalize(&mut self, generation: u64, cache: &mut ElementCache) {
        self.generation = generation;
        let state = self.glass.state(generation, self.scale as f32);
        let whole = self.shared.borrow_mut().glass_check(self.owner(), state);
        let (id, mut commit) = cache.slot(self.key, self.sig(generation));
        if !whole {
            commit = cache.advance(self.key).unwrap_or(commit);
        }
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

    /// Outside the window a capture repeats the window's edge: each side and corner is filled from the nearest row,
    /// column or pixel inside, and nothing is filled where the window covers the capture.
    #[test]
    fn a_capture_outside_its_window_repeats_the_window_edge() {
        // a 100 x 50 capture at half scale of a 200 x 100 rectangle, the window covering its upper-left part
        let (fb, size) = ([10, 20, 200, 100], (100, 50));
        let blits = edge_blits(fb, size, [0, 40, 150, 200]);
        let area = |r: [i32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
        // what lies inside, in texels: x from 0 to 70, y from 10 to 50
        let inside = 70 * 40;
        assert_eq!(
            blits.iter().map(|(_, d)| area(*d)).sum::<i32>(),
            100 * 50 - inside,
            "the blits fill exactly what lies outside: {blits:?}"
        );
        for (src, _) in &blits {
            assert!(
                src[0] >= 10 && src[2] <= 160 && src[1] >= 40 && src[3] <= 120,
                "{src:?}"
            );
            assert!(
                src[2] - src[0] == 1 || src[3] - src[1] == 1,
                "a row, a column or a pixel: {src:?}"
            );
        }
        assert!(
            edge_blits(fb, size, [0, 0, 400, 400]).is_empty(),
            "the window covers it"
        );
        assert!(
            edge_blits(fb, size, [300, 0, 10, 10]).is_empty(),
            "the window misses it"
        );
    }

    /// At a fractional scale, where a capture texel does not fall on whole framebuffer pixels, every texel is filled
    /// once: from the window where its centre lies inside it, and otherwise by one blit from a pixel inside the window.
    #[test]
    fn at_a_fractional_scale_each_texel_outside_the_window_is_filled_once_from_inside_it() {
        let scale = Scale::from(1.5);
        for (offset, size_pt) in [
            ((20.3, 10.7), (150.2, 60.1)),
            ((0.4, 33.3), (77.7, 9.9)),
            ((101.1, 0.0), (99.9, 100.9)),
        ] {
            let window = to_physical_rect(Rectangle::new(offset.into(), size_pt.into()), scale);
            let within = [window.loc.x, window.loc.y, window.size.w, window.size.h];
            // an odd capture rectangle, so the half-scale capture is not a whole number of pixels per texel
            let fb = [3, 7, 301, 151];
            let size = (
                (fb[2] as f32 * GLASS_BACKDROP_SCALE).ceil() as i32,
                (fb[3] as f32 * GLASS_BACKDROP_SCALE).ceil() as i32,
            );
            let blits = edge_blits(fb, size, within);
            let inside = |x: f32, y: f32| {
                x >= within[0] as f32
                    && x <= (within[0] + within[2]) as f32
                    && y >= within[1] as f32
                    && y <= (within[1] + within[3]) as f32
            };
            for (src, _) in &blits {
                assert!(
                    inside(src[0] as f32, src[1] as f32) && inside(src[2] as f32, src[3] as f32),
                    "{offset:?}: read from inside the window: {src:?}"
                );
            }
            for ty in 0..size.1 {
                for tx in 0..size.0 {
                    let filled = blits
                        .iter()
                        .filter(|(_, d)| d[0] <= tx && tx < d[2] && d[1] <= ty && ty < d[3])
                        .count();
                    // where the capture keeps its own texel, that texel's centre lies in the window
                    let centre = (
                        fb[0] as f32 + (tx as f32 + 0.5) * fb[2] as f32 / size.0 as f32,
                        fb[1] as f32 + (ty as f32 + 0.5) * fb[3] as f32 / size.1 as f32,
                    );
                    assert!(
                        filled == 1 || (filled == 0 && inside(centre.0, centre.1)),
                        "{offset:?}: texel ({tx}, {ty}) at {centre:?} filled {filled} times"
                    );
                }
            }
        }
    }

    /// Glass reading only part of what lies below it is another backdrop: the part is part of its identity, so a window
    /// that moves or resizes captures again, and the same part keeps the same identity.
    #[test]
    fn the_part_a_container_reads_is_part_of_its_identity() {
        let container = || {
            GlassContainer::new(Container::new(
                vec![GlassShape::new([40.0, 30.0, 200.0, 60.0], 16.0)],
                0.0,
                GlassMode::default(),
            ))
        };
        let window = Rectangle::<f64, Logical>::new((10.0, 20.0).into(), (400.0, 300.0).into());
        let moved = Rectangle::new((11.0, 20.0).into(), (400.0, 300.0).into());
        let (all, part) = (container(), container().within(window));
        assert_eq!(all.within, None);
        assert_eq!(part.within, Some(window));
        assert_ne!(all.key, part.key);
        assert_eq!(part.key, container().within(window).key);
        assert_ne!(part.key, container().within(moved).key);
        assert_ne!(part.state(1, 1.0), container().within(moved).state(1, 1.0));
    }

    /// The glass of a subsurface reads and draws only inside the subsurface: its sampled area, which the damage tracker
    /// repaints when what lies below changes, stops at the subsurface's edges, and what it reads is the window's part
    /// of them.
    #[test]
    fn bounded_glass_samples_no_more_than_its_bound_and_reads_the_window_s_part_of_it() {
        let container = || {
            GlassContainer::new(Container::new(
                vec![GlassShape::new([180.0, 8.0, 120.0, 36.0], 18.0)],
                0.0,
                GlassMode::default(),
            ))
        };
        let band = Rectangle::<f64, Logical>::new((0.0, 0.0).into(), (480.0, 52.0).into());
        let free = container().sample_area().unwrap();
        assert!(
            free.loc.y < 0.0 && free.loc.y + free.size.h > 52.0,
            "unbounded, the blur reaches past the row: {free:?}"
        );
        let bounded = container().bounded(band);
        assert_eq!(bounded.sample_area(), free.intersection(band));
        assert_eq!(bounded.within, Some(band));
        assert_ne!(bounded.key, container().key);
        let window = Rectangle::<f64, Logical>::new((2.0, 3.0).into(), (400.0, 300.0).into());
        let both = container().within(window).bounded(band);
        assert_eq!(both.within, window.intersection(band));
        assert_eq!(both.sample_area(), free.intersection(band));
        // a bound the shapes do not reach samples nothing
        let away = Rectangle::<f64, Logical>::new((0.0, 400.0).into(), (480.0, 52.0).into());
        assert_eq!(container().bounded(away).sample_area(), None);
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
        let below = |id, commit, geometry| Below {
            id,
            commit,
            alpha: 1.0,
            geometry,
        };
        let inside = |commit| below(1, commit, rect(150, 120, 50, 50));
        let outside = |commit| below(2, commit, rect(400, 400, 50, 50));
        let base = area_generation(area, [inside(0), outside(0)]);
        // a change outside the area keeps the blurred levels
        assert_eq!(base, area_generation(area, [inside(0), outside(5)]));
        assert_eq!(base, area_generation(area, [inside(0)]));
        // a change of content, a move or a new element under the area makes them again
        assert_ne!(base, area_generation(area, [inside(1), outside(0)]));
        assert_ne!(
            base,
            area_generation(area, [below(1, 0, rect(160, 120, 50, 50)), outside(0)])
        );
        assert_ne!(
            base,
            area_generation(
                area,
                [inside(0), below(3, 0, rect(290, 190, 20, 20)), outside(0)]
            )
        );
        // an element that moves into the area counts from then on
        assert_ne!(
            base,
            area_generation(area, [inside(0), below(2, 0, rect(280, 150, 50, 50))])
        );
        // a window under the glass fading without a new commit
        let fading = Below {
            alpha: 0.5,
            ..inside(0)
        };
        assert_ne!(base, area_generation(area, [fading, outside(0)]));
    }

    /// One frame of `shown` containers (owner, state, sig): every build checks them, then each draws, making its
    /// textures (a payload naming it) when it has none. What each build's checks say, and what was dropped.
    fn frame(cache: &mut GlassCache<u64>, shown: &[(u64, u64, u64)]) -> (Vec<bool>, Vec<u64>) {
        let whole = shown
            .iter()
            .map(|(o, st, _)| cache.check(*o, *st))
            .collect();
        let mut dropped = Vec::new();
        for (owner, state, sig) in shown {
            dropped.extend(cache.begin_draw());
            if cache.get(*owner, *sig).is_none() {
                dropped.extend(cache.insert(*owner, *state, *sig, *owner));
            }
        }
        (whole, dropped)
    }

    #[test]
    fn the_cache_keeps_what_the_frame_shows_even_past_its_limit() {
        let mut cache = GlassCache::default();
        let many: Vec<(u64, u64, u64)> =
            (0..MAX_CACHED as u64 + 4).map(|i| (i, 7, 70 + i)).collect();
        let (whole, dropped) = frame(&mut cache, &many);
        assert!(whole.iter().all(|w| *w) && dropped.is_empty());
        assert_eq!(
            cache.len(),
            MAX_CACHED + 4,
            "nothing the frame shows is dropped"
        );
        // an unchanged second frame draws every container from what it kept
        let (whole, dropped) = frame(&mut cache, &many);
        assert!(whole.iter().all(|w| *w) && dropped.is_empty());
        // one new container once the others stopped showing: the least recently drawn ones make room
        let (_, dropped) = frame(&mut cache, &[(99, 7, 990)]);
        assert_eq!(dropped, (0..5).collect::<Vec<_>>());
        assert_eq!(cache.len(), MAX_CACHED);
        // a dropped container that shows again is reported once, so its element repaints the whole area
        let (whole, _) = frame(&mut cache, &[(0, 7, 70), (6, 7, 76)]);
        assert_eq!(whole, vec![false, true]);
        let (whole, _) = frame(&mut cache, &[(0, 7, 70)]);
        assert_eq!(whole, vec![true]);
    }

    #[test]
    fn a_container_keeps_one_capture_per_output_and_a_new_look_replaces_the_oldest() {
        let mut cache = GlassCache::default();
        // the same container on two outputs: two captures, each drawn from what it kept
        frame(&mut cache, &[(1, 10, 100)]);
        frame(&mut cache, &[(1, 11, 110)]);
        let (whole, dropped) = frame(&mut cache, &[(1, 10, 100)]);
        assert!(whole[0] && dropped.is_empty());
        assert_eq!(cache.len(), 2);
        // its backdrop changes: the new capture replaces the least recently drawn, which is not a loss
        let (_, dropped) = frame(&mut cache, &[(1, 12, 120)]);
        assert_eq!(dropped, vec![1]);
        assert_eq!(cache.len(), 2);
        assert_eq!(frame(&mut cache, &[(1, 12, 120)]).0, vec![true]);
    }

    #[test]
    fn textures_unused_for_long_are_released_and_their_container_repaints_when_it_shows_again() {
        let mut cache = GlassCache::default();
        frame(&mut cache, &[(1, 10, 100), (2, 20, 200)]);
        // container 2 keeps drawing (a video under it) while container 1 does not
        let mut dropped = Vec::new();
        for i in 0..=STALE_AFTER {
            dropped.extend(frame(&mut cache, &[(2, 21 + i, 210 + i)]).1);
        }
        assert!(dropped.contains(&1), "container 1's textures were released");
        assert_eq!(frame(&mut cache, &[(1, 10, 100)]).0, vec![false]);
    }

    /// Glass under `frames` builds of the same scene: its commit after each.
    fn commits(
        shared: &SharedMaterial,
        cache: &mut ElementCache,
        frames: usize,
    ) -> Vec<CommitCounter> {
        let container = Rc::new(GlassContainer::new(Container::new(
            vec![GlassShape::new([40.0, 30.0, 200.0, 60.0], 16.0)],
            0.0,
            GlassMode::default(),
        )));
        (0..frames)
            .map(|_| {
                let mut e = GlassElement::new(
                    ElementKey::new("backdrop", 1),
                    cache,
                    container.sample_area().unwrap(),
                    container.clone(),
                    shared.clone(),
                    1.0,
                    1.0,
                );
                e.finalize(42, cache);
                cache.end_frame();
                e.current_commit()
            })
            .collect()
    }

    #[test]
    fn an_unchanged_glass_element_keeps_its_commit_so_the_area_is_not_repainted() {
        let shared = MaterialGl::detached();
        let mut cache = ElementCache::default();
        let c = commits(&shared, &mut cache, 3);
        assert_eq!(c[1], c[0]);
        assert_eq!(c[2], c[0]);
    }

    #[test]
    fn a_glass_element_whose_textures_were_dropped_repaints_once() {
        let shared = MaterialGl::detached();
        let mut cache = ElementCache::default();
        let before = commits(&shared, &mut cache, 1)[0];
        {
            let owner = {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                ElementKey::new("backdrop", 1).hash(&mut h);
                h.finish()
            };
            let mut m = shared.borrow_mut();
            m.glass.cache.mark_lost(owner);
        }
        let after = commits(&shared, &mut cache, 2);
        assert_ne!(
            after[0], before,
            "the commit advances, so the whole area is repainted"
        );
        assert_eq!(after[1], after[0], "once");
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

    #[test]
    fn a_probe_reads_the_texels_under_its_shape_in_the_capture() {
        // an output 200 × 100 pixels, rows from the bottom, captured at framebuffer rect (20, 10, 40, 30)
        let proj = projection(200.0, 100.0);
        let uv = capture_uv(&proj, [0, 0, 200, 100], [20, 10, 40, 30]);
        // a shape of 10 × 10 points at scale 1 in the capture's left half
        let r = probe_uv([20.0, 10.0, 20.0, 30.0], 1.0, &uv);
        assert!(r[0].abs() < 1e-5 && (r[2] - 0.5).abs() < 1e-5, "{r:?}");
        // a grid 4 × 2 whose left half is black and right half white
        let mut grid = vec![0u8; 4 * 2 * 4];
        for y in 0..2 {
            for x in 2..4 {
                grid[(y * 4 + x) * 4..(y * 4 + x) * 4 + 3].copy_from_slice(&[255; 3]);
            }
        }
        assert_eq!(grid_mean(&grid, (4, 2), r), [0.0; 3]);
        assert_eq!(grid_mean(&grid, (4, 2), [0.5, 0.0, 1.0, 1.0]), [1.0; 3]);
        assert_eq!(grid_mean(&grid, (4, 2), [0.0, 0.0, 1.0, 1.0]), [0.5; 3]);
        // a shape smaller than a texel takes the texel under its centre
        assert_eq!(grid_mean(&grid, (4, 2), [0.8, 0.4, 0.81, 0.41]), [1.0; 3]);
    }

    #[test]
    fn dark_glass_over_white_has_a_light_face_and_over_black_a_dark_one() {
        use cupertino_theme::glass::{face_tone, FaceTone, GlassStyle};
        let mode = |dark| GlassMode {
            dark,
            style: GlassStyle::Regular,
            ..GlassMode::default()
        };
        let dark = mode(true);
        assert_eq!(
            face_tone(face_luminance([1.0; 3], dark, 28.0), None),
            FaceTone::Light
        );
        assert_eq!(
            face_tone(face_luminance([0.0; 3], dark, 28.0), None),
            FaceTone::Dark
        );
        // the milky face of light glass stays light over black too, though darker than over white
        let light = mode(false);
        let (white, black) = (
            face_luminance([1.0; 3], light, 28.0),
            face_luminance([0.0; 3], light, 28.0),
        );
        assert!(black < white);
        assert_eq!(face_tone(black, None), FaceTone::Light);
    }

    #[test]
    fn small_backdrops_are_probed_and_the_probes_are_part_of_the_identity() {
        let shape = |w| GlassShape::new([0.0, 0.0, w, 30.0], 15.0);
        let container =
            GlassContainer::new(Container::new(vec![shape(40.0)], 0.0, GlassMode::default()));
        let probe = |serial| ToneProbe {
            serial,
            rect: [0.0, 0.0, 40.0, 30.0],
        };
        let a = container.clone().with_probes(vec![probe(1)]);
        let b = container.clone().with_probes(vec![probe(2)]);
        assert_ne!(a.key, container.key);
        assert_ne!(
            a.key, b.key,
            "a new backdrop over the same shape is measured too"
        );
        assert_eq!(container.clone().with_probes(Vec::new()).key, container.key);
    }
}
