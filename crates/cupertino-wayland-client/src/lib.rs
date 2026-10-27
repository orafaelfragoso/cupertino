//! Wayland client transport over standard protocols only.
//!
//! A [`Connection`] needs `wl_compositor`, `wl_shm` and `xdg_wm_base`; every other protocol is optional and listed in
//! its [`CapabilitySet`]. It owns application windows ([`Toplevel`]) and their popups ([`Popup`]), hands out shm
//! buffers to draw a frame into ([`BufferLease`]) and presents them in answer to the latest [`WindowConfigure`], at the
//! surface's scale (fractional with `wp_fractional_scale_v1` and `wp_viewporter`, else the preferred or output
//! scale). A compositor without the decoration protocol, or one that asks for it, leaves the window frame to the client
//! ([`Decorations::Client`]). The application runs one loop around [`Connection::wait`] and reacts to the returned
//! [`Event`]s; other threads wake it with a [`Waker`].

mod buffer;
mod capability;
mod clipboard;
mod configure;
mod connection;
mod error;
mod input;
mod output;
mod scale;

pub use buffer::{BufferFormat, BufferLayout, PixelRect};
pub use capability::{Capability, CapabilitySet};
pub use clipboard::ClipboardOffer;
pub use configure::{Decorations, WindowConfigure, WindowStates};
pub use connection::{
    BufferLease, Connection, Event, ExternalRole, FrameCallback, Popup, PopupConfigure,
    PopupPlacement, State, SurfaceId, Toplevel, ToplevelConfig, Waker,
};
pub use error::{Disconnect, Error};
pub use input::{Button, KeyboardEvent, Modifiers, PointerEvent};
pub use output::{OutputId, OutputInfo};
pub use scale::Scale;
pub use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape as CursorShape;
pub use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor, ConstraintAdjustment, Gravity,
};
pub use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
