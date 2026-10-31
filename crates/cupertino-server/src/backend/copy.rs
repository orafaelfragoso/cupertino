//! A frame of the render GPU on a display of another card. The render GPU draws into a buffer of its own; the
//! display's card imports that buffer as a texture and draws it, waiting on the render's sync point inside its own
//! frame. The buffer is linear, which the other card can read. When the render GPU cannot draw into a linear buffer or
//! the card cannot import it, the frame goes through memory, from a buffer in the render GPU's own layout; when that
//! fails too, the display is unsupported and nothing is tried again.

use std::os::fd::AsFd;

use anyhow::{anyhow, Context, Result};
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer};
use smithay::backend::allocator::{Fourcc, Modifier, Slot, Swapchain};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{
    Bind, ExportMem, Frame, ImportDma, ImportMem, Texture, TextureMapping,
};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Size, Transform};

const FORMAT: Fourcc = Fourcc::Argb8888;

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

/// The copies of one display's frames.
pub struct FrameCopy<F: AsFd + Clone + 'static> {
    allocator: GbmAllocator<F>,
    /// What the render GPU can draw into ([`render_modifiers`]).
    modifiers: Vec<Modifier>,
    swapchain: Swapchain<GbmAllocator<F>>,
    /// Damage of the render GPU's frames, upright at the display's pixel size.
    pub tracker: OutputDamageTracker,
    size: Size<i32, Physical>,
    scale: f64,
    via: Via,
    /// The memory path's texture on the display's card, updated in place.
    uploaded: Option<GlesTexture>,
    id: Id,
    commit: CommitCounter,
    /// The buffer the display's card read last: the render GPU does not draw into it again before the next frame is
    /// queued.
    shown: Option<Slot<GbmBuffer>>,
    #[cfg(test)]
    refused: Vec<Via>,
}

/// The modifiers the render GPU can draw `FORMAT` with, for the memory path's buffers.
pub fn render_modifiers(render: &GlesRenderer) -> Vec<Modifier> {
    render
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .filter(|f| f.code == FORMAT)
        .map(|f| f.modifier)
        .collect()
}

impl<F: AsFd + Clone + 'static> FrameCopy<F> {
    fn swapchain(&self) -> Swapchain<GbmAllocator<F>> {
        let modifiers = match self.via {
            Via::Dmabuf => vec![Modifier::Linear],
            Via::Memory | Via::Unsupported => self.modifiers.clone(),
        };
        Swapchain::new(
            self.allocator.clone(),
            self.size.w as u32,
            self.size.h as u32,
            FORMAT,
            modifiers,
        )
    }
}

