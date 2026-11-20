//! Outputs observed by the component, identified by the `wl_output` registry name (`OutputId`).

use cupertino_kit::geometry::Rect;
use serde::{Deserialize, Serialize};

use crate::context;

/// The registry name of an output, shared by every component while the output is present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OutputId(u32);

impl OutputId {
    /// The output of the `wl_output` registry name `raw` (internal to the crate).
    pub(crate) const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Raw registry global name (internal to the crate).
    pub(crate) const fn raw(self) -> u32 {
        self.0
    }
}

/// Orientation of an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Transform {
    #[default]
    Normal,
    Rotated90,
    Rotated180,
    Rotated270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

/// Description of one connected output in global logical coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub id: OutputId,
    /// The connector name, such as `DP-1`.
    pub name: Option<String>,
    pub description: Option<String>,
    /// Position and size in logical points.
    pub geometry: Option<Rect>,
    /// Device pixels per logical point.
    pub scale: f64,
    pub transform: Transform,
}

/// Every output currently known to the calling thread's component.
pub fn all() -> Vec<Output> {
    context::with_ctx(|ctx| ctx.outputs.clone()).unwrap_or_default()
}

/// Look up one output by its `OutputId`.
pub fn get(id: OutputId) -> Option<Output> {
    context::with_ctx(|ctx| ctx.outputs.iter().find(|o| o.id == id).cloned()).flatten()
}
