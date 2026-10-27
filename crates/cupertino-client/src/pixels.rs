//! The pixel layout of the buffers the server reads.

/// Convert premultiplied RGBA bytes (the layout of `tiny_skia::Pixmap`) to premultiplied
/// ARGB8888 (`wl_shm`, bytes B, G, R, A in memory) by swapping the red and blue bytes of every pixel.
pub fn rgba_to_argb8888(rgba: &[u8], argb: &mut [u8]) {
    assert_eq!(rgba.len(), argb.len(), "pixel buffers differ in size");
    let (src, _) = rgba.as_chunks::<4>();
    let (dst, _) = argb.as_chunks_mut::<4>();
    for (s, d) in src.iter().zip(dst) {
        *d = [s[2], s[1], s[0], s[3]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_becomes_argb8888_bytes() {
        let rgba = [10, 20, 30, 40, 1, 2, 3, 255];
        let mut argb = [0; 8];
        rgba_to_argb8888(&rgba, &mut argb);
        assert_eq!(argb, [30, 20, 10, 40, 3, 2, 1, 255]);
    }
}
