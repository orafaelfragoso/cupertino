//! Menu bar role feed and requests (`docs/architecture/shell-host.md#windows-and-applications`).

use cupertino_kit::core::WindowKey;
pub use cupertino_shell_client::{
    ActiveApp, ActiveClient, AppWindow, BoundAction, EditAction, GtkMenu, KeyClass,
    MenuBarUpdate as Update, PrimaryModifier, SessionAction, Shortcuts, WindowCommand, WindowMenu,
    WindowState, MAX_BACKDROP_BANDS,
};

use crate::context::{self, ShellRequest};
use crate::ServerWindowId;

/// Tell the server whether a menu of the menu bar `window` is currently open.
pub fn set_menu_open(window: &WindowKey, open: bool) {
    context::push_request(ShellRequest::MenuBarSetOpen {
        window: window.clone(),
        open,
    });
}

/// Ask the server to run `command` on the active window.
pub fn window_command(window: &WindowKey, command: WindowCommand) {
    context::push_request(ShellRequest::MenuBarWindowCommand {
        window: window.clone(),
        command,
    });
}

/// Activate one window of the active application from the Window menu.
pub fn activate_window(window: &WindowKey, id: ServerWindowId) {
    context::push_request(ShellRequest::MenuBarActivateWindow {
        window: window.clone(),
        id,
    });
}

/// Move the active window to 1-based workspace number `workspace`.
pub fn move_to_workspace(window: &WindowKey, workspace: u32) {
    context::push_request(ShellRequest::MenuBarMoveToWorkspace {
        window: window.clone(),
        workspace,
    });
}

/// Ask the server to forward `action` to the session agent.
pub fn session_action(window: &WindowKey, action: SessionAction) {
    context::push_request(ShellRequest::MenuBarSessionAction {
        window: window.clone(),
        action,
    });
}

/// Send an edit menu action (`Cut`, `Copy`, `Paste`, `SelectAll`, `Undo`, `Redo`) to the active window.
pub fn edit(window: &WindowKey, action: EditAction) {
    context::push_request(ShellRequest::MenuBarEdit {
        window: window.clone(),
        action,
    });
}

/// Force quit the active application when its app id matches `app_id`.
pub fn force_quit_app(window: &WindowKey, app_id: &str) {
    context::push_request(ShellRequest::MenuBarForceQuitApp {
        window: window.clone(),
        app_id: app_id.to_owned(),
    });
}
