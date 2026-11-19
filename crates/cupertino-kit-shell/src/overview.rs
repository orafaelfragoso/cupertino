//! Overview role feed and requests (`cupertino_overview_v1`, `docs/architecture/shell-host.md#windows-and-applications`).

use cupertino_kit::geometry::Rect;

use crate::context::{self, ShellRequest};
use crate::outputs::OutputId;
use crate::{ServerWindowId, WorkspaceId};

/// Mode of an overview toggle request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// All windows of the current workspace, with the workspaces bar.
    All,
    /// Windows of one application.
    App,
}

/// One window in an output's overview set.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: ServerWindowId,
    pub app_id: String,
    pub title: String,
    /// Resting frame in global logical coordinates (without claim of this connection).
    pub frame: Rect,
    pub minimized: bool,
    pub active: bool,
}

/// One workspace of an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub current: bool,
    pub full_screen: bool,
}

/// Toggle request sent by the server when the user triggers overview via shortcut, hot corner, or action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toggle {
    pub mode: Mode,
    pub app_id: String,
}

/// One atomic update group from `cupertino_overview_v1`.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub area: Option<Rect>,
    pub windows: Vec<Window>,
    pub workspaces: Vec<Workspace>,
    pub toggle: Option<Toggle>,
}

/// Pick and activate `window` on `output` (window change inside a transaction bracket).
pub fn activate_window(output: OutputId, window: ServerWindowId) {
    context::push_request(ShellRequest::OverviewActivateWindow { output, window });
}

/// Switch `output` to `workspace` (window change inside a transaction bracket).
pub fn switch_workspace(output: OutputId, workspace: WorkspaceId) {
    context::push_request(ShellRequest::OverviewSwitchWorkspace { output, workspace });
}

/// Append a new regular workspace to `output`.
pub fn add_workspace(output: OutputId) {
    context::push_request(ShellRequest::OverviewAddWorkspace { output });
}

/// Remove `workspace` from `output` (ignored for full-screen or last regular workspace).
pub fn remove_workspace(output: OutputId, workspace: WorkspaceId) {
    context::push_request(ShellRequest::OverviewRemoveWorkspace { output, workspace });
}

/// Move `workspace` to 0-based `index` on `output`.
pub fn move_workspace(output: OutputId, workspace: WorkspaceId, index: u32) {
    context::push_request(ShellRequest::OverviewMoveWorkspace {
        output,
        workspace,
        index,
    });
}

/// Move `window` to `workspace` on `output` (window change inside a transaction bracket).
pub fn move_window(output: OutputId, window: ServerWindowId, workspace: WorkspaceId) {
    context::push_request(ShellRequest::OverviewMoveWindow {
        output,
        window,
        workspace,
    });
}
