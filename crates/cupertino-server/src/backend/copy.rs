//! A frame of the render GPU on a display of another card. The render GPU draws into a linear buffer of its own; the
//! display's card imports that buffer as a texture and draws it, waiting on the render's sync point inside its own
//! frame. When the render GPU cannot draw into a linear buffer or the card cannot import it (by the formats they list,
//! or as it turns out), the render GPU draws into a texture of its own, read back into memory and uploaded to the card,
//! only the rows that changed once the card's texture holds the frame before; when that fails too, the display is
//! unsupported and nothing is tried again.

use std::os::fd::AsFd;

use anyhow::{anyhow, Context, Result};
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer};
use smithay::backend::allocator::{Format, Fourcc, Modifier, Slot, Swapchain};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{
    ffi, GlesError, GlesFrame, GlesMapping, GlesRenderer, GlesTexture,
};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{
    Bind, ExportMem, Frame, ImportDma, ImportMem, Offscreen, Texture,
};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Size, Transform};

/// The format of the buffers the display's card imports.
const FORMAT: Fourcc = Fourcc::Argb8888;
/// The format of the frames that go through memory.
const MEMORY_FORMAT: Fourcc = Fourcc::Abgr8888;

/// How the display's card gets the frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// It imports the render GPU's buffer.
    Dmabuf,
    /// The frame is read back into memory and uploaded to it.
    Memory,
    /// Neither works: the display shows nothing.
    Unsupported,
}

/// The display cannot show the render GPU's frames.
#[derive(Debug)]
pub struct Unsupported;

/// What the render GPU draws the next frame into.
pub enum Target {
    /// A linear buffer for the display's card to import.
    Dmabuf {
        slot: Slot<GbmBuffer>,
        dmabuf: Dmabuf,
    },
    /// A texture of the render GPU's own, read back through memory, holding the frame before when `age` is 1.
    Texture { texture: GlesTexture, age: usize },
}

/// The copies of one display's frames.
pub struct FrameCopy<F: AsFd + Clone + 'static> {
    swapchain: Swapchain<GbmAllocator<F>>,
    /// Damage of the render GPU's frames, upright at the display's pixel size.
    pub tracker: OutputDamageTracker,
    size: Size<i32, Physical>,
    scale: f64,
    via: Via,
    /// The memory path's texture on the render GPU.
    drawn: Option<GlesTexture>,
    /// The memory path's texture on the display's card, updated in place.
    uploaded: Option<GlesTexture>,
    id: Id,
    commit: CommitCounter,
    /// The buffer the display's card read last: the render GPU does not draw into it again before the next frame is
    /// queued.
    shown: Option<Slot<GbmBuffer>>,
}

/// How a display of `display`'s card gets the frames of `render` first: through a linear buffer where both list it.
pub fn first_via(render: &GlesRenderer, display: &GlesRenderer) -> Via {
    let linear = Format {
        code: FORMAT,
        modifier: Modifier::Linear,
    };
    match render
        .egl_context()
        .dmabuf_render_formats()
        .contains(&linear)
        && display.dmabuf_formats().contains(&linear)
    {
        true => Via::Dmabuf,
        false => Via::Memory,
    }
}

