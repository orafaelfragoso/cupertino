//! Drawing on a surface of the transport (`cupertino-wayland-client`) with tiny-skia.
//!
//! A [`ShmPresenter`] per surface keeps the last frame's [`tiny_skia::Pixmap`], paces frames by the surface's frame
//! callback and the transport's free buffers, and converts only what a frame changed (its damage and what the buffer
//! missed) into the buffer it presents. [`popup::placement`] places a popup at a rectangle of its parent, and
//! [`clipboard`] reads and writes the text clipboard. [`WaylandWindowSystem`] is CupertinoKit's window system on the
//! transport; [`SurfaceRoles`] lets the application give its windows roles from protocols the window system does not
//! speak. It binds no protocol beyond the transport's standard set.

mod action_grab;
pub mod clipboard;
mod convert;
mod damage;
mod error;
pub mod gpu_path;
pub mod gpu_present;
mod pixels;
pub mod popup;
mod presenter;
pub mod renderer;
mod roles;
mod window_system;

pub use cupertino_wayland_client::PixelRect;
pub use damage::Damage;
pub use error::Error;
pub use gpu_path::GpuRenderers;
pub use pixels::rgba_to_argb8888;
pub use presenter::{copy_rects, Ready, ShmPresenter};
pub use roles::{RoleEvent, SurfaceRoles};
pub use window_system::WaylandWindowSystem;
