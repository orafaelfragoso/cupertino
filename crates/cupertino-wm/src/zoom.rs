//! Zoom = best fit, not maximize (the design toolkit lets the
//! client pick a "standard frame"; on Wayland only min/max size and an optional content hint exist).

use crate::config::ZoomConfig;
use crate::geometry::{Rect, Size};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SizeHints {
    pub min: Option<Size>,
    pub max: Option<Size>,
    /// Preferred content size, when the client provides one (no standard protocol: always optional).
    pub content: Option<Size>,
}

/// Best-fit frame for `current` inside `usable`.
///
/// Preferred size = content hint, else the usable area scaled by `default_fill`; then clamped to
/// `max`, enlarged to `min`, and limited to the usable area (a minimum larger than the area wins and
/// overflows). The result is centred on the current window centre and translated into the area.
pub fn best_fit(cfg: &ZoomConfig, usable: Rect, current: Rect, hints: &SizeHints) -> Rect {
    let avail = usable.inset(cfg.margin);
    let mut w = hints.content.map_or(avail.w * cfg.default_fill, |s| s.w);
    let mut h = hints.content.map_or(avail.h * cfg.default_fill, |s| s.h);
    w = w.min(avail.w);
    h = h.min(avail.h);
    if let Some(max) = hints.max {
        if max.w > 0.0 {
            w = w.min(max.w);
        }
        if max.h > 0.0 {
            h = h.min(max.h);
        }
    }
    if let Some(min) = hints.min {
        w = w.max(min.w);
        h = h.max(min.h);
    }
    let r = Rect::new(current.cx() - w / 2.0, current.cy() - h / 2.0, w, h);
    r.clamp_into(&avail)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0.0, 24.0, 1920.0, 1056.0);

    #[test]
    fn unconstrained_fills_area() {
        let r = best_fit(&ZoomConfig::default(), AREA, Rect::new(100.0, 100.0, 400.0, 300.0), &SizeHints::default());
        assert_eq!(r, AREA);
    }

    #[test]
    fn max_size_limits_and_centres_on_window() {
        let hints = SizeHints { max: Some(Size::new(800.0, 600.0)), ..Default::default() };
        let r = best_fit(&ZoomConfig::default(), AREA, Rect::new(500.0, 400.0, 400.0, 300.0), &hints);
        assert_eq!((r.w, r.h), (800.0, 600.0));
        assert_eq!((r.cx(), r.cy()), (700.0, 550.0));
        let edge = best_fit(&ZoomConfig::default(), AREA, Rect::new(100.0, 100.0, 400.0, 300.0), &hints);
        assert!(edge.x == AREA.x && edge.y == AREA.y, "pushed into the area");
    }

    #[test]
    fn content_hint_is_not_maximize() {
        let hints = SizeHints { content: Some(Size::new(1000.0, 700.0)), ..Default::default() };
        let r = best_fit(&ZoomConfig::default(), AREA, Rect::new(1500.0, 800.0, 400.0, 300.0), &hints);
        assert_eq!((r.w, r.h), (1000.0, 700.0));
        assert!(r.right() <= AREA.right() && r.bottom() <= AREA.bottom());
    }

    #[test]
    fn min_beats_area_and_hint_is_clamped() {
        let small = Rect::new(0.0, 0.0, 300.0, 200.0);
        let hints = SizeHints { min: Some(Size::new(2000.0, 500.0)), ..Default::default() };
        let r = best_fit(&ZoomConfig::default(), small, Rect::new(0.0, 0.0, 100.0, 100.0), &hints);
        assert_eq!(r.w, 2000.0);
        let hints = SizeHints { content: Some(Size::new(5000.0, 5000.0)), ..Default::default() };
        let r = best_fit(&ZoomConfig::default(), AREA, Rect::new(0.0, 0.0, 100.0, 100.0), &hints);
        assert_eq!((r.w, r.h), (AREA.w, AREA.h));
    }

    #[test]
    fn margin_and_fill_fraction() {
        let cfg = ZoomConfig { margin: 10.0, default_fill: 0.5 };
        let r = best_fit(&cfg, AREA, Rect::new(0.0, 0.0, 100.0, 100.0), &SizeHints::default());
        assert_eq!((r.w, r.h), (950.0, 518.0));
        assert!(r.x >= 10.0 && r.y >= 34.0);
    }
}
