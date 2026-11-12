//! Backends: nested winit window (development), DRM/KMS on a TTY (`udev` feature) and an
//! offscreen EGL device (headless tests and screenshots).

use smithay::backend::drm::DrmNode;
use smithay::backend::egl::{EGLContext, EGLDevice};
use smithay::backend::renderer::gles::{Capability, GlesError, GlesRenderer};
use smithay::backend::renderer::{ImportDma, ImportMemWl};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use crate::state::Cupertino;

#[cfg(feature = "udev")]
pub mod copy;
pub mod headless;
#[cfg(feature = "udev")]
pub mod input;
#[cfg(feature = "udev")]
pub mod record;
#[cfg(feature = "udev")]
pub mod udev;
#[cfg(feature = "winit")]
pub mod winit;

pub enum Backend {
    #[cfg(feature = "winit")]
    Winit(Box<winit::WinitBackend>),
    Headless(Box<headless::HeadlessBackend>),
    #[cfg(feature = "udev")]
    Udev(Box<udev::UdevBackend>),
}

impl Backend {
    /// The GLES renderer all backends draw with.
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        match self {
            #[cfg(feature = "winit")]
            Backend::Winit(w) => w.graphics.renderer(),
            Backend::Headless(h) => &mut h.renderer,
            #[cfg(feature = "udev")]
            Backend::Udev(u) => u.renderer(),
        }
    }

    pub fn switch_vt(&mut self, _vt: i32) {
        #[cfg(feature = "udev")]
        if let Backend::Udev(u) = self {
            u.switch_vt(_vt);
        }
    }

    /// Whether no frame reaches the screens now: on DRM the session is paused or the display sleeps (every CRTC off).
    pub fn shows_nothing(&self) -> bool {
        match self {
            #[cfg(feature = "udev")]
            Backend::Udev(u) => u.shows_nothing(),
            _ => false,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "winit")]
            Backend::Winit(_) => "winit",
            Backend::Headless(_) => "headless",
            #[cfg(feature = "udev")]
            Backend::Udev(_) => "udev",
        }
    }
}

/// Whether no frame of `state` reaches the screens now ([`Backend::shows_nothing`]).
pub fn shows_nothing(state: &Cupertino) -> bool {
    state.backend.as_ref().is_some_and(Backend::shows_nothing)
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

/// Create the dmabuf global (with default feedback when the render node is known) and publish the
/// shm formats of `renderer`.
pub fn init_buffers(state: &mut Cupertino, renderer: &GlesRenderer) {
    let formats = renderer.dmabuf_formats();
    let node = render_node(renderer);
    let dh = state.display_handle.clone();
    let global = match node.and_then(|n| {
        DmabufFeedbackBuilder::new(n.dev_id(), formats.clone())
            .build()
            .ok()
    }) {
        Some(feedback) => state
            .dmabuf_state
            .create_global_with_default_feedback::<Cupertino>(&dh, &feedback),
        None => state.dmabuf_state.create_global::<Cupertino>(&dh, formats),
    };
    state.dmabuf_global = Some(global);
    state.shm_state.update_formats(renderer.shm_formats());
}

/// How long until [`Cupertino::tick`] has time-driven work ([`Cupertino::next_tick_deadline`]), in wall time; a passed
/// deadline nothing acted on yet is a millisecond away, so it cannot spin a loop.
pub(crate) fn tick_wait(state: &Cupertino) -> Option<std::time::Duration> {
    const PASSED: std::time::Duration = std::time::Duration::from_millis(1);
    state.next_tick_deadline().map(|at| {
        let wait = (at - state.now()) / state.time_scale;
        if wait > 0.0 {
            std::time::Duration::from_secs_f64(wait)
        } else {
            PASSED
        }
    })
}
