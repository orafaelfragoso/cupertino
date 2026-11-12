//! Pixel-shader rectangle element with fractional logical geometry, plus the cache that gives
//! per-frame rebuilt elements stable ids and commit counters (so damage tracking only reports
//! changes when the parameters actually changed).

use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};

use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, Uniform,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size};

/// Identity of a cached element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElementKey {
    pub kind: &'static str,
    pub index: u64,
}

impl ElementKey {
    pub const fn new(kind: &'static str, index: u64) -> Self {
        Self { kind, index }
    }
}

/// Hash builder for element signatures (f64 values hashed by bit pattern).
#[derive(Default)]
pub struct Sig(DefaultHasher);

impl Sig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn f(mut self, v: f64) -> Self {
        v.to_bits().hash(&mut self.0);
        self
    }
    pub fn u(mut self, v: u64) -> Self {
        v.hash(&mut self.0);
        self
    }
    pub fn rect(self, r: Rectangle<f64, Logical>) -> Self {
        self.f(r.loc.x).f(r.loc.y).f(r.size.w).f(r.size.h)
    }
    pub fn finish(self) -> u64 {
        self.0.finish()
    }
}

#[derive(Debug)]
struct Slot {
    id: Id,
    commit: CommitCounter,
    sig: u64,
    used: bool,
}

/// Stable ids and commit counters for elements rebuilt every frame.
#[derive(Debug, Default)]
pub struct ElementCache {
    slots: HashMap<ElementKey, Slot>,
    /// What each glass element's backdrop signature was made from, and whether it was used since the last frame.
    backdrops: HashMap<ElementKey, (crate::render::material::glass::BackdropRecord, bool)>,
}

impl ElementCache {
    /// Id and commit for `key`; the commit counter advances when `sig` differs from the last call.
    pub fn slot(&mut self, key: ElementKey, sig: u64) -> (Id, CommitCounter) {
        let s = self.slots.entry(key).or_insert_with(|| Slot {
            id: Id::new(),
            commit: CommitCounter::default(),
            sig,
            used: true,
        });
        if s.sig != sig {
            s.sig = sig;
            s.commit.increment();
        }
        s.used = true;
        (s.id.clone(), s.commit)
    }

    /// Id and commit for `key` as they are, without comparing a signature; `sig` is stored only for a new key.
    pub fn peek(&mut self, key: ElementKey, sig: u64) -> (Id, CommitCounter) {
        let s = self.slots.entry(key).or_insert_with(|| Slot {
            id: Id::new(),
            commit: CommitCounter::default(),
            sig,
            used: true,
        });
        s.used = true;
        (s.id.clone(), s.commit)
    }

    /// Advance the commit of `key` though its signature is unchanged; its new commit, if it is known.
    pub fn advance(&mut self, key: ElementKey) -> Option<CommitCounter> {
        let s = self.slots.get_mut(&key)?;
        s.commit.increment();
        Some(s.commit)
    }

    /// What the backdrop signature of glass element `key` was made from last frame.
    pub fn backdrop(
        &self,
        key: ElementKey,
    ) -> Option<&crate::render::material::glass::BackdropRecord> {
        self.backdrops.get(&key).map(|(r, _)| r)
    }

    /// Keep what the backdrop signature of glass element `key` was made from this frame.
    pub fn set_backdrop(
        &mut self,
        key: ElementKey,
        record: crate::render::material::glass::BackdropRecord,
    ) {
        self.backdrops.insert(key, (record, true));
    }

    /// Forget every key that was not requested since the previous call.
    pub fn end_frame(&mut self) {
        self.slots
            .retain(|_, s| std::mem::replace(&mut s.used, false));
        self.backdrops
            .retain(|_, (_, used)| std::mem::replace(used, false));
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// Physical rectangle of a logical one, rounding both edges (no gaps between neighbours).
pub fn to_physical_rect(r: Rectangle<f64, Logical>, scale: Scale<f64>) -> Rectangle<i32, Physical> {
    let x0 = (r.loc.x * scale.x).round();
    let y0 = (r.loc.y * scale.y).round();
    let x1 = ((r.loc.x + r.size.w) * scale.x).round();
    let y1 = ((r.loc.y + r.size.h) * scale.y).round();
    Rectangle::new(
        (x0 as i32, y0 as i32).into(),
        ((x1 - x0) as i32, (y1 - y0) as i32).into(),
    )
}

/// A rectangle drawn by a pixel shader. The shader sees `v_coords` in `[0, 1]` across the
/// element and the uniform `area_pt` (element size in points); the remaining uniforms are
/// supplied by the caller in points relative to the element origin.
#[derive(Debug, Clone)]
pub struct ShaderRect {
    program: GlesPixelProgram,
    id: Id,
    commit: CommitCounter,
    area: Rectangle<f64, Logical>,
    alpha: f32,
    uniforms: Vec<Uniform<'static>>,
    opaque: bool,
}

impl ShaderRect {
    pub fn new(
        program: GlesPixelProgram,
        id: Id,
        commit: CommitCounter,
        area: Rectangle<f64, Logical>,
        alpha: f32,
        mut uniforms: Vec<Uniform<'static>>,
        opaque: bool,
    ) -> Self {
        uniforms.push(Uniform::new(
            "area_pt",
            (area.size.w as f32, area.size.h as f32),
        ));
        Self {
            program,
            id,
            commit,
            area,
            alpha,
            uniforms,
            opaque,
        }
    }

    pub fn area(&self) -> Rectangle<f64, Logical> {
        self.area
    }
}

impl Element for ShaderRect {
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

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        if self.opaque {
            OpaqueRegions::from_slice(&[Rectangle::from_size(self.geometry(scale).size)])
        } else {
            OpaqueRegions::default()
        }
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for ShaderRect {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.render_pixel_shader_to(
            &self.program,
            src,
            dst,
            Size::from((1, 1)),
            Some(damage),
            self.alpha,
            &self.uniforms,
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_commits_only_on_signature_change() {
        let mut c = ElementCache::default();
        let k = ElementKey::new("shadow", 1);
        let (id1, c1) = c.slot(k, 5);
        let (id2, c2) = c.slot(k, 5);
        assert_eq!(id1, id2);
        assert_eq!(c1, c2);
        let (_, c3) = c.slot(k, 6);
        assert_ne!(c2, c3);
    }

    #[test]
    fn cache_drops_unused_keys() {
        let mut c = ElementCache::default();
        c.slot(ElementKey::new("a", 0), 0);
        c.slot(ElementKey::new("b", 0), 0);
        c.end_frame();
        assert_eq!(c.len(), 2);
        c.slot(ElementKey::new("a", 0), 0);
        c.end_frame();
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn physical_rect_rounds_edges() {
        let r = Rectangle::<f64, Logical>::new((10.4, 20.6).into(), (100.4, 50.0).into());
        let p = to_physical_rect(r, Scale::from(1.0));
        assert_eq!(p, Rectangle::new((10, 21).into(), (101, 50).into()));
        let p2 = to_physical_rect(r, Scale::from(2.0));
        assert_eq!(p2.loc, (21, 41).into());
    }

    #[test]
    fn signature_differs_by_value() {
        assert_ne!(Sig::new().f(1.0).finish(), Sig::new().f(2.0).finish());
        assert_eq!(
            Sig::new().u(3).f(1.5).finish(),
            Sig::new().u(3).f(1.5).finish()
        );
    }
}
