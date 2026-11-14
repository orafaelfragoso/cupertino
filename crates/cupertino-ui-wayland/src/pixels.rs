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

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u32, y: u32, width: u32, height: u32) -> PixelRect {
        PixelRect {
            x,
            y,
            width,
            height,
        }
    }

    /// Each pixel of a 100 x 100 area, by how many of `rects` hold it.
    fn coverage(rects: &[PixelRect]) -> Vec<u32> {
        let mut count = vec![0; 100 * 100];
        for r in rects {
            for y in r.y..r.y + r.height {
                for x in r.x..r.x + r.width {
                    count[(y * 100 + x) as usize] += 1;
                }
            }
        }
        count
    }

    #[test]
    fn overlapping_rectangles_become_ones_that_hold_each_pixel_of_their_union_once() {
        let viewport = rect(10, 20, 80, 70);
        // a scroll's damage and the same viewport missed from the frames shown in the other buffers
        assert_eq!(disjoint(&[viewport, viewport, viewport]), vec![viewport]);
        let cases: Vec<Vec<PixelRect>> = vec![
            vec![rect(0, 0, 50, 50), rect(25, 25, 50, 50)],
            vec![
                rect(10, 10, 80, 80),
                rect(30, 0, 10, 100),
                rect(0, 40, 100, 5),
            ],
            vec![rect(20, 20, 10, 10), rect(0, 0, 100, 100)],
            vec![rect(0, 0, 10, 10), rect(10, 0, 10, 10), rect(0, 0, 0, 30)],
        ];
        for rects in cases {
            let union: Vec<u32> = coverage(&rects).into_iter().map(|c| c.min(1)).collect();
            assert_eq!(coverage(&disjoint(&rects)), union, "{rects:?}");
        }
    }

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
        convert_rects(&rgba, stride, &mut argb, stride, &[caret]);
        for y in 0..height {
            for x in 0..width {
                let inside = (300..304).contains(&x) && (20..44).contains(&y);
                let at = y * stride + x * 4;
                assert_eq!(argb[at], if inside { 1 } else { 0xAA }, "({x}, {y})");
            }
        }
    }
}
