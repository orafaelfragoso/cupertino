//! Backend building blocks: cross-GPU frame copy, evdev input translation and hotplug,
//! active-turn stall recording, and shared DRM/EGL helpers.

pub mod copy;
pub mod drm;
pub mod input;
pub mod stalls;

pub use drm::{
    gles_renderer, loop_wait, primary_sync, render_node, DrmSurfaceOutput, Frame, REST_WAIT,
};
