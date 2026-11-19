//! Backend primitives of the compositor (D25): logind Smithay session, frame
//! scheduling, late latching, sync files, output bit-depth, DRM tracing, acquire waits,
//! software-renderer selection, GPU reset tracking, and frame statistics.
//! Depends on no other server crate.

pub mod acquire_waits;
pub mod drm_trace;
pub mod frame_stats;
pub mod gpu_reset;
pub mod latch;
pub mod output_depth;
pub mod schedule;
pub mod session;
pub mod software_render;
pub mod sync_file;
