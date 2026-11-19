//! Backends: nested winit window (development), DRM/KMS on a TTY (`udev` feature) and an
//! offscreen EGL device (headless tests and screenshots).

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ImportDma, ImportMemWl};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use crate::state::Cupertino;

#[cfg(feature = "udev")]
pub use cupertino_server_backend::backend::copy;
#[cfg(feature = "udev")]
pub use cupertino_server_backend::backend::input;
pub use cupertino_server_backend::backend::{
    gles_renderer, loop_wait, primary_sync, render_node, stalls, DrmSurfaceOutput, Frame, REST_WAIT,
};
pub mod headless;
pub mod record;
#[cfg(feature = "udev")]
pub mod udev;
#[cfg(feature = "winit")]
pub mod winit;

#[cfg(feature = "udev")]
impl cupertino_server_backend::backend::input::EvdevHost for Cupertino {
    fn evdev_clock_now(&self) -> std::time::Duration {
        std::time::Duration::from(self.clock.now())
    }

    fn evdev_natural_scrolling_for(&self, touchpad: bool) -> bool {
        self.config.pointer.natural_scrolling_for(touchpad)
    }

    fn evdev_pass_unfiltered_key(&mut self, code: u32) {
        self.key_filters.pass_unfiltered(code);
    }

    fn evdev_process_input_event(
        &mut self,
        event: smithay::backend::input::InputEvent<input::EvdevBackend>,
    ) {
        self.process_input_event(event);
    }
}

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

/// Create the dmabuf global (with default feedback when the render node is known) and publish the
/// shm formats of `renderer`; with a render node and [`Cupertino::program_cache_base`], start making the toolkit's GPU
/// programs into that cache ([`crate::toolkit_programs`]).
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
    // the toolkit's programs for the device the clients are offered, once, off the frame path
    if let (Some(path), Some(base)) = (
        node.and_then(|n| n.dev_path()),
        state.program_cache_base.take(),
    ) {
        state.toolkit_programs = crate::toolkit_programs::start(path, base);
    }
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
