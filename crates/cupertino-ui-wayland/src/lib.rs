//! Drawing on a surface of the transport (`cupertino-wayland-client`) with tiny-skia.
//!
//! A [`ShmPresenter`] per surface keeps the last frame's [`tiny_skia::Pixmap`], paces frames by the surface's frame
//! callback and the transport's free buffers, and converts only what a frame changed (its damage and what the buffer
//! missed) into the buffer it presents. [`popup::placement`] places a popup at a rectangle of its parent, and
//! [`clipboard`] reads and writes the text clipboard. It binds no protocol beyond the transport's standard set.

pub mod clipboard;
mod error;
mod pixels;
pub mod popup;
mod presenter;

pub use error::Error;
pub use pixels::rgba_to_argb8888;
pub use presenter::{Ready, ShmPresenter};
