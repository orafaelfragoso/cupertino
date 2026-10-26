//! Animation primitives for a Rust/Smithay compositor, modelled on the design.
//!
//! Each module documents which parts are as designed against the design binaries and which are approximate. See.

pub mod animator;
pub mod momentum;
pub mod rubber_band;
pub mod spring;
pub mod timing;

pub use animator::{Animatable, AnimatedValue, Curve};
pub use momentum::{Momentum, MomentumModel, QuarticMomentum, QuarticMomentum2D};
pub use rubber_band::{ElasticMotion, RubberBand, RubberBandFilter, RubberBandFilterConfig};
pub use spring::{Spring, SpringParams};
pub use timing::{cosine_ease, CubicBezier, TimingPreset};