impl<F: AsFd + Clone + 'static> FrameCopy<F> {
    /// The copies for a display of `size` pixels upright (its mode turned by its transform) at `scale`, in buffers of the
    /// render GPU from `allocator`, which draws with `modifiers`.
    pub fn new(
        allocator: GbmAllocator<F>,
        modifiers: Vec<Modifier>,
        size: Size<i32, Physical>,
        scale: f64,
    ) -> Self {
        let swapchain = Swapchain::new(
            allocator.clone(),
            size.w as u32,
            size.h as u32,
            FORMAT,
            vec![Modifier::Linear],
        );
        Self {
            swapchain,
            allocator,
            modifiers,
            tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
            size,
            scale,
            via: Via::Dmabuf,
            uploaded: None,
            id: Id::new(),
            commit: CommitCounter::default(),
            shown: None,
            #[cfg(test)]
            refused: Vec::new(),
        }
    }

    pub fn via(&self) -> Via {
        self.via
    }

    /// Whether the copies are for a display of `size` pixels upright at `scale`.
    pub fn fits(&self, size: Size<i32, Physical>, scale: f64) -> bool {
        self.size == size && self.scale == scale
    }

    /// Go on the way an earlier copy of the display found, so a display given another mode does not try again what
    /// failed.
    pub fn continue_via(&mut self, via: Via) {
        if via != self.via {
            self.via = via;
            self.swapchain = self.swapchain();
        }
    }

    /// The render GPU could not draw into the buffer of [`FrameCopy::acquire`]: a linear one is given up for one in its
    /// own layout, read back through memory; that failing too, the display is unsupported.
    pub fn cannot_draw(&mut self, error: &dyn std::fmt::Display) {
        self.via = match self.via {
            Via::Dmabuf => {
                tracing::warn!(error = %error, "the render GPU cannot draw into a buffer another GPU reads; the display's frames go through memory");
                Via::Memory
            }
            Via::Memory | Via::Unsupported => {
                tracing::error!(error = %error, "the render GPU cannot draw the display's frames; it is unsupported");
                Via::Unsupported
            }
        };
        self.swapchain = self.swapchain();
        self.shown = None;
        self.id = Id::new();
    }

    /// A buffer to draw the next frame into, its age and the buffer itself.
    pub fn acquire(&mut self) -> Result<(Slot<GbmBuffer>, Dmabuf)> {
        anyhow::ensure!(self.via != Via::Unsupported, "the display is unsupported");
        let slot = self
            .swapchain
            .acquire()
            .map_err(|e| anyhow!("allocating a copy buffer: {e}"))?
            .context("every copy buffer is in use")?;
        let dmabuf = slot
            .export()
            .map_err(|e| anyhow!("exporting a copy buffer: {e}"))?;
        Ok((slot, dmabuf))
    }

    /// Hand the frame drawn into `slot`, finished once `sync` is signalled, to the display's card. `damage` is what
    /// changed since the frame before. A way that fails is logged once and not tried again.
    #[allow(clippy::too_many_arguments)]
    pub fn bring(
        &mut self,
        render: &mut GlesRenderer,
        display: &mut GlesRenderer,
        slot: Slot<GbmBuffer>,
        mut dmabuf: Dmabuf,
        sync: SyncPoint,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<CopiedFrame, Unsupported> {
        self.swapchain.submitted(&slot);
        let before = self.via;
        let brought = loop {
            match self.via {
                Via::Dmabuf => match self.refuse(Via::Dmabuf).and_then(|()| {
                    display
                        .import_dmabuf(&dmabuf, None)
                        .map_err(|e| anyhow!("{e}"))
                }) {
                    Ok(texture) => break (texture, sync),
                    Err(e) => {
                        tracing::warn!(error = %format!("{e:#}"), "the display's GPU cannot read the render GPU's buffers; its frames go through memory");
                        self.via = Via::Memory;
                    }
                },
                Via::Memory => match self
                    .refuse(Via::Memory)
                    .and_then(|()| self.upload(render, display, &mut dmabuf))
                {
                    Ok(texture) => break (texture, SyncPoint::signaled()),
                    Err(e) => {
                        tracing::error!(error = %format!("{e:#}"), "the display cannot show the render GPU's frames; it is unsupported");
                        self.via = Via::Unsupported;
                    }
                },
                Via::Unsupported => return Err(Unsupported),
            }
        };
        if self.via != before {
            // the texture is another one: nothing of the last frame is on it
            self.id = Id::new();
        }
        self.commit.increment();
        self.shown = Some(slot);
        let (texture, sync) = brought;
        Ok(CopiedFrame {
            id: self.id.clone(),
            commit: self.commit,
            size: self.size,
            texture,
            sync,
            damage: damage.to_vec(),
        })
    }

    /// Read the frame back on the render GPU and upload it to the display's card.
    fn upload(
        &mut self,
        render: &mut GlesRenderer,
        display: &mut GlesRenderer,
        dmabuf: &mut Dmabuf,
    ) -> Result<GlesTexture> {
        let region = Rectangle::from_size(Size::<i32, Buffer>::from((self.size.w, self.size.h)));
        let mapping = {
            let framebuffer = render.bind(dmabuf)?;
            render.copy_framebuffer(&framebuffer, region, FORMAT)?
        };
        let data = render.map_texture(&mapping)?;
        match &self.uploaded {
            Some(texture) => {
                display.update_memory(texture, data, region)?;
                Ok(texture.clone())
            }
            None => {
                let texture =
                    display.import_memory(data, FORMAT, region.size, mapping.flipped())?;
                self.uploaded = Some(texture.clone());
                Ok(texture)
            }
        }
    }

    #[cfg(test)]
    fn refuse(&self, via: Via) -> Result<()> {
        match self.refused.contains(&via) {
            true => Err(anyhow!("refused by the test")),
            false => Ok(()),
        }
    }

    #[cfg(not(test))]
    fn refuse(&self, _: Via) -> Result<()> {
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::OwnedFd;

    use smithay::utils::DeviceFd;

    use smithay::backend::allocator::gbm::{GbmBufferFlags, GbmDevice};
    use smithay::backend::renderer::{Color32F, Offscreen, Renderer};

    use super::*;

    const RED: Color32F = Color32F::new(1.0, 0.0, 0.0, 1.0);
    const GREEN: Color32F = Color32F::new(0.0, 1.0, 0.0, 1.0);

    /// Buffers of the render node, and two renderers with displays of their own, as on two GPUs.
    fn devices() -> Option<(GbmDevice<DeviceFd>, GlesRenderer, GlesRenderer)> {
        let file = File::options()
            .read(true)
            .write(true)
            .open("/dev/dri/renderD128")
            .ok()?;
        let gbm = GbmDevice::new(DeviceFd::from(OwnedFd::from(file))).ok()?;
        let render = crate::backend::headless::create_renderer().ok()?;
        let display = crate::backend::headless::create_renderer().ok()?;
        Some((gbm, render, display))
    }

    /// Draw `color` over a frame on the render GPU and hand it to the display's card.
    fn frame(
        copy: &mut FrameCopy<DeviceFd>,
        render: &mut GlesRenderer,
        display: &mut GlesRenderer,
        color: Color32F,
    ) -> Result<CopiedFrame, Unsupported> {
        let size = copy.size;
        loop {
            let Ok((slot, mut dmabuf)) = copy.acquire() else {
                return Err(Unsupported);
            };
            // as the render target does it: a buffer the render GPU cannot draw into is given up
            let drawn = render.bind(&mut dmabuf).and_then(|mut framebuffer| {
                let mut frame = render.render(&mut framebuffer, size, Transform::Normal)?;
                frame.clear(color, &[Rectangle::from_size(size)])?;
                frame.finish()
            });
            match drawn {
                Ok(sync) => {
                    return copy.bring(
                        render,
                        display,
                        slot,
                        dmabuf,
                        sync,
                        &[Rectangle::from_size(size)],
                    )
                }
                Err(e) => copy.cannot_draw(&e),
            }
        }
    }

    /// The first pixel of `copied` as the display's card draws it.
    fn shown(display: &mut GlesRenderer, copied: &CopiedFrame) -> [u8; 4] {
        let size = copied.size;
        let mut target: GlesTexture = display
            .create_buffer(FORMAT, Size::from((size.w, size.h)))
            .unwrap();
        let region = Rectangle::from_size(Size::<i32, Buffer>::from((1, 1)));
        let mapping = {
            let mut framebuffer = display.bind(&mut target).unwrap();
            {
                let mut frame = display
                    .render(&mut framebuffer, size, Transform::Normal)
                    .unwrap();
                let full = Rectangle::from_size(size);
                copied
                    .draw(&mut frame, copied.src(), full, &[full], &[])
                    .unwrap();
                frame.finish().unwrap().wait().unwrap();
            }
            display
                .copy_framebuffer(&framebuffer, region, Fourcc::Abgr8888)
                .unwrap()
        };
        let data = display.map_texture(&mapping).unwrap();
        [data[0], data[1], data[2], data[3]]
    }

    #[test]
    fn a_frame_reaches_the_other_card_through_its_buffer_then_memory_then_not_at_all() {
        let Some((gbm, mut render, mut display)) = devices() else {
            eprintln!("skipping: no GBM render node");
            return;
        };
        let allocator = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
        let mut copy = FrameCopy::new(
            allocator,
            render_modifiers(&render),
            Size::from((64, 32)),
            1.0,
        );

        // through the buffer where the render GPU can draw into a linear one (not on the proprietary NVIDIA driver)
        let copied = frame(&mut copy, &mut render, &mut display, RED).unwrap();
        assert_eq!(shown(&mut display, &copied), [255, 0, 0, 255]);
        let first = copied.id().clone();

        // the display's card cannot import the buffer: the frame goes through memory, as a new element
        copy.via = Via::Dmabuf;
        copy.refused = vec![Via::Dmabuf];
        let copied = frame(&mut copy, &mut render, &mut display, GREEN).unwrap();
        assert_eq!(copy.via(), Via::Memory);
        assert_eq!(shown(&mut display, &copied), [0, 255, 0, 255]);
        assert_ne!(copied.id(), &first);
        let copied = frame(&mut copy, &mut render, &mut display, RED).unwrap();
        assert_eq!(shown(&mut display, &copied), [255, 0, 0, 255]);

        // memory fails too: the display is unsupported, and nothing is tried again once the cause is gone
        copy.refused = vec![Via::Dmabuf, Via::Memory];
        assert!(frame(&mut copy, &mut render, &mut display, GREEN).is_err());
        copy.refused.clear();
        assert!(frame(&mut copy, &mut render, &mut display, GREEN).is_err());
        assert_eq!(copy.via(), Via::Unsupported);
    }
}
