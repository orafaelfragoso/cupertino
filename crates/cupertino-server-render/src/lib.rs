//! Rendering primitives of the compositor (D25): scene lowering, shaders, window chrome,
//! cursors, effects, frame opacity detection, lingering popups, and snapshot lifetime tracking.
//! Depends only on `cupertino-server-backend` among the server crates.

pub mod config;
pub mod cursor;
pub mod decor;
pub mod decor_access;
pub mod drag_return;
pub mod frame;
pub mod frame_fill;
pub mod fx;
pub mod popup_linger;
pub mod render;
pub mod toolkit_programs;
