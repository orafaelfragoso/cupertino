//! Cupertino's thin GPU layer: what a 2D renderer needs and no more.
//!
//! * A [`Device`] on a render node (through GBM) or on the surfaceless platform, or borrowed from a context someone
//!   else made ([`Device::borrow`], with the state-restore contract of [`BorrowedState`]).
//! * [`Buffer`]s (uniform, vertex, index), [`Texture`]s (sampled, render targets, with external memory: dmabufs
//!   allocated by the device or imported), [`Sampler`]s.
//! * Immutable [`Pipeline`]s made from the generated programs of [`shaders`], with their [`BindGroupLayout`]s;
//!   [`BindGroup`]s naming the resources a draw reads.
//! * Explicit render passes ([`Device::begin_pass`]: a target, a load operation, then pipelines, bind groups and
//!   draws; a borrowed device's [`Device::begin_owner_pass`] draws into the framebuffer its owner has bound) and
//!   [`Fence`]s after a submission, with native sync files where the driver has them.
//! * [`atlas`]: 8-bit coverage, colour glyph and path mask atlases with least-recently-used eviction and caps;
//!   [`pool`]: pooled render targets with a cap.
//!
//! The shape follows Vulkan, so a second backend implements the same calls: resources are made from descriptions
//! and destroyed explicitly, a pipeline holds all its state, a pass names its target and how it starts, and nothing
//! of a backend's own objects shows in the API. The one backend today is OpenGL ES 3.0 over EGL. EGL, GLES and GBM
//! are loaded at run time: a machine without them gets [`Error::Unavailable`], never a failed start, and the caller
//! draws on the CPU.
//!
//! Orientation, on every backend: a target's first row (the top of an image handed to the compositor) is at clip
//! y = -1 and fragment row 0, and texture coordinate v = 0 is a texture's first row. Pixels written and read
//! through [`Device::write_texture`] and [`Device::read_texture`] are first row first.
//!
//! Every context is made with robustness and, where the driver offers it, video-memory purge notification;
//! [`Device::status`] reports a reset or a purge ([`Loss`]), after which every call fails with [`Error::Lost`] and
//! the caller makes a new device.

pub mod atlas;
mod gles;
pub mod pool;
pub mod program_cache;
pub mod shaders;
pub mod syncobj;
mod types;

pub use gles::{
    wait_sync_file, BindGroup, BindGroupEntry, BindGroupLayout, BindingResource, BorrowedState,
    Buffer, DetachedDevice, Device, ExternalImage, Fence, Pass, Pipeline, PipelineDesc, Sampler,
    Texture, MAX_BINDINGS, MAX_GROUPS,
};
pub use program_cache::ProgramCacheStats;
pub use types::*;
