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
