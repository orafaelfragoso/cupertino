//! Dithering keyed to the pixel position.
//!
//! Before a value is stored in 8 bits, the offset of its pixel is added to it: `v + t(x, y) / 255`, with `t` from the
//! 8 × 8 ordered matrix, `(m + 0.5) / 64 − 0.5` for the matrix entry `m` (0 to 63). The offset lies inside half a
//! step, so a value 8 bits hold exactly is stored unchanged (flat colours, opaque alpha, nothing at all) and only
//! values between two steps are spread over the neighbouring pixels. The same pixel always gets the same offset:
//! nothing is random, damage repaints match and goldens are stable.
//!
//! Which channels: the colour of a straight-alpha output (blur, material, glass), the alpha of a one-colour coverage
//! output (a shadow), and the colour of a gradient (its premultiplied channels, held at its alpha; the alpha is
//! rounded). The position is the framebuffer pixel, as `gl_FragCoord` counts it, or the pixel of the window or buffer
//! a renderer draws into. [`GLSL`] is the shader implementation, tested against [`offset`].

/// Whether renderers dither by default: on since the look freeze, which re-recorded the goldens with it.
pub const BY_DEFAULT: bool = true;

/// The shader implementation: `dither_offset`, `dither_rgb` and `dither_alpha` over `gl_FragCoord.xy`.
pub const GLSL: &str = include_str!("dither.glsl");

/// The line a shader holds where [`with_dither`] puts the implementation.
pub const MARKER: &str = "//_DITHER_";

/// Matrix size.
pub const SIZE: u32 = 8;

/// The ordered 2 × 2 matrix `[[0, 2], [3, 1]]` at the bits of `x` and `y`.
fn pair(x: u32, y: u32) -> u32 {
    2 * (x ^ y) + y
}

/// The 8 × 8 ordered matrix entry, 0 to 63, of pixel `(x, y)`.
pub fn matrix(x: u32, y: u32) -> u32 {
    let bit = |v: u32, k: u32| (v >> k) & 1;
    16 * pair(bit(x, 0), bit(y, 0)) + 4 * pair(bit(x, 1), bit(y, 1)) + pair(bit(x, 2), bit(y, 2))
}

/// The offset of pixel `(x, y)`, in 8-bit steps: inside (-0.5, 0.5).
pub fn offset(x: u32, y: u32) -> f32 {
    (matrix(x, y) as f32 + 0.5) / 64.0 - 0.5
}

/// `v` (0 to 1) with the offset of pixel `(x, y)` added, before it is stored.
pub fn dither(v: f32, x: u32, y: u32) -> f32 {
    v + offset(x, y) / 255.0
}

/// `v` stored in 8 bits: held to 0 to 1, rounded to nearest.
pub fn quantize(v: f32) -> u8 {
    let x = v.clamp(0.0, 1.0) * 255.0;
    // to nearest, a half away from zero, as `f32::round`; the fraction `x - whole` is exact, and no library call is
    // made where the target has no rounding instruction
    let whole = x as u32;
    (whole + u32::from(x - whole as f32 >= 0.5)) as u8
}

/// `v` dithered at pixel `(x, y)` and stored in 8 bits.
pub fn store(v: f32, x: u32, y: u32) -> u8 {
    quantize(dither(v, x, y))
}

/// `source` with the shader implementation in place of its [`MARKER`] line and `CUPERTINO_DITHER` defined when `on`;
/// unchanged when not, so a shader that is not dithered compiles from the very text it had.
pub fn with_dither(source: &str, on: bool) -> String {
    if !on {
        return source.to_owned();
    }
    source.replace(MARKER, &format!("#define CUPERTINO_DITHER 1\n{GLSL}"))
}
