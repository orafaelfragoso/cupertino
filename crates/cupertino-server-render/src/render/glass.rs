//! Glass pass on GLES, part of the material pipeline ([`MaterialGl`]): one element per glass container.
//!
//! The element covers what of the backdrop the container reads, its shapes grown by the reach of the blur and the
//! refraction (`Container::sample_area`). When it draws with a new backdrop, that area of the bound framebuffer is
//! captured at half scale, blurred into the levels its recipe uses, the blur fill and the edge bleed's blur (a copy of its own only when none of those has its deviation), and kept; the composite then
//! shades the union of the shapes from those textures with the glass program `cupertino-glass` generates
//! ([`cupertino_glass::shaders::GLASS`]). The textures are kept per container and made again only when the backdrop below the
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

use cupertino_geometry::CornerStyle;
use cupertino_glass::glass::{
    face_color, BleedSource, Container, Cut, GlassUniform, BLEED_LEVEL, FILL_LEVEL, SHARP_LEVEL,
};
use cupertino_glass::ops::{PassParams, PASS_KEEP_ALPHA};
use cupertino_glass::shaders::{GLASS_BLOCK, GLASS_TEXTURES, PLACEMENT};
use cupertino_raster_math::blur::plan_blur;
use cupertino_theme::glass::{relative_luminance, GlassMode, GlassParams, GLASS_BACKDROP_SCALE};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{ffi, GlesError, GlesFrame, GlesRenderer};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size};

use super::{
    link_generated, sampler_loc, GLuint, MaterialGl, MaterialMask, SavedState, SharedMaterial,
    Target, BIND_PLACE,
};
use crate::render::shader_rect::{to_physical_rect, ElementCache, ElementKey, Sig};

