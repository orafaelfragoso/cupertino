//! The integration tests of `cupertino-input`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod cursor;
mod edge;
#[path = "geom-keysym.rs"]
mod geom_keysym;
mod gesture;
mod hotcorner;
#[path = "mouse-accel.rs"]
mod mouse_accel;
mod pointer;
#[path = "scroll-accel.rs"]
mod scroll_accel;
#[path = "scroll-count.rs"]
mod scroll_count;
mod shortcuts;
mod table;
#[path = "top-zone.rs"]
mod top_zone;
mod touchpad;
