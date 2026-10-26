//! Renderer-agnostic window effect generators.
//!
//! Every effect is a pure function of a progress value (and an optional driver that turns time
//! into progress), so any renderer can evaluate it per frame:
//!
//! * [`warp`]: warp minimize, a two-phase funnel mesh (2 × N vertices, N adaptive).
//! * [`funnel`]: funnel minimize, a regular grid of 70 pt cells (at most 20 × 20 vertices).
//! * [`scale`]: scale minimize, a single quad.
//! * [`minimize`]: the three effects with their durations (500/400/200 ms) as reversible,
//!   interruptible animations.
//! * [`transitions`]: per-window-kind open/close transforms, zoom and full-screen frame changes,
//!   workspace slide, overview zoom and show desktop as [`transitions::Placement`]s.
//! * [`mesh`]: the common output, a vertex grid with texture coordinates.
//! * [`ease`]: easing curves and the interruptible progress animator.
//! * [`raster`]: a small CPU renderer of textured meshes used for tests and sample images.

pub mod ease;
pub mod funnel;
pub mod mesh;
pub mod minimize;
pub mod raster;
pub mod scale;
pub mod space_slide;
pub mod transitions;
pub mod warp;

pub use ease::{cosine, Driver, Ease, ProgressAnimator};
pub use funnel::{funnel_cells, funnel_mesh, FunnelParams};
pub use mesh::{Mesh, Vertex};
pub use minimize::{MinimizeAnimation, MinimizeEffect, MinimizeKind, MinimizeParams, MinimizePhase};
pub use scale::{lerp_rect, scale_mesh, scale_rect};
pub use space_slide::{point_in_x, settle_time, SlideStart, SpaceSlideState};
pub use transitions::{
    frame_change_duration, frame_max_delta, server_move_driver, FullScreenTransition, OverviewZoom, Placement, ShowDesktop, TilingSprings,
    TransformKind, TransformSample, WindowKind, WindowZoom, WorkspaceSlide, SERVER_MOVE_DURATION, SHEET_DURATION, SHEET_SLIDE,
};
pub use warp::{subdivide, warp_mesh, DockEdge, Landing, WarpDirection, WarpGeometry, WarpParams, WarpTiming};
