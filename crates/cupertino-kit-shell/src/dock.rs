//! Dock role feed and requests (`docs/architecture/shell-host.md#windows-and-applications`).

use cupertino_kit::core::WindowKey;
use cupertino_kit::geometry::Rect;
pub use cupertino_shell_client::{
    AppCommand, Corner, DockChange as Change, DockUpdate as Update, DockWindow as Window,
    DockWindowIcon as WindowIcon, Gesture, WindowIconImage,
};

use crate::context::{self, ShellRequest};
use crate::{ServerWindowId, WorkspaceId};

/// Bring the application named by `app_id` forward.
pub fn activate_app(window: &WindowKey, app_id: &str) {
    context::push_request(ShellRequest::DockActivateApp {
        window: window.clone(),
        app_id: app_id.to_owned(),
    });
}

/// Restore (if minimized), raise and focus one window.
pub fn activate_window(window: &WindowKey, id: ServerWindowId) {
    context::push_request(ShellRequest::DockActivateWindow {
        window: window.clone(),
        id,
    });
}

/// Restore a minimized window.
pub fn restore_window(window: &WindowKey, id: ServerWindowId) {
    context::push_request(ShellRequest::DockRestoreWindow {
        window: window.clone(),
        id,
    });
}

/// Set the tile rectangle a minimized window animates into and out of.
pub fn set_minimize_target(window: &WindowKey, id: ServerWindowId, rect: Rect) {
    context::push_request(ShellRequest::DockSetMinimizeTarget {
        window: window.clone(),
        id,
        rect,
    });
}

/// Force quit an application from its dock tile or Force Quit dialog.
pub fn force_quit_app(window: &WindowKey, app_id: &str) {
    context::push_request(ShellRequest::DockForceQuitApp {
        window: window.clone(),
        app_id: app_id.to_owned(),
    });
}

/// Run an application-level command (`Hide`, `HideOthers`, `ShowAll`, `Quit`).
pub fn app_command(window: &WindowKey, app_id: &str, command: AppCommand) {
    context::push_request(ShellRequest::DockAppCommand {
        window: window.clone(),
        app_id: app_id.to_owned(),
        command,
    });
}

/// Trigger a named compositor action mapped from a hot corner or gesture.
pub fn run_action(window: &WindowKey, action: &str) {
    context::push_request(ShellRequest::DockRunAction {
        window: window.clone(),
        action: action.to_owned(),
    });
}

/// Highlight entry `index` in the application switcher.
pub fn switcher_select(window: &WindowKey, index: u32) {
    context::push_request(ShellRequest::DockSwitcherSelect {
        window: window.clone(),
        index,
    });
}

/// Commit the highlighted application in the application switcher.
pub fn switcher_activate(window: &WindowKey) {
    context::push_request(ShellRequest::DockSwitcherActivate {
        window: window.clone(),
    });
}

/// Cancel the application switcher without switching.
pub fn switcher_cancel(window: &WindowKey) {
    context::push_request(ShellRequest::DockSwitcherCancel {
        window: window.clone(),
    });
}

/// Switch the dock's output to `workspace`.
pub fn switch_workspace(window: &WindowKey, workspace: WorkspaceId) {
    context::push_request(ShellRequest::DockSwitchWorkspace {
        window: window.clone(),
        workspace,
    });
}

/// Append a workspace to the dock's output.
pub fn add_workspace(window: &WindowKey) {
    context::push_request(ShellRequest::DockAddWorkspace {
        window: window.clone(),
    });
}

/// Remove `workspace` from the dock's output (ignored for full-screen or last regular workspace).
pub fn remove_workspace(window: &WindowKey, workspace: WorkspaceId) {
    context::push_request(ShellRequest::DockRemoveWorkspace {
        window: window.clone(),
        workspace,
    });
}

/// Move `workspace` to 0-based `index` among the output's workspaces.
pub fn move_workspace(window: &WindowKey, workspace: WorkspaceId, index: u32) {
    context::push_request(ShellRequest::DockMoveWorkspace {
        window: window.clone(),
        workspace,
        index,
    });
}
