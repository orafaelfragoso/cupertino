//! Shared DRM/EGL and main-loop wait helpers used by the compositor backends and renderer.

use std::time::Duration;

use smithay::backend::allocator::gbm::GbmAllocator;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::DrmOutput;
use smithay::backend::drm::{DrmDeviceFd, DrmNode};
use smithay::backend::egl::{EGLContext, EGLDevice};
use smithay::backend::renderer::gles::{Capability, GlesError, GlesRenderer};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::Buffer;
use smithay::desktop::utils::OutputPresentationFeedback;

use crate::schedule::PresentationId;

/// The DRM output of one display, as the renderer draws to it.
pub type DrmSurfaceOutput =
    DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, Frame, DrmDeviceFd>;

/// What a queued flip carries to its vblank.
pub struct Frame {
    /// The scheduler's id for it.
    pub id: PresentationId,
    /// Presentation feedback delivered to clients once the flip lands.
    pub feedback: OutputPresentationFeedback,
    /// The client buffers it was drawn from. Dropping the last handle of one signals its
    /// explicit-sync release point, which must wait until the GPU finished reading it: once the
    /// flip is done, the frame drawn from it is finished.
    pub buffers: Vec<Buffer>,
    /// Its number in the DRM trace, when the trace is on ([`crate::drm_trace::DrmTrace::queued`]).
    pub trace: Option<u64>,
}

/// A GLES renderer on `context` without GL fences.
///
/// Fences only order work between shared contexts, and every backend owns one unshared context,
/// so they add nothing. Some drivers also keep memory for every fence ever created, even after it
/// is deleted, so a fence per texture upload and per draw grows the server without bound.
///
/// # Safety
///
/// `context` must be used by this renderer alone, as for [`GlesRenderer::new`].
pub unsafe fn gles_renderer(context: EGLContext) -> Result<GlesRenderer, GlesError> {
    let capabilities = unsafe { GlesRenderer::supported_capabilities(&context) }?
        .into_iter()
        .filter(|c| *c != Capability::Fencing);
    unsafe { GlesRenderer::with_capabilities(context, capabilities) }
}

/// The render node of the EGL display `renderer` draws on, if it has one.
pub fn render_node(renderer: &GlesRenderer) -> Option<DrmNode> {
    EGLDevice::device_for_display(renderer.egl_context().display())
        .ok()
        .and_then(|d| d.try_get_render_node().ok().flatten())
}

/// What the primary plane of a DRM frame waits for before it is shown: the render's sync point,
/// `None` when an element is scanned out on the plane and nothing was drawn.
pub fn primary_sync<B, F, E>(
    primary: &smithay::backend::drm::compositor::PrimaryPlaneElement<'_, B, F, E>,
) -> Option<SyncPoint>
where
    B: smithay::backend::allocator::Buffer,
    F: smithay::backend::drm::Framebuffer,
{
    match primary {
        smithay::backend::drm::compositor::PrimaryPlaneElement::Swapchain(e) => {
            Some(e.sync.clone())
        }
        smithay::backend::drm::compositor::PrimaryPlaneElement::Element(_) => None,
    }
}

/// The longest a main loop sleeps with nothing due. Every piece of time-driven work names its
/// deadline, and events, frames and the end of a run wake the loop themselves, so on an idle
/// desktop the loop sleeps; this is only a backstop against a deadline left unnamed, which then
/// runs a minute late instead of never.
pub const REST_WAIT: Duration = Duration::from_secs(60);

/// How long a main loop sleeps: until the first of `waits` (a frame, the tick's work, the end of
/// the run), at most [`REST_WAIT`].
pub fn loop_wait(waits: impl IntoIterator<Item = Option<Duration>>) -> Duration {
    waits
        .into_iter()
        .flatten()
        .min()
        .map_or(REST_WAIT, |wait| wait.min(REST_WAIT))
}
