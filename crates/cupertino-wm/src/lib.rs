//! Pure-logic window-management model for a Wayland compositor.
//!
//! Drive it with [`Wm::handle`] (commands in, effects out) and [`Wm::tick`] (absolute timestamps in
//! seconds). The compositor maps [`Effect`]s to protocol requests and reads
//! [`Wm::presentation`] / [`Wm::workspace_position`] / [`Wm::snap_preview`] when rendering.
//!
//! Sources:,
//! the workspace. Unless a doc comment says, behaviour and constants are
//! approximate or unknown and configurable through [`WmConfig`].

pub mod activation;
pub mod command;
pub mod config;
pub mod focus;
pub mod geometry;
pub mod ids;
pub mod interaction;
pub mod kind;
pub mod lifecycle;
pub mod magnet;
pub mod menu_bar;
pub mod pieces;
mod slide;
pub mod tiling;
pub mod window;
pub mod wm;
pub mod workspace;
pub mod zoom;

pub use activation::{
    token_accepted, GRANT_LIFETIME, LATE_ACTIVATION, TYPING_QUIET, USER_EVENT_SLACK,
};
pub use command::{
    Activation, AnimationKind, Command, Effect, Grant, MapFocus, MapParams, Modifiers, Requester,
};
pub use config::{
    AnimationConfig, DesktopPeekConfig, FocusConfig, MagnetConfig, SnapConfig, WmConfig,
    WorkspaceConfig, ZoomConfig,
};
pub use cupertino_effects::{TransformKind, WindowKind};
pub use focus::{hidden_index, Switcher, SwitcherStep, CASCADE_STEP, SWITCHER_SHOW_DELAY};
pub use geometry::{Edges, Point, Rect, Size};
pub use ids::{AppId, OutputId, WindowId, WorkspaceId};
pub use interaction::PreviewPresentation;
pub use kind::{child_frame, modal_panel_frame, sheet_frame, window_kind, MODAL_CASCADE};
pub use lifecycle::{FrameShape, Lifecycle, LifecycleState, Presentation, Resting, ToplevelStates};
pub use magnet::{unsnap_distance, Guide, Magnet, MagnetStep};
pub use tiling::{hot_zone, hot_zone_hit, tile_rect, HotZoneHit, TileKind};
pub use window::{App, Assignment, DisabledControls, Window, WindowLevel};
pub use wm::{Output, Wm};
pub use workspace::{Workspace, WorkspaceKind};
pub use zoom::{best_fit, SizeHints};
