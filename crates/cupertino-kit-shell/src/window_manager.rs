//! Window manager role feed and requests (`docs/architecture/shell-host.md#windows-and-applications`).

use cupertino_kit::geometry::Rect;

use crate::context::{self, ShellRequest};
use crate::outputs::OutputId;
use crate::ServerWindowId;

/// One arranged window reported by the server for an output.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: ServerWindowId,
    pub app_id: String,
    /// Window frame in global logical coordinates.
    pub frame: Rect,
    /// Whether this window is the key window of the session.
    pub active: bool,
}

/// Where the window manager places an arranged window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Shown on the stage at its own frame.
    Stage,
    /// Hidden off-stage (or shown only through a claimed hole on the strip surface).
    Hidden,
}

/// One atomic update from the server for an output's window manager.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    /// Usable area in global logical coordinates.
    pub usable_area: Option<Rect>,
    /// Arranged windows of the output.
    pub windows: Vec<Window>,
    /// Server asked to toggle the window stage on this output.
    pub toggle_stage: bool,
}

/// Queue a placement for `window` on `output` (takes effect on [`commit`]).
pub fn place(output: OutputId, window: ServerWindowId, placement: Placement) {
    context::push_request(ShellRequest::WindowManagerPlace {
        output,
        window,
        placement,
    });
}

/// Commit pending placements on `output` (a window change inside a transaction bracket).
pub fn commit(output: OutputId) {
    context::push_request(ShellRequest::WindowManagerCommit { output });
}

/// Activate `window` on `output` (a window change inside a transaction bracket).
pub fn activate_window(output: OutputId, window: ServerWindowId) {
    context::push_request(ShellRequest::WindowManagerActivateWindow { output, window });
}
