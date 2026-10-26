//! Stage model: window groups, a centre stage, a strip of recent groups, spring swap animation,
//! add-to-stage by drag, strip hide/peek and desktop hiding. Pure logic (no compositor dependency).
//!
//! The strip layout settings and their default values are as designed and the swap spring is as designed; paddings and dwell times are approximate or unknown and
//! configurable through [`StageConfig`].

pub mod config;
pub mod stage;

pub use config::{ClickDesktop, Grouping, StageConfig};
pub use stage::{DropTarget, GroupId, Placement, Stage, StageEvent, StripSlot};
