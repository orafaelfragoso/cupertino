use super::*;

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
        let _material = tracing::trace_span!("server.material", rects = damage.len()).entered();
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
