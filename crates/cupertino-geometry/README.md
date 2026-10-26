# cupertino-geometry

Dependency-free 2D geometry for the Cupertino compositor, with no GPU crates. All values are `f64` and y grows downwards.

## Public API

| Module | Contents |
|---|---|
| `types` | `Point`, `Size`, `Rect`, `Insets` with the usual set operations |
| `rounded` | `RoundedRect` with per-corner `Radii` and `CornerStyle::{Circular, Continuous}`: cubic `Path`, reference `sdf`, shader-identical `sdf_approx`, `coverage` for anti-aliasing |
| `nine_slice` | `NineSlice`: nine (or three/six) slices with source/destination rects and UVs |
| `shadow` | Gaussian weights, box-blur sizes/kernels, bilinear tap merging, closed-form blurred rectangle, `ShadowSpec` |

Methods of `RoundedRect`:

| Method | Role |
|---|---|
| `RoundedRect::path | Closed path, 4 cubics per continuous corner by default (1 per circular corner) |
| `RoundedRect::sdf | Reference signed distance, negative inside. Exact for circular corners |
| `RoundedRect::sdf_approx | Closed form `(G − 1)/|∇G|` for the superellipse norm `G`, the same formula the glass shaders use |
| `RoundedRect::coverage(p, pixel) | `clamp(0.5 − sdf/pixel, 0, 1) |

## How it works

The continuous corner is approximate; the shape of the real curve is unknown. The corner
occupies a square of side `expansion · radius`. The default is 1.528, community documented and to be verified, and
`CornerStyle::Continuous { expansion } configures it.

Inside that square the boundary is the superellipse `u^n + v^n = 1`. `n` is chosen so the curve passes through the same
diagonal point as a circular arc of the nominal radius. So `expansion = 1` gives `n = 2`, an exact circle, and the default
gives `n ≈ 3.26`. For `n > 2` curvature is zero where the corner meets the straight edge.

The maximum deviation of `path from the design distance field at radius 100 with 4 segments is 0.004 px. Run
`cargo run --release -p cupertino-geometry --example accuracy` to reproduce it.

## Tests

```
cargo test -p cupertino-geometry
cargo bench -p cupertino-geometry
```
