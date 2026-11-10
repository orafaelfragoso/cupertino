//! The privileged protocols of the shell processes as extensions on the transport's queue (`cupertino-wayland-client`).
//!
//! [`LayerShell`] creates layer surfaces (`zwlr_layer_shell_v1`), [`SessionLockManager`] locks the session and covers
//! the outputs with lock surfaces (`ext_session_lock_v1`), [`IdleNotifier`] reports an idle seat
//! (`ext_idle_notify_v1`), and [`Shell`] (`cupertino_shell_v1`) observes the session and gives layer surfaces the
//! menu-bar and dock roles. What their objects receive arrives as a [`ShellEvent`] inside the transport's
//! `Event::Extension`, in order with the transport's own events; the session's appearance and display options arrive
//! as the [`cupertino_protocol_app::AppEvent`] an application's observer reports.

mod control;
mod dock;
pub use control::{ControlAuthority, ControlEvent, ControlSession, DesktopControl};
mod error;
mod menu_bar;
mod roles;
mod session;
pub mod test_clock;

pub use dock::{
    AppCommand, Corner, Dock, DockChange, DockUpdate, DockWindow, DockWindowIcon, Gesture,
    OverviewMode, OverviewWindow, OverviewWorkspace, WindowIconImage,
};
pub use error::Error;
pub use menu_bar::{
    ActiveApp, AppWindow, BoundAction, EditAction, GtkMenu, MenuBar, MenuBarUpdate,
    PrimaryModifier, SessionAction, SessionAgent, Shortcuts, WindowCommand, WindowMenu,
    WindowState, MAX_SPREAD_BANDS,
};
pub use roles::{
    AppActivation, IdleNotifier, IdleWatch, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    Session, SessionLock, SessionLockManager, Shell, ShellEvent,
};
pub use session::{SessionChange, SessionState};
pub use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
pub use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
