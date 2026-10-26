//! Snapping guides of partly covered windows (control flow and constants).
//!
//! The windows are walked front to back. A window that touches none of the windows in front of it
//! guides with its whole frame. Otherwise only its visible part does: the frame minus the union of
//! the frames in front, as a banded region (horizontal bands, each a run of x spans, bands with the
//! same spans merged; the design's region type is banded this way). A piece narrower or
//! shorter than `min_piece` (or than the window, when the window is smaller) survives only when one
//! of its edges lies whole on the edge of another piece (reading of the loop), and a piece
//! guides only along its edges that are edges of the window itself.

use crate::geometry::{Edges, Rect};
use crate::magnet::Guide;

/// Guides for windows given front to back, `min_piece` being (20 pt).
pub fn window_guides(front_to_back: &[Rect], min_piece: f64) -> Vec<Guide> {
    let mut guides = Vec::new();
    for (i, window) in front_to_back.iter().enumerate() {
        let front = &front_to_back[..i];
        if !front.iter().any(|f| window.intersection(f).is_some_and(|r| !r.is_empty())) {
            guides.push(Guide::Window(*window));
            continue;
        }
        let pieces = visible_pieces(*window, front);
        let (min_w, min_h) = (min_piece.min(window.w), min_piece.min(window.h));
        for (k, piece) in pieces.iter().enumerate() {
            let small = piece.w < min_w || piece.h < min_h;
            if small && !pieces.iter().enumerate().any(|(j, other)| j != k && shares_full_edge(piece, other)) {
                continue;
            }
            let edges = Edges { left: piece.x == window.x, right: piece.right() == window.right(), top: piece.y == window.y, bottom: piece.bottom() == window.bottom() };
            if edges.any() {
                guides.push(Guide::Piece { rect: *piece, edges });
            }
        }
    }
    guides
}

/// A horizontal band of the visible region: its top, its bottom and its uncovered spans (left, right).
type Band = (f64, f64, Vec<(f64, f64)>);

/// `window` minus the union of `cover`, as banded rectangles from top to bottom.
pub fn visible_pieces(window: Rect, cover: &[Rect]) -> Vec<Rect> {
    let covers: Vec<Rect> = cover.iter().filter_map(|c| window.intersection(c)).filter(|r| !r.is_empty()).collect();
    let mut ys: Vec<f64> = covers.iter().flat_map(|c| [c.y, c.bottom()]).chain([window.y, window.bottom()]).collect();
    ys.sort_by(f64::total_cmp);
    ys.dedup();
    // (top, bottom, spans) per band, merged with the band above when the spans are equal
    let mut bands: Vec<Band> = Vec::new();
    for pair in ys.windows(2) {
        let (y0, y1) = (pair[0], pair[1]);
        let mut spans = vec![(window.x, window.right())];
        for c in covers.iter().filter(|c| c.y <= y0 && c.bottom() >= y1) {
            spans = spans.into_iter().flat_map(|(a, b)| [(a, c.x.min(b)), (c.right().max(a), b)]).filter(|(a, b)| b > a).collect();
        }
        match bands.last_mut() {
            Some(last) if last.1 == y0 && last.2 == spans => last.1 = y1,
            _ => bands.push((y0, y1, spans)),
        }
    }
    bands.into_iter().flat_map(|(y0, y1, spans)| spans.into_iter().map(move |(a, b)| Rect::new(a, y0, b - a, y1 - y0))).collect()
}

/// Whether a whole edge of `piece` lies on the facing edge of `other`.
fn shares_full_edge(piece: &Rect, other: &Rect) -> bool {
    let within = |a0: f64, a1: f64, b0: f64, b1: f64| b0 <= a0 && a1 <= b1;
    let vertical = within(piece.y, piece.bottom(), other.y, other.bottom()) && (piece.x == other.right() || piece.right() == other.x);
    let horizontal = within(piece.x, piece.right(), other.x, other.right()) && (piece.y == other.bottom() || piece.bottom() == other.y);
    vertical || horizontal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uncovered_window_guides_whole() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(200.0, 0.0, 100.0, 100.0);
        assert_eq!(window_guides(&[a, b], 20.0), vec![Guide::Window(a), Guide::Window(b)]);
    }

    #[test]
    fn a_window_covered_in_the_middle_splits_into_bands() {
        let back = Rect::new(0.0, 0.0, 300.0, 300.0);
        let front = Rect::new(100.0, 100.0, 100.0, 100.0);
        assert_eq!(
            visible_pieces(back, &[front]),
            vec![
                Rect::new(0.0, 0.0, 300.0, 100.0),
                Rect::new(0.0, 100.0, 100.0, 100.0),
                Rect::new(200.0, 100.0, 100.0, 100.0),
                Rect::new(0.0, 200.0, 300.0, 100.0),
            ]
        );
    }

    #[test]
    fn pieces_guide_only_along_the_windows_own_edges() {
        // the front window covers the right half: the visible left half guides with its left,
        // top and bottom edges, not with the cut at x = 150
        let back = Rect::new(0.0, 0.0, 300.0, 200.0);
        let front = Rect::new(150.0, -50.0, 300.0, 300.0);
        let guides = window_guides(&[front, back], 20.0);
        assert_eq!(guides[1], Guide::Piece { rect: Rect::new(0.0, 0.0, 150.0, 200.0), edges: Edges { left: true, right: false, top: true, bottom: true } });
    }

    #[test]
    fn a_thin_piece_survives_only_next_to_another_piece() {
        // a 10 pt strip left visible on the right, alone: dropped
        let back = Rect::new(0.0, 0.0, 300.0, 200.0);
        let front = Rect::new(-10.0, -10.0, 300.0, 300.0);
        assert_eq!(window_guides(&[front, back], 20.0).len(), 1, "only the front window guides");
        // a 10 pt strip under a 100 pt band: kept, its whole top edge lies on the band's bottom
        let notch = Rect::new(0.0, 100.0, 290.0, 200.0);
        let guides = window_guides(&[notch, back], 20.0);
        assert!(guides.contains(&Guide::Piece { rect: Rect::new(290.0, 100.0, 10.0, 100.0), edges: Edges { left: false, right: true, top: false, bottom: true } }));
    }
}