/// Uniform block binding of the glass composite.
pub(super) const BIND_GLASS: GLuint = 2;
/// Containers whose textures are kept once they no longer show; past it the least recently drawn goes first. Containers
/// the frame being built shows are kept even past it.
const MAX_CACHED: usize = 16;
/// Textures kept per container: one per output it shows on, as each output's capture differs.
const MAX_PER_OWNER: usize = 2;
/// Draws after which the textures of a container that no longer draws are released.
const STALE_AFTER: u64 = 2048;
/// Lost containers remembered until their element is built again; older ones are forgotten.
const MAX_LOST: usize = 4 * MAX_CACHED;

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
        // a layer's glass: its lens, its blur ramp, where its rim is lit from and its lifted shadow
        for v in [
            container.lens,
            container.blur_ramp[0],
            container.blur_ramp[1],
            container.params.highlight.angle,
            container.params.shadow.opacity,
        ] {
            v.to_bits().hash(&mut h);
        }
        for s in &container.shapes {
            for v in s.rect.iter().chain([&s.radius]) {
                v.to_bits().hash(&mut h);
            }
        }
        // each shape's look and values, its own recipe's where it has one (a segment in other glass than its group, a
        // tint)
        for look in container.looks() {
            bytemuck::bytes_of(&look).hash(&mut h);
        }
        for v in &container.values {
            bytemuck::bytes_of(v).hash(&mut h);
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

    /// The container cut to `mask` (output-local points), the rounded outline of the window it is in, as well as to its
    /// shapes: a square shape that reaches the window's corners follows them, as the window's content does, and a shape
    /// rounded inside the outline is left as it is.
    pub fn masked(mut self, mask: MaterialMask) -> Self {
        let style = if mask.continuous {
            CornerStyle::continuous()
        } else {
            CornerStyle::Circular
        };
        let cut = Cut {
            rect: [
                mask.rect.loc.x as f32,
                mask.rect.loc.y as f32,
                mask.rect.size.w as f32,
                mask.rect.size.h as f32,
            ],
            extent: (mask.radius_pt * style.expansion()) as f32,
            exponent: style.exponent() as f32,
        };
        let mut sig = Sig::new().u(self.key);
        for v in cut.rect.iter().chain([&cut.extent, &cut.exponent]) {
            sig = sig.u(u64::from(v.to_bits()));
        }
        self.key = sig.finish();
        self.container = self.container.cut_to(cut);
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Below {
    pub id: u64,
    pub commit: u64,
    /// The opacity the element is drawn at: a window can fade without a new commit.
    pub alpha: f32,
    pub geometry: Rectangle<i32, Physical>,
    /// A rectangle inside the geometry where the element draws nothing (a shadow under its window), if any.
    pub hole: Option<Rectangle<i32, Physical>>,
}

impl Below {
    /// The parts of the element's place it may draw on: its geometry but the hole it shares with `other`.
    fn drawn(&self, other: &Below) -> Vec<Rectangle<i32, Physical>> {
        let hole = self
            .hole
            .zip(other.hole)
            .and_then(|(a, b)| a.intersection(b));
        match hole {
            Some(hole) => self.geometry.subtract_rect(hole),
            None => vec![self.geometry],
        }
    }
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

/// What the backdrop signature of a glass element was made from in the last frame: the signature and the elements below
/// that overlapped the area it samples, bottom to top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackdropRecord {
    pub generation: u64,
    pub below: Vec<Below>,
}

/// The backdrop signature of a glass element sampling `area`, given the elements below that overlap it (`overlapping`,
/// bottom to top) and what the signature was made from last frame. When the same elements lie there, in the same order,
/// place and opacity, and those that committed since report no damage inside `area` (`damage(i, since)`: the damage
/// of `overlapping[i]` since commit `since`, in output pixels), the backdrop the glass reads is what it was: the last
/// signature holds and the blurred levels are kept. Otherwise it is [`area_generation`]'s.
pub fn backdrop_generation(
    area: Rectangle<i32, Physical>,
    overlapping: &[Below],
    last: Option<&BackdropRecord>,
    damage: impl FnMut(usize, u64) -> Vec<Rectangle<i32, Physical>>,
) -> u64 {
    backdrop_change(area, overlapping, last, damage).generation
}

/// A backdrop signature and what changed under the area since the last one.
#[derive(Clone, Debug, PartialEq)]
pub struct BackdropChange {
    pub generation: u64,
    /// The last frame's signature and the rectangles (output pixels, within the area) whose pixels may differ from
    /// what lay there then; none when that is not known (no last frame, or the elements changed order).
    pub since: Option<(u64, Vec<Rectangle<i32, Physical>>)>,
}

/// [`backdrop_generation`] together with where the backdrop changed since the last frame: the places an element left
/// or took, the old and new places of one that moved or changed opacity, and the damage of one that committed.
pub fn backdrop_change(
    area: Rectangle<i32, Physical>,
    overlapping: &[Below],
    last: Option<&BackdropRecord>,
    mut damage: impl FnMut(usize, u64) -> Vec<Rectangle<i32, Physical>>,
) -> BackdropChange {
    let Some(last) = last else {
        return BackdropChange {
            generation: area_generation(area, overlapping.iter().copied()),
            since: None,
        };
    };
    let unique = |list: &[Below]| {
        let mut ids: Vec<u64> = list.iter().map(|b| b.id).collect();
        ids.sort_unstable();
        ids.windows(2).all(|w| w[0] != w[1])
    };
    // the elements in both frames, in the order of each: the same order, or the change is not known
    let kept_was: Vec<u64> = last
        .below
        .iter()
        .filter(|w| overlapping.iter().any(|n| n.id == w.id))
        .map(|w| w.id)
        .collect();
    let kept_now: Vec<u64> = overlapping
        .iter()
        .filter(|n| last.below.iter().any(|w| w.id == n.id))
        .map(|n| n.id)
        .collect();
    let ordered = unique(&last.below) && unique(overlapping) && kept_was == kept_now;
    let mut changed = Vec::new();
    let mut held = last.below.len() == overlapping.len();
    for was in &last.below {
        if !overlapping.iter().any(|n| n.id == was.id) {
            changed.extend(was.drawn(was));
        }
    }
    for (i, now) in overlapping.iter().enumerate() {
        match last.below.iter().find(|w| w.id == now.id) {
            None => {
                held = false;
                changed.extend(now.drawn(now));
            }
            Some(was)
                if was.alpha.to_bits() != now.alpha.to_bits() || was.geometry != now.geometry =>
            {
                // where it was and where it is, but where it draws nothing in both
                held = false;
                changed.extend(was.drawn(now));
                changed.extend(now.drawn(was));
            }
            Some(was) if was.commit != now.commit => {
                let inside: Vec<_> = damage(i, was.commit)
                    .into_iter()
                    .filter_map(|d| d.intersection(area))
                    .collect();
                held &= inside.is_empty();
                changed.extend(inside);
            }
            Some(_) => {}
        }
    }
    held &= last
        .below
        .iter()
        .zip(overlapping)
        .all(|(w, n)| w.id == n.id);
    let generation = if held {
        last.generation
    } else {
        area_generation(area, overlapping.iter().copied())
    };
    let since = ordered.then(|| {
        let inside = changed
            .into_iter()
            .filter_map(|c| c.intersection(area))
            .collect();
        (last.generation, inside)
    });
    BackdropChange { generation, since }
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

/// The size in texels of the capture of framebuffer rectangle `fb` at `scale` of the output's pixels (at most the
/// output's own), at least one texel each way.
pub fn capture_size(fb: [i32; 4], scale: f32) -> (i32, i32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale.min(1.0)
    } else {
        GLASS_BACKDROP_SCALE
    };
    (
        ((fb[2] as f32 * scale).ceil() as i32).max(1),
        ((fb[3] as f32 * scale).ceil() as i32).max(1),
    )
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
    ubo: GLuint,
    cache: GlassCache<CachedGlass>,
    /// Readbacks of faces under small shapes, oldest first, until the GPU finished them.
    reads: Vec<ToneRead>,
    /// Faces measured since the server last took them ([`MaterialGl::take_tones`]).
    readings: Vec<ToneReading>,
    /// Glass backdrops that were drawn too large to follow what lies behind them since the server last took them.
    unadapted: Vec<u64>,
    /// The small targets readbacks shrink a blur level into, kept for the next readbacks.
    grids: Vec<Target>,
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

/// The textures the glass program reads: the blurred copies, the blur fill, the edge bleed's blur and the capture.
const GLASS_SLOTS: usize = GLASS_TEXTURES.len();

/// The blurred backdrop of one container: its textures and which of them each level and the blur fill reads.
struct CachedGlass {
    targets: Vec<Target>,
    slots: [usize; GLASS_SLOTS],
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

    /// Before `owner` makes new textures: give back those of it that [`GlassCache::insert`] would drop for them, the
    /// oldest past [`MAX_PER_OWNER`] - 1, so the new ones are made in them rather than in textures created anew.
    pub fn make_room(&mut self, owner: u64) -> Vec<T> {
        let mut dropped = Vec::new();
        self.drop_oldest_of(owner, MAX_PER_OWNER - 1, &mut dropped);
        dropped
    }

    /// Drop the least recently used textures of `owner` into `dropped` until it keeps no more than `limit`.
    fn drop_oldest_of(&mut self, owner: u64, limit: usize, dropped: &mut Vec<T>) {
        loop {
            let same = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.owner == owner);
            if same.clone().count() <= limit {
                break;
            }
            let oldest = same
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
                .unwrap();
            dropped.push(self.entries.swap_remove(oldest).payload);
        }
    }

    /// Keep `payload` for `owner`, made for `state` and captured as `sig`, and give back what that drops: the oldest
    /// textures of the same container past [`MAX_PER_OWNER`], and past [`MAX_CACHED`] the least recently drawn of
    /// containers this build does not show.
    pub fn insert(&mut self, owner: u64, state: u64, sig: u64, payload: T) -> Vec<T> {
        let mut dropped = Vec::new();
        self.drop_oldest_of(owner, MAX_PER_OWNER - 1, &mut dropped);
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
        let source = &cupertino_glass::shaders::GLASS;
        let program = link_generated(
            gl,
            source,
            &[(GLASS_BLOCK, BIND_GLASS), (PLACEMENT, BIND_PLACE)],
        )?;
        // the four blurred copies on the first four units, then the blur fill, the edge bleed's blur and the capture
        let mut units = Vec::new();
        for at in GLASS_TEXTURES {
            match sampler_loc(gl, program, source, at) {
                Ok(loc) => units.push(loc),
                Err(e) => {
                    gl.DeleteProgram(program);
                    return Err(e);
                }
            }
        }
        gl.UseProgram(program);
        for (unit, loc) in units.into_iter().enumerate() {
            gl.Uniform1i(loc, unit as i32);
        }
        gl.UseProgram(0);
        let mut ubo = 0;
        gl.GenBuffers(1, &mut ubo);
        Ok(Self {
            program,
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
            .chain(std::mem::take(&mut self.grids))
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

#[path = "glass/draw.rs"]
mod draw;
pub use draw::GlassElement;
