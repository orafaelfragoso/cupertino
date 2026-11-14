//! The one definition of the drawing maths. Every renderer (the toolkit's CPU renderer, the compositor's passes and
//! any later GPU renderer) and every shader computes these, and is tested against the committed [`vectors`].
//!
//! * [`coverage`]: distance and coverage of rectangles and rounded rectangles with circular or continuous corners,
//!   strokes and nested clips.
//! * [`blur`]: the blur plan (a pyramid of 2× halvings and a Gaussian), its kernel, edge rule, premultiplied alpha
//!   and intermediate precision, with an implementation in `f32` for tests.
//! * [`shadow`]: drop and inner shadows of a rounded rectangle in closed form.
//! * [`gradient`]: up to eight stops, interpolated in straight encoded sRGB, premultiplied after.
//! * [`dither`]: dithering keyed to the pixel position, and its shader implementation.
//! * [`placement`]: where an image drawn one source pixel per device pixel lands.
//!
//! Everything is `f32`, as the shaders compute it.

pub mod blur;
pub mod coverage;
pub mod dither;
pub mod gradient;
pub mod placement;
pub mod shadow;
pub mod vectors;
