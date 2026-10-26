//! XCursor theme builder.
//!
//! The cursors are the procedural vector cursors of [`procedural`], drawn at every requested nominal size with a baked
//! shadow. Output: an XCursor theme (`index.theme`, `cursors/*`, alias symlinks) and optionally a hyprcursor source
//! theme.
//!
//! * [`theme::build_theme`] — the whole pipeline; [`theme::BuildOptions`] — all knobs;
//! * [`xcursor`] — the file format (writer and parser);
//! * [`shadow`] — Gaussian shadow baking;
//! * [`names`] — freedesktop names and aliases; [`procedural`] — the built-in cursors; [`hypr`] — hyprcursor.

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod hypr;
pub mod names;
pub mod procedural;
pub mod shadow;
pub mod theme;
pub mod xcursor;

pub use theme::{build_theme, BuildOptions, ShadowMode};
