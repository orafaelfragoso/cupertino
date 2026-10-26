# cupertino-animation

Dependency-free animation primitives for the Cupertino compositor. Time is always an absolute timestamp in seconds, so a
curve samples identically at any refresh rate.

## Public API

| Module | Contents |
|---|---|
| `spring` | `SpringParams`, `Spring`: damped spring with perceptual duration/bounce constructors, settling duration |
| `timing` | `CubicBezier` with the five built-in timing functions, `TimingPreset` (table: five béziers + spring mass 1 / stiffness 100 / damping 10), `cosine_ease` |
| `momentum` | `QuarticMomentum` (default model, `C = 1000`, `x0 + sign·C·(T⁴−(T−t)⁴)`), `QuarticMomentum2D`, `MomentumModel` selector, `Momentum` (per-millisecond exponential alternative) |
| `rubber_band` | `RubberBand` and free functions: elastic overscroll `d·(1 − 1/(x·c/d + 1))`, inverse, slope, clamp helper. Coefficient `c` is configurable, default 0.55 |
| `rubber_band` | `ElasticMotion` (`(x0 + v·t·0.31)·exp(−20t/1.6)`), `RubberBandFilterConfig` / `RubberBandFilter` (stiffness 20, direction lock 1.0, min delta 10) |
| `animator` | `AnimatedValue<T>`, `Curve`, `Animatable`: retargetable animations with C¹ continuity, gesture tracking and release |

The physics are written up.

```rust
use cupertino_animation::{AnimatedValue, Curve, RubberBand, SpringParams};

let mut x = AnimatedValue::new(0.0f64);
x.animate_to(100.0, Curve::Spring(SpringParams::from_perceptual(0.35, 0.0)), 0.0);
let _ = x.value(0.1);

let band = RubberBand::new(800.0);
let shown = band.map(120.0);
```

## Tests

```
cargo test -p cupertino-animation
cargo bench -p cupertino-animation --bench throughput
```
