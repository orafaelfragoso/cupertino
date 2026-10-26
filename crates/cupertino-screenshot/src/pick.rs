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

#[cfg(test)]
mod tests {
    use super::*;

    fn window(identifier: &str, frame: Rect) -> PickWindow {
        PickWindow {
            identifier: identifier.into(),
            frame,
        }
    }

    #[test]
    fn the_front_most_window_under_the_pointer_is_picked() {
        let windows = [
            window("front", Rect::new(100.0, 100.0, 200.0, 200.0)),
            window("back", Rect::new(0.0, 0.0, 400.0, 400.0)),
        ];
        assert_eq!(window_at(&windows, Point::new(150.0, 150.0)), Some(0));
        assert_eq!(window_at(&windows, Point::new(350.0, 50.0)), Some(1));
        assert_eq!(window_at(&windows, Point::new(500.0, 50.0)), None);
    }

    #[test]
    fn the_highlight_is_the_window_clipped_to_the_output() {
        let w = window("a", Rect::new(1200.0, 100.0, 200.0, 100.0));
        assert_eq!(
            highlight(&w, &Rect::new(0.0, 0.0, 1280.0, 800.0)),
            Some(Rect::new(1200.0, 100.0, 80.0, 100.0))
        );
        assert_eq!(
            highlight(&w, &Rect::new(1280.0, 0.0, 1920.0, 1080.0)),
            Some(Rect::new(1280.0, 100.0, 120.0, 100.0))
        );
        assert_eq!(highlight(&w, &Rect::new(0.0, 900.0, 1280.0, 800.0)), None);
    }
}
