//! Input primitives of the compositor (D25): key routing, keyboard settings and layouts,
//! accessibility key filters, in-process input-method bridge, accent-picker model, hung-client
//! ping liveness, pointer constraints, shortcut inhibit, and global shortcuts helpers.
//! Depends on no other server crate.

pub mod events;
pub mod global_shortcuts;
pub mod hung;
pub mod input;
pub mod input_method;
pub mod kbd;
pub mod kbd_filters;
pub mod kbd_ui;
pub mod localed;
pub mod overlay_types;
pub mod pointer_constraints;
pub mod shortcut_inhibit;
