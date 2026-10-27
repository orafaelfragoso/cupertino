//! Client toolkit of the Cupertino shell processes (notifications, menu bar, launcher, dock).
//!
//! A [`Client`] connects to the server through the transport (`cupertino-wayland-client`), binds the globals every
//! client needs (layer shell, `cupertino_app_effects_v1`, `xdg_activation_v1`, `zxdg_output_manager_v1`, shm, seat;
//! fractional scale with viewporter when offered) and owns the surfaces: layer surfaces on a chosen
//! output, application windows with server-side decorations ([`Client::toplevel`]), popups of both with a seat
//! grab, the menu-bar and dock roles, and the lock surfaces of a session lock
//! (`ext_session_lock_v1`, [`Client::lock_session`]) with idle notifications ([`Client::watch_idle`]), and the text
//! clipboard ([`Client::set_clipboard_text`], [`Client::clipboard_text`]). Drawing is tiny-skia on a [`tiny_skia::Pixmap`] at the
//! surface's device scale, presented through the transport's shm buffers and paced by frame callbacks. The
//! application runs one loop around [`Client::wait`] and reacts to the returned [`Event`]s;
//! other threads wake it with a [`Waker`].
//!
//! [`Client::connect`] binds no private object; a shell process the server started uses [`Client::connect_shell`], which
//! adds `cupertino_shell_v1` for the menu-bar and dock roles and the lock state. A missing required global is an error at
//! connect, never a degraded mode.
//!
//! The protocols outside the transport's standard set are extensions on its queue: the public ones in
//! `cupertino-protocol-app`, the privileged ones in `cupertino-shell-client`; surfaces draw through
//! `cupertino-ui-wayland`. This API maps onto those crates and goes away once the shell processes use them directly.

mod client;
mod error;
mod event;
mod keyboard;
mod output;
mod scale;

pub use client::{Client, CursorLook, LockState, SurfaceId, TitlebarStyle, ToplevelConfig, Waker};
/// The session's accessibility display options ([`Client::display_options`], [`Event::DisplayOptions`]).
pub use cupertino_defaults::accessibility::DisplayOptions;
pub use cupertino_protocol_app::Backdrop;
pub use cupertino_shell_client::{
    ActiveApp, Anchor, AppCommand, Corner, Dock, DockChange, DockUpdate, DockWindow, Gesture,
    GtkMenu, KeyboardInteractivity, Layer, LayerSurfaceConfig, Margin, MenuBar, MenuBarUpdate,
    OverviewMode, OverviewWindow, OverviewWorkspace, SessionAction, SessionAgent, WindowCommand,
    WindowMenu, WindowState,
};
pub use cupertino_ui_wayland::rgba_to_argb8888;
pub use error::ClientError;
pub use event::{key, ActivationRequest, Button, Event, PointerEvent};
pub use keyboard::Modifiers;
pub use output::{OutputGeometry, OutputId, OutputInfo};
pub use scale::Scale;
pub use wayland_client::Connection;
