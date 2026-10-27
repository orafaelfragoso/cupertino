//! Client toolkit of the Cupertino shell processes (notifications, menu bar, launcher, dock).
//!
//! A [`Client`] connects to the server, binds the globals every client needs (layer shell,
//! `cupertino_app_effects_v1`, `xdg_activation_v1`, `zxdg_output_manager_v1`, shm, seat; fractional
//! scale with viewporter when offered) and owns the surfaces: layer surfaces on a chosen
//! output, application windows with server-side decorations ([`Client::toplevel`]), popups of both with a seat
//! grab, the menu-bar and dock roles, and the lock surfaces of a session lock
//! (`ext_session_lock_v1`, [`Client::lock_session`]) with idle notifications ([`Client::watch_idle`]), and the text
//! clipboard ([`Client::set_clipboard_text`], [`Client::clipboard_text`]). Drawing is tiny-skia on a [`tiny_skia::Pixmap`] at the
//! surface's device scale, double-buffered through shm and paced by frame callbacks. The
//! application runs one loop around [`Client::wait`] and reacts to the returned [`Event`]s;
//! other threads wake it with a [`Waker`].
//!
//! [`Client::connect`] binds no private object; a shell process the server started uses [`Client::connect_shell`], which
//! adds `cupertino_shell_v1` for the menu-bar and dock roles and the lock state. A missing required global is an error at
//! connect, never a degraded mode.

mod backdrop;
mod client;
mod clipboard;
mod dock;
mod error;
mod event;
mod keyboard;
mod menu_bar;
mod output;
mod popup;
mod scale;
mod shm;

pub use backdrop::Backdrop;
pub use client::{
    Client, CursorLook, LayerSurfaceConfig, LockState, Margin, SurfaceId, TitlebarStyle,
    ToplevelConfig, Waker,
};
/// The session's accessibility display options ([`Client::display_options`], [`Event::DisplayOptions`]).
pub use cupertino_defaults::accessibility::DisplayOptions;
pub use dock::{
    AppCommand, Corner, Dock, DockChange, DockUpdate, DockWindow, Gesture, OverviewMode,
    OverviewWindow, OverviewWorkspace,
};
pub use error::ClientError;
pub use event::{key, ActivationRequest, Button, Event, PointerEvent};
pub use keyboard::Modifiers;
pub use menu_bar::{
    ActiveApp, GtkMenu, MenuBar, MenuBarUpdate, WindowCommand, WindowMenu, WindowState,
};
pub use output::{OutputGeometry, OutputId, OutputInfo};
pub use scale::Scale;
pub use shm::{rgba_to_argb8888, BufferLayout};
pub use wayland_client::Connection;
pub use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
pub use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};

pub(crate) use client::State;
