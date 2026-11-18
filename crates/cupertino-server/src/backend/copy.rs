//! A frame of the render GPU on a display of another card. The render GPU draws into a linear buffer of its own; the
//! display's card imports that buffer as a texture and draws it, waiting on the render's sync point inside its own
//! frame. When the render GPU cannot draw into a linear buffer or the card cannot import it (by the formats they list,
//! or as it turns out), the render GPU draws into a texture of its own, read back into memory and uploaded to the card,
//! only the rows that changed once the card's texture holds the frame before; when that fails too, the display is
//! unsupported and nothing is tried again.
//!
//! The compositor's loop never waits for the render GPU without a deadline: the display's card waits for the render's
//! sync point itself where it can import it, and otherwise the loop waits at most [`CPU_WAIT`]. A frame not finished
//! by then is late: the display keeps showing the frame before, and the late frame is brought at a later refresh.

use std::os::fd::AsFd;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer};
use smithay::backend::allocator::{Format, Fourcc, Modifier, Slot, Swapchain};
use smithay::backend::egl::fence::EGLFence;
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
/// The longest the compositor's loop waits on the CPU for the render GPU to finish a frame, when the display's card
/// cannot wait for it itself or the frame is read back through memory. Past it the frame is late and shown at a later
/// refresh; half a refresh at 60 Hz, so a frame the GPU finishes in time still makes its vblank.
pub const CPU_WAIT: Duration = Duration::from_millis(8);

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

/// What came of handing a frame to the display's card.
pub enum Brought {
    /// The frame for the display's card to draw.
    Frame(CopiedFrame),
    /// The render GPU had not finished the frame within [`CPU_WAIT`]: the display keeps the frame before, and
    /// [`FrameCopy::bring_late`] brings this one at a later refresh.
    Late,
}

/// A frame the render GPU had not finished when it was brought.
enum Late {
    /// Imported by the display's card, drawn once `sync` is reached.
    Imported(CopiedFrame, SyncPoint),
    /// To be read back through memory once `sync` is reached.
    Unread {
        target: Target,
        sync: SyncPoint,
        damage: Vec<Rectangle<i32, Physical>>,
    },
}

/// How a renderer stands to a sync point ([`settle`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// The sync point is reached.
    Reached,
    /// The renderer's GPU waits for it before the commands that follow.
    Queued,
    /// Not reached within the wait, or nothing could bound the wait: the frame is not drawn yet.
    Late,
}

/// Make what `renderer` draws next wait for `sync` without blocking the thread for longer than `wait`: reached already,
/// or the renderer's GPU waiting for it (the fence of its own display, or a native fence it imports), or else waited for
/// on the CPU for at most `wait` ([`finished`]).
pub fn settle(renderer: &mut GlesRenderer, sync: &SyncPoint, wait: Duration) -> Settled {
    if sync.is_reached() {
        return Settled::Reached;
    }
    // make the renderer's context current, for the waits inserted into it
    if renderer.with_context(|_| ()).is_ok() {
        let display = renderer.egl_context().display();
        if sync
            .get::<EGLFence>()
            .is_some_and(|f| f.wait(display).is_ok())
        {
            return Settled::Queued;
        }
        let imported = EGLFence::supports_importing(display)
            .then(|| sync.export())
            .flatten()
            .and_then(|native| EGLFence::import(display, native).ok());
        if imported.is_some_and(|f| f.wait(display).is_ok()) {
            return Settled::Queued;
        }
    }
    match finished(sync, wait) {
        true => Settled::Reached,
        false => Settled::Late,
    }
}

