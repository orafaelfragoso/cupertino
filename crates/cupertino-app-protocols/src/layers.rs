//! The limits of `cupertino_layers_v1` that the server checks and clients check before they send, as
//! `protocols/cupertino-layers-v1.xml` documents them. Arguments of type `fixed` are compared as the wire carries
//! them, in 256ths of a unit.

/// Longest timed animation or animation through samples, in milliseconds: a longer one is `invalid_curve`.
pub const MAX_DURATION_MS: u32 = 10_000;
/// Shortest and longest spring response, in seconds: one outside is `invalid_curve`.
pub const SPRING_RESPONSE: (f64, f64) = (0.01, 10.0);
/// Lowest and highest spring damping ratio: one outside is `invalid_curve`.
pub const SPRING_DAMPING: (f64, f64) = (0.05, 4.0);
/// Lowest and highest magnification of a layer's glass lens: one outside is `invalid_value`.
pub const LENS: (f64, f64) = (1.0, 4.0);
/// Widest blend between the joined shapes of a glass group, in logical points: a wider one is `invalid_value`.
pub const MAX_GROUP_SPACING: f64 = 64.0;
/// Widest blur of a layer's content, the standard deviation of its Gaussian in the layer's logical points: a wider
/// one is `invalid_value`.
pub const MAX_CONTENT_BLUR: f64 = 64.0;
/// Widest edge along a layer's glass, in logical points: a wider one is `invalid_value`.
pub const MAX_GLASS_EDGE: f64 = 4.0;
/// Fewest samples of one animation through samples: fewer are `invalid_curve`.
pub const MIN_SAMPLES: usize = 2;
/// Most samples of one animation through samples, as many as one request of at most 4096 bytes carries with room to
/// spare: more are `invalid_curve`.
pub const MAX_SAMPLES: usize = 1000;
