//! Dependency-free 2D geometry for the compositor.
//!
//! * [`types`]: `Point`, `Size`, `Rect`, `Insets` (all `f64`).
//! * [`rounded`]: rounded rectangles with circular or continuous ("squircle") corners: cubic path
//!   generation, signed distance functions and anti-aliasing coverage.
//! * [`nine_slice`]: nine-slice scaling math.
//! * [`shadow`]: Gaussian kernel helpers, box-blur approximations and closed-form blurred
//!   rectangles for shadows.
//!
//! The continuous-corner model is approximate: the design draws its window shape in
//! the window server, and only gives an expansion factor of about
//! 1.528 (community-documented, to be verified). Every shape parameter is therefore configurable
//! through [`rounded::CornerStyle`].

pub mod nine_slice;
pub mod rounded;
pub mod shadow;
pub mod types;

pub use nine_slice::{NineSlice, Slice};
pub use rounded::{CornerStyle, Path, PathOptions, PathSeg, Radii, RoundedRect};
pub use types::{Insets, Point, Rect, Size};