impl<F: AsFd + Clone + 'static> FrameCopy<F> {
    /// The copies for a display of `size` pixels upright (its mode turned by its transform) at `scale`, in linear
    /// buffers of the render GPU from `allocator`, starting `via` ([`first_via`], or the way an earlier copy of the
    /// display found, so a display given another mode does not try again what failed).
    pub fn new(
        allocator: GbmAllocator<F>,
        size: Size<i32, Physical>,
        scale: f64,
        via: Via,
    ) -> Self {
        Self {
            swapchain: Swapchain::new(
                allocator,
                size.w as u32,
                size.h as u32,
                FORMAT,
                vec![Modifier::Linear],
            ),
            tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
            size,
            scale,
            via,
            drawn: None,
            uploaded: None,
            id: Id::new(),
            commit: CommitCounter::default(),
            shown: None,
        }
    }

    pub fn via(&self) -> Via {
        self.via
    }

    /// Whether the copies are for a display of `size` pixels upright at `scale`.
    pub fn fits(&self, size: Size<i32, Physical>, scale: f64) -> bool {
        self.size == size && self.scale == scale
    }

    /// The render GPU could not draw into the target of [`FrameCopy::acquire`]: a linear buffer is given up for
    /// memory; that failing too, the display is unsupported.
    pub fn cannot_draw(&mut self, error: &dyn std::fmt::Display) {
        let via = match self.via {
            Via::Dmabuf => {
                tracing::warn!(error = %error, "the render GPU cannot draw into a buffer another GPU reads; the display's frames go through memory");
                Via::Memory
            }
            Via::Memory | Via::Unsupported => {
                tracing::error!(error = %error, "the render GPU cannot draw the display's frames; it is unsupported");
                Via::Unsupported
            }
        };
        self.change_via(via);
        self.shown = None;
    }

    /// Go on `via` as a new element: nothing of the last frame is on the next texture.
    fn change_via(&mut self, via: Via) {
        self.via = via;
        self.drawn = None;
        self.id = Id::new();
    }

    /// What to draw the next frame into.
    pub fn acquire(&mut self, render: &mut GlesRenderer) -> Result<Target> {
        match self.via {
            Via::Dmabuf => {
                let slot = self
                    .swapchain
                    .acquire()
                    .map_err(|e| anyhow!("allocating a copy buffer: {e}"))?
                    .context("every copy buffer is in use")?;
                let dmabuf = slot
                    .export()
                    .map_err(|e| anyhow!("exporting a copy buffer: {e}"))?;
                Ok(Target::Dmabuf { slot, dmabuf })
            }
            Via::Memory => match &self.drawn {
                Some(texture) => Ok(Target::Texture {
                    texture: texture.clone(),
                    age: 1,
                }),
                None => {
                    let texture: GlesTexture = render
                        .create_buffer(MEMORY_FORMAT, (self.size.w, self.size.h).into())
                        .map_err(|e| anyhow!("creating a copy texture: {e}"))?;
                    self.drawn = Some(texture.clone());
                    Ok(Target::Texture { texture, age: 0 })
                }
            },
            Via::Unsupported => anyhow::bail!("the display is unsupported"),
        }
    }

    /// Hand the frame drawn into `target`, finished once `sync` is signalled, to the display's card. `damage` is what
    /// changed since the frame before. A way that fails is logged once and not tried again.
    pub fn bring(
        &mut self,
        render: &mut GlesRenderer,
        display: &mut GlesRenderer,
        mut target: Target,
        sync: SyncPoint,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<CopiedFrame, Unsupported> {
        if let Target::Dmabuf { slot, dmabuf } = &target {
            self.swapchain.submitted(slot);
            if self.via == Via::Dmabuf {
                match self.refuse(Via::Dmabuf).and_then(|()| {
                    display
                        .import_dmabuf(dmabuf, None)
                        .map_err(|e| anyhow!("{e}"))
                }) {
                    Ok(texture) => return Ok(self.copied(texture, sync, target, damage)),
                    Err(e) => {
                        tracing::warn!(error = %format!("{e:#}"), "the display's GPU cannot read the render GPU's buffers; its frames go through memory");
                        self.change_via(Via::Memory);
                    }
                }
            }
        }
        if self.via == Via::Memory {
            match self
                .refuse(Via::Memory)
                .and_then(|()| self.upload(render, display, &mut target, damage))
            {
                Ok(texture) => {
                    return Ok(self.copied(texture, SyncPoint::signaled(), target, damage))
                }
                Err(e) => {
                    tracing::error!(error = %format!("{e:#}"), "the display cannot show the render GPU's frames; it is unsupported");
                    self.change_via(Via::Unsupported);
                }
            }
        }
        Err(Unsupported)
    }

    fn copied(
        &mut self,
        texture: GlesTexture,
        sync: SyncPoint,
        target: Target,
        damage: &[Rectangle<i32, Physical>],
    ) -> CopiedFrame {
        self.commit.increment();
        if let Target::Dmabuf { slot, .. } = target {
            self.shown = Some(slot);
        }
        CopiedFrame {
            id: self.id.clone(),
            commit: self.commit,
            size: self.size,
            texture,
            sync,
            damage: damage.to_vec(),
        }
    }

    /// Read the frame in `target` back on the render GPU and upload it to the display's card: only the rows `damage`
    /// covers where the card's texture holds the frame the render GPU's texture was drawn over, else all of it.
    fn upload(
        &mut self,
        render: &mut GlesRenderer,
        display: &mut GlesRenderer,
        target: &mut Target,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<GlesTexture> {
        let full = Rectangle::from_size(Size::<i32, Buffer>::from((self.size.w, self.size.h)));
        let texture = match &self.uploaded {
            Some(texture) if matches!(target, Target::Texture { age: 1, .. }) => texture.clone(),
            uploaded => {
                let mapping = read_back(render, target, full)?;
                let data = render.map_texture(&mapping)?;
                let texture = match uploaded {
                    Some(texture) => {
                        display.update_memory(texture, data, full)?;
                        texture.clone()
                    }
                    // the rows come top first; a texture marked flipped would be drawn as its first row repeated
                    None => display.import_memory(data, MEMORY_FORMAT, full.size, false)?,
                };
                self.copied_rows(full);
                self.uploaded = Some(texture.clone());
                return Ok(texture);
            }
        };
        for rows in damaged_rows(damage, self.size) {
            let mapping = read_back(render, target, rows)?;
            let data = render.map_texture(&mapping)?;
            // smithay's update_memory takes the region out of a whole frame's pixels; these are the region's alone
            display.with_context(|gl| unsafe {
                gl.BindTexture(ffi::TEXTURE_2D, texture.tex_id());
                gl.TexSubImage2D(
                    ffi::TEXTURE_2D,
                    0,
                    rows.loc.x,
                    rows.loc.y,
                    rows.size.w,
                    rows.size.h,
                    ffi::RGBA,
                    ffi::UNSIGNED_BYTE,
                    data.as_ptr().cast(),
                );
                gl.BindTexture(ffi::TEXTURE_2D, 0);
            })?;
            self.copied_rows(rows);
        }
        Ok(texture)
    }

    fn copied_rows(&mut self, _: Rectangle<i32, Buffer>) {}

    fn refuse(&self, _: Via) -> Result<()> {
        Ok(())
    }
}

fn read_back(
    render: &mut GlesRenderer,
    target: &mut Target,
    region: Rectangle<i32, Buffer>,
) -> Result<GlesMapping> {
    let framebuffer = match target {
        Target::Dmabuf { dmabuf, .. } => render.bind(dmabuf)?,
        Target::Texture { texture, .. } => render.bind(texture)?,
    };
    Ok(render.copy_framebuffer(&framebuffer, region, MEMORY_FORMAT)?)
}

/// The rows `damage` covers in a frame of `size`, as bands across its whole width, merged where they meet.
fn damaged_rows(
    damage: &[Rectangle<i32, Physical>],
    size: Size<i32, Physical>,
) -> Vec<Rectangle<i32, Buffer>> {
    let mut spans: Vec<(i32, i32)> = damage
        .iter()
        .map(|r| (r.loc.y.max(0), (r.loc.y + r.size.h).min(size.h)))
        .filter(|(top, bottom)| top < bottom)
        .collect();
    spans.sort_unstable();
    let mut bands: Vec<(i32, i32)> = Vec::new();
    for (top, bottom) in spans {
        match bands.last_mut() {
            Some(band) if top <= band.1 => band.1 = band.1.max(bottom),
            _ => bands.push((top, bottom)),
        }
    }
    bands
        .into_iter()
        .map(|(top, bottom)| Rectangle::new((0, top).into(), (size.w, bottom - top).into()))
        .collect()
}

/// The render GPU's frame as the display's card draws it: one texture over the whole display.
pub struct CopiedFrame {
    id: Id,
    commit: CommitCounter,
    size: Size<i32, Physical>,
    texture: GlesTexture,
    /// Signalled when the render GPU finished the frame.
    sync: SyncPoint,
    /// What changed since the frame of the commit before.
    damage: Vec<Rectangle<i32, Physical>>,
}

impl Element for CopiedFrame {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(self.texture.size()).to_f64()
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_size(self.size)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self.commit.distance(commit) {
            Some(0) => DamageSet::default(),
            Some(1) => DamageSet::from_slice(&self.damage),
            _ => DamageSet::from_slice(&[self.geometry(scale)]),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::from_slice(&[self.geometry(scale)])
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for CopiedFrame {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.wait(&self.sync)?;
        Frame::render_texture_from_to(
            frame,
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            1.0,
        )
    }
}
