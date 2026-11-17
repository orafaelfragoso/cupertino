//! The window pick: the windows on screen as the server lists them (`cupertino_screenshot_v1`, front to back, frames
//! in global logical points with their decorations) and the one under the pointer.

use cupertino_geometry::{Point, Rect};

/// A window the pick can choose: its foreign-toplevel identifier, which names it for the toplevel capture, and its
/// frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PickWindow {
    pub identifier: String,
    pub frame: Rect,
}

/// The front-most window under `p`, as an index into `windows` (front to back).
pub fn window_at(windows: &[PickWindow], p: Point) -> Option<usize> {
    windows.iter().position(|w| w.frame.contains(p))
}

/// The part of `window` drawn on the output `bounds`, where its highlight goes; `None` when it is on another output.
pub fn highlight(window: &PickWindow, bounds: &Rect) -> Option<Rect> {
    window.frame.intersection(bounds)
}
