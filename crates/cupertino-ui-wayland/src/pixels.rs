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

/// [`rgba_to_argb8888`] of only `rects` (inside both buffers), with rows `rgba_stride` and `argb_stride` bytes apart.
pub(crate) fn convert_rects(
    rgba: &[u8],
    rgba_stride: usize,
    argb: &mut [u8],
    argb_stride: usize,
    rects: &[PixelRect],
) {
    for r in rects {
        let (start, len) = (r.x as usize * 4, r.width as usize * 4);
        for row in r.y as usize..(r.y + r.height) as usize {
            let (from, to) = (row * rgba_stride + start, row * argb_stride + start);
            rgba_to_argb8888(&rgba[from..from + len], &mut argb[to..to + len]);
        }
    }
}

/// The bytes of `rects` (inside both buffers) copied as they are, for a buffer in the renderer's own byte order
/// (`abgr8888`, bytes R, G, B, A in memory), with rows `from_stride` and `to_stride` bytes apart.
pub(crate) fn copy_rects(
    from: &[u8],
    from_stride: usize,
    to: &mut [u8],
    to_stride: usize,
    rects: &[PixelRect],
) {
    for r in rects {
        let (start, len) = (r.x as usize * 4, r.width as usize * 4);
        for row in r.y as usize..(r.y + r.height) as usize {
            let (a, b) = (row * from_stride + start, row * to_stride + start);
            to[b..b + len].copy_from_slice(&from[a..a + len]);
        }
    }
}

/// The pixels of `rects` as rectangles that do not overlap, each pixel of their union in exactly one: a frame's
/// damage and what its buffer missed of the frames shown since often cover the same pixels (a scroll damages the whole
/// moved viewport every frame), and each pixel needs converting once.
pub(crate) fn disjoint(rects: &[PixelRect]) -> Vec<PixelRect> {
    let mut out: Vec<PixelRect> = Vec::with_capacity(rects.len());
    for r in rects.iter().filter(|r| r.width > 0 && r.height > 0) {
        let mut pieces = vec![*r];
        for taken in &out {
            pieces = pieces.iter().flat_map(|p| subtract(p, taken)).collect();
            if pieces.is_empty() {
                break;
            }
        }
        out.extend(pieces);
    }
    out
}

/// What of `r` lies outside `cut`: at most four rectangles, the bands above and below it and the pieces beside it.
fn subtract(r: &PixelRect, cut: &PixelRect) -> Vec<PixelRect> {
    let (rx1, ry1) = (r.x + r.width, r.y + r.height);
    let (cx1, cy1) = (cut.x + cut.width, cut.y + cut.height);
    if cut.x >= rx1 || cx1 <= r.x || cut.y >= ry1 || cy1 <= r.y {
        return vec![*r];
    }
    let piece = |x0: u32, y0: u32, x1: u32, y1: u32| PixelRect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    };
    let (y0, y1) = (r.y.max(cut.y), ry1.min(cy1));
    let mut out = Vec::with_capacity(4);
    if r.y < y0 {
        out.push(piece(r.x, r.y, rx1, y0));
    }
    if y1 < ry1 {
        out.push(piece(r.x, y1, rx1, ry1));
    }
    if r.x < cut.x {
        out.push(piece(r.x, y0, cut.x, y1));
    }
    if cx1 < rx1 {
        out.push(piece(cx1, y0, rx1, y1));
    }
    out
}