/// Whether `sync` is reached within `wait`, waited for on the CPU. A fence the wait cannot be bounded for (one not of
/// EGL) is only asked whether it is signalled: it is never waited for.
pub fn finished(sync: &SyncPoint, wait: Duration) -> bool {
    if sync.is_reached() {
        return true;
    }
    sync.get::<EGLFence>()
        .is_some_and(|f| f.client_wait(Some(wait), true).unwrap_or(false))
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
    /// The frame the render GPU had not finished when it was brought: brought before another is drawn.
    late: Option<Late>,
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
            late: None,
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
        self.late = None;
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
    /// changed since the frame before. A way that fails is logged once and not tried again. The thread waits for the
    /// render GPU at most [`CPU_WAIT`]; a frame not finished by then is [`Brought::Late`].
    pub fn bring(
        &mut self,
        render: &mut GlesRenderer,
        display: &mut GlesRenderer,
        target: Target,
        sync: SyncPoint,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<Brought, Unsupported> {
        if let Target::Dmabuf { slot, dmabuf } = &target {
            self.swapchain.submitted(slot);
            if self.via == Via::Dmabuf {
                match self.refuse(Via::Dmabuf).and_then(|()| {
                    display
                        .import_dmabuf(dmabuf, None)
                        .map_err(|e| anyhow!("{e}"))
                }) {
                    Ok(texture) => {
                        let frame = self.copied(texture, target, damage);
                        return Ok(self.imported(display, frame, sync));
                    }
                    Err(e) => {
                        tracing::warn!(error = %format!("{e:#}"), "the display's GPU cannot read the render GPU's buffers; its frames go through memory");
                        self.change_via(Via::Memory);
                    }
                }
            }
        }
        if self.via == Via::Memory {
            return self.read(render, display, target, sync, damage.to_vec());
        }
        Err(Unsupported)
    }

    /// Bring the frame that was late, when there is one ([`Brought::Late`]): again late while the render GPU has not
    /// finished it within [`CPU_WAIT`]. `None` without a late frame.
    pub fn bring_late(
        &mut self,
        render: &mut GlesRenderer,
        display: &mut GlesRenderer,
    ) -> Option<Result<Brought, Unsupported>> {
        Some(match self.late.take()? {
            Late::Imported(frame, sync) => Ok(self.imported(display, frame, sync)),
            Late::Unread {
                target,
                sync,
                damage,
            } => self.read(render, display, target, sync, damage),
        })
    }

    /// The frame the display's card imported, once it is finished or the card's GPU waits for it.
    fn imported(
        &mut self,
        display: &mut GlesRenderer,
        frame: CopiedFrame,
        sync: SyncPoint,
    ) -> Brought {
        match settle(display, &sync, CPU_WAIT) {
            Settled::Reached | Settled::Queued => Brought::Frame(frame),
            Settled::Late => {
                tracing::debug!(
                    "the render GPU has not finished the display's frame; it is shown later"
                );
                self.late = Some(Late::Imported(frame, sync));
                Brought::Late
            }
        }
    }

    /// The frame in `target` read back and uploaded to the display's card, once the render GPU finished it.
    fn read(
        &mut self,
        render: &mut GlesRenderer,
        display: &mut GlesRenderer,
        mut target: Target,
        sync: SyncPoint,
        damage: Vec<Rectangle<i32, Physical>>,
    ) -> Result<Brought, Unsupported> {
        // reading back waits for the render GPU in the driver, without a deadline: only a finished frame is read
        if !finished(&sync, CPU_WAIT) {
            tracing::debug!(
                "the render GPU has not finished the display's frame; it is read back later"
            );
            self.late = Some(Late::Unread {
                target,
                sync,
                damage,
            });
            return Ok(Brought::Late);
        }
        match self
            .refuse(Via::Memory)
            .and_then(|()| self.upload(render, display, &mut target, &damage))
        {
            Ok(texture) => Ok(Brought::Frame(self.copied(texture, target, &damage))),
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "the display cannot show the render GPU's frames; it is unsupported");
                self.change_via(Via::Unsupported);
                Err(Unsupported)
            }
        }
    }

    fn copied(
        &mut self,
        texture: GlesTexture,
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

/// The render GPU's frame as the display's card draws it: one texture over the whole display. It is made only once the
/// render GPU finished the frame or the card's GPU waits for it ([`settle`]), so drawing it never waits.
pub struct CopiedFrame {
    id: Id,
    commit: CommitCounter,
    size: Size<i32, Physical>,
    texture: GlesTexture,
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
