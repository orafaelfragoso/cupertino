//! Toolkit theme generator for the Cupertino desktop.
//!
//! From the design tokens of `cupertino-theme` it produces
//!
//! * GTK 3 and GTK 4 / libadwaita style sheets (light and dark, accent, control sizes, radii, focus
//!   ring, selection) — [`css`];
//! * a KDE colour scheme and a `qt5ct`/`qt6ct` palette (no Kvantum theme involved) — [`qt`];
//! * a fontconfig snippet that maps the system UI font to the open substitute — [`fontconfig`].
//!
//! [`generate::generate`] returns the files in memory ([`generate::GeneratedFile`], paths relative
//! to the output directory); [`generate::write_files`] writes them to a directory and
//! [`install::install`] copies them into explicit XDG target directories — only on request.
//! [`cssck`] is a small CSS tokenizer/validator used to check the output.
//!
//! colours are token values (storage, sRGB); the mapping of
//! tokens to toolkit roles, control radii and the focus ring are approximate (authored).

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod css;
pub mod cssck;
pub mod fontconfig;
pub mod generate;
pub mod install;
pub mod metrics;
pub mod palette;
pub mod qt;

pub use generate::{generate, write_files, GeneratedFile, GeneratorOptions};
pub use metrics::{ControlSizeName, Metrics};
pub use palette::{Accent, Palette};
