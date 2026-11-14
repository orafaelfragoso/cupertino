//! The layers the CPU renderer keeps between frames.
//!
//! A kept scroll layer ([`crate::Layer::Scroll`]) is an image of its content in the content's own pixels, the size of
//! what its node's clip shows: content pixel `(x, y)` is the window pixel the content shows at with no scroll, and
//! shows at window pixel `(x - dx, y - dy)` for a drawn offset of `(dx, dy)` device pixels. The image holds the rows
//! `top..top + height`, row `y` in image row `y mod height`, so a vertical scroll moves no pixel: the rows that stay in
//! view stay where they are and only the rows that come into view are painted, over those that left. A horizontal
//! scroll, or a scroll past the whole image, paints it whole.
//!
//! The content is painted in content pixels, its scratch images sized by each node's own extent and never by the
//! image or the damage, so a row painted as the content scrolls in has the bytes the same row has in an image painted
//! whole at any offset.

use crate::geometry::Bounds;
use crate::PaintNodeId;
use std::collections::{HashMap, HashSet};
use tiny_skia::Pixmap;

/// How many bytes the kept layers of one renderer hold at most beyond one image the size of its frame: a window keeps
/// one window-sized layer and a few small ones. A layer past the cap is not kept: its pixels are painted every frame.
pub(crate) const LAYER_SLACK_BYTES: u64 = 8 << 20;

/// The side of the squares, in content pixels, a node of a scroll layer too large for one scratch image is painted in.
pub(crate) const LAYER_TILE: i32 = 1024;

/// The most pixels one scratch image of a scroll layer's node covers before the node is painted a tile at a time.
pub(crate) const LAYER_SCRATCH_PIXELS: u64 = 1 << 22;

/// A kept scroll layer.
pub(crate) struct ScrollImage {
    pub(crate) pixels: Pixmap,
    pub(crate) scale: f64,
    /// Where its node's clip shows it in the window, in window pixels.
    pub(crate) viewport: Bounds,
    /// The content pixel of the image's first column, and the first content row it holds.
    pub(crate) x: i32,
    pub(crate) top: i32,
}

impl ScrollImage {
    pub(crate) fn bytes(&self) -> u64 {
        u64::from(self.pixels.width()) * u64::from(self.pixels.height()) * 4
    }

    /// The content rows the image holds.
    pub(crate) fn content(&self) -> Bounds {
        Bounds {
            x0: self.x,
            y0: self.top,
            x1: self.x + self.pixels.width() as i32,
            y1: self.top + self.pixels.height() as i32,
        }
    }
}

/// The rows `y0..y1` of a content rectangle as they lie in an image of `height` rows kept as a ring: each piece's
/// content rows and the image row it starts at. A rectangle no taller than the image has at most two pieces.
pub(crate) fn ring_pieces(y0: i32, y1: i32, height: i32) -> Vec<(i32, i32, i32)> {
    let mut pieces = Vec::with_capacity(2);
    let mut y = y0;
    while y < y1 && height > 0 {
        let row = y.rem_euclid(height);
        let end = y1.min(y + (height - row));
        pieces.push((y, end, row));
        y = end;
    }
    pieces
}

/// Everything a renderer keeps between frames, by node.
#[derive(Default)]
pub(crate) struct Kept {
    pub(crate) scrolls: HashMap<PaintNodeId, ScrollImage>,
    /// The scroll layers of the last frame drawn, kept or not. A layer new in a frame shows what its nodes, all new
    /// to it, damage; one the frame had before, with no image, shows what it did there only if laid over whole.
    pub(crate) seen: HashSet<PaintNodeId>,
}

impl Kept {
    pub(crate) fn bytes(&self) -> u64 {
        self.scrolls.values().map(ScrollImage::bytes).sum()
    }

    /// Lets the images go; the layers stay seen, so the next frame lays them over whole.
    pub(crate) fn clear(&mut self) {
        self.scrolls.clear();
    }

    pub(crate) fn count(&self) -> u64 {
        self.scrolls.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_wraps_into_at_most_two_pieces_of_the_ring() {
        assert_eq!(ring_pieces(0, 10, 10), vec![(0, 10, 0)]);
        assert_eq!(ring_pieces(5, 15, 10), vec![(5, 10, 5), (10, 15, 0)]);
        assert_eq!(ring_pieces(-3, 4, 10), vec![(-3, 0, 7), (0, 4, 0)]);
        assert_eq!(ring_pieces(23, 27, 10), vec![(23, 27, 3)]);
        assert!(ring_pieces(4, 4, 10).is_empty());
    }
}
