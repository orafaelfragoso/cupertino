//! Animation primitives for a Rust/Smithay compositor, modelled on the design.
//!
//! Each module documents which parts are as designed against the design binaries and which are approximate. See.

pub mod animator;
pub mod layer;
pub mod momentum;
pub mod rubber_band;
pub mod scroll;
pub mod spring;
pub mod timing;
pub mod velocity;

pub use animator::{Animatable, AnimatedValue, Curve};
pub use layer::{LayerCurve, LayerProperty};
pub use momentum::{Momentum, MomentumModel, QuarticMomentum, QuarticMomentum2D};
pub use rubber_band::{ElasticMotion, RubberBand, RubberBandFilter, RubberBandFilterConfig};
pub use scroll::{KineticScroller, MomentumKind, ScrollConfig};
pub use spring::{Spring, SpringParams};
pub use timing::{cosine_ease, CubicBezier, TimingPreset};
pub use velocity::VelocityTracker;
