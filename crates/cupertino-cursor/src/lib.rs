//! XCursor theme builder.
//!
//! Inputs: a directory of per-cursor vector PDFs (`<name>/cursor.pdf`, optionally `info.plist`) plus
//! metadata (hotspots, shadow parameters, animation) — or no input at all, in which case the
//! procedural vector cursors of [`procedural`] are used. Output: an XCursor theme
//! (`index.theme`, `cursors/*, alias symlinks) and optionally a hyprcursor source theme.
//!
//! * [`theme::build_theme`] — the whole pipeline; [`theme::BuildOptions`] — all knobs;
//! * [`xcursor`] — the file format (writer and parser);
//! * [`raster`] — poppler (`pdftocairo`, `pdfinfo`) rasterisation;
//! * [`shadow`] — Gaussian shadow baking; [`meta`], [`plist`] — metadata;
//! * [`names`] — freedesktop name mapping; [`procedural`] — the built-in cursors; [`hypr`] — hyprcursor.
//!
//! Output derived from a reference cursor set is proprietary and for local use only; the default
//! output directory is the git-ignored.

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod hypr;
pub mod meta;
pub mod names;
pub mod plist;
pub mod procedural;
pub mod raster;
pub mod shadow;
pub mod theme;
pub mod xcursor;

pub use theme::{build_theme, BuildOptions, ShadowMode, Source};
