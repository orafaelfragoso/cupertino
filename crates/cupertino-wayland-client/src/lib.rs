//! Wayland client transport over standard protocols only.
//!
//! A [`Connection`] needs `wl_compositor`, `wl_shm` and `xdg_wm_base`; every other protocol is optional and listed in
//! its [`CapabilitySet`]. It owns application windows ([`Toplevel`]) and their popups ([`Popup`]), hands out shm
//! buffers to draw a frame into ([`BufferLease`]) and presents them in answer to the latest [`WindowConfigure`], at the
//! surface's scale (fractional with `wp_fractional_scale_v1` and `wp_viewporter`, else the preferred or output
//! scale), and talks to input methods for the focused text field ([`Connection::set_text_input`]). A compositor without
//! the decoration protocol, or one that asks for it, leaves the window frame to the client
//! ([`Decorations::Client`]). The application runs one loop around [`Connection::wait`] and reacts to the returned
//! [`Event`]s; other threads wake it with a [`Waker`].

mod buffer;
mod capability;
mod clipboard;
mod compose;
mod configure;
mod connection;
mod cursor;
mod error;
mod input;
mod output;
mod presentation;
mod scale;
mod text_input;

pub use buffer::{BufferFormat, BufferLayout, PixelRect};
pub use capability::{Capability, CapabilitySet, Extension};
pub use clipboard::{
    ClipboardOffer, DragEvent, DragIcon, DragOffer, DragSource, DragSourceEvent, ReadId,
};
pub use configure::{Decorations, WindowConfigure, WindowStates, WmCapabilities};
pub use connection::gpu::{
    DmabufAttributes, DmabufFeedback, DmabufPlaneRef, FrameSize, GpuBufferId, GpuFrame, TimelineId,
};
pub use connection::{
    BufferLease, Connection, Event, ExtensionEvent, ExternalRole, FrameCallback, HeldPress, Popup,
    PopupConfigure, PopupPlacement, PressDevice, State, SurfaceId, TokenRequest, Toplevel,
    ToplevelConfig, Waker,
};
pub use cursor::CursorTheme;
pub use error::{Disconnect, Error};
pub use input::{key, Button, KeyboardEvent, Modifiers, PointerEvent, TouchEvent};
pub use output::{OutputGeometry, OutputId, OutputInfo, OutputTransform};
pub use presentation::Presented;
pub use scale::Scale;
pub use text_input::{
    Preedit, SurroundingText, TextInputDone, TextInputEvent, TextInputState, MAX_SURROUNDING,
};
pub use wayland_client::protocol::wl_data_device_manager::DndAction as DragActions;
pub use wayland_client::protocol::wl_pointer::AxisSource as ScrollSource;
pub use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape as CursorShape;
pub use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::Kind as PresentedKind;
pub use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    ChangeCause, ContentHint, ContentPurpose,
};
pub use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor, ConstraintAdjustment, Gravity,
};
pub use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
