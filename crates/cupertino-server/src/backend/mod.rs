//! Backends: nested winit window (development), DRM/KMS on a TTY (`udev` feature) and an
//! offscreen EGL device (headless tests and screenshots).

use smithay::backend::egl::EGLDevice;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ImportDma, ImportMemWl};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use crate::state::Cupertino;

pub mod headless;
#[cfg(feature = "udev")]
pub mod input;
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

/// Create the dmabuf global (with default feedback when the render node is known) and publish the
/// shm formats of `renderer`.
pub fn init_buffers(state: &mut Cupertino, renderer: &GlesRenderer) {
    let formats = renderer.dmabuf_formats();
    let node = EGLDevice::device_for_display(renderer.egl_context().display()).ok().and_then(|d| d.try_get_render_node().ok().flatten());
    let dh = state.display_handle.clone();
    let global = match node.and_then(|n| DmabufFeedbackBuilder::new(n.dev_id(), formats.clone()).build().ok()) {
        Some(feedback) => state.dmabuf_state.create_global_with_default_feedback::<Cupertino>(&dh, &feedback),
        None => state.dmabuf_state.create_global::<Cupertino>(&dh, formats),
    };
    state.dmabuf_global = Some(global);
    state.shm_state.update_formats(renderer.shm_formats());
}
