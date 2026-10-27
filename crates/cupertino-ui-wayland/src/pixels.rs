//! The pixel layout of the buffers the server reads.

use cupertino_wayland_client::PixelRect;

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

/// [`rgba_to_argb8888`] of only `rects` (inside both buffers), rows `stride` bytes apart.
pub(crate) fn convert_rects(rgba: &[u8], argb: &mut [u8], stride: usize, rects: &[PixelRect]) {
    for r in rects {
        let (start, len) = (r.x as usize * 4, r.width as usize * 4);
        for row in r.y..r.y + r.height {
            let at = row as usize * stride + start;
            rgba_to_argb8888(&rgba[at..at + len], &mut argb[at..at + len]);
        }
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

    #[test]
    fn only_the_rectangles_are_converted() {
        let (width, height, stride) = (1920, 1080, 1920 * 4);
        let rgba = vec![1u8; stride * height];
        let mut argb = vec![0xAAu8; stride * height];
        // a caret
        let caret = PixelRect {
            x: 300,
            y: 20,
            width: 4,
            height: 24,
        };
        convert_rects(&rgba, &mut argb, stride, &[caret]);
        for y in 0..height {
            for x in 0..width {
                let inside = (300..304).contains(&x) && (20..44).contains(&y);
                let at = y * stride + x * 4;
                assert_eq!(argb[at], if inside { 1 } else { 0xAA }, "({x}, {y})");
            }
        }
    }
}
