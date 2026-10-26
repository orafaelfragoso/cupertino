//! Quick-preview transition geometry.
//!
//! The design opens its preview panel by an animated frame change of an ordinary window: the
//! panel starts at the source item's rectangle and ends at the fitted content rectangle. The
//! duration is the system window-resize time, whose structure is as designed: `min(0.35, 181 *
//! g(maxDelta / 100 + 1) / velocity) where is the largest change of origin or size and
//! the cap is 0.35 s. The function `g` (a libm routine) and the velocity (a runtime preference)
//! is not known: this module uses `ln` and a velocity of 1000, which is approximate, and an
//! ease-in-out interpolation (approximate). Nothing here is Linux-window-system specific: a shell
//! can drive any surface with [`ZoomTransition::at`].

use crate::geom::{Rect, Size};
use crate::metrics::timing::WINDOW_RESIZE_CAP;

/// Window-resize duration in seconds between two frames.
pub fn resize_duration(from: &Rect, to: &Rect, velocity: f64) -> f64 {
    let d = (to.x - from.x)
        .abs()
        .max((to.y - from.y).abs())
        .max((to.w - from.w).abs())
        .max((to.h - from.h).abs());
    (181.0 * (d / 100.0 + 1.0).ln() / velocity).min(WINDOW_RESIZE_CAP)
}

pub const DEFAULT_VELOCITY: f64 = 1000.0;

/// Cubic ease-in-out.
pub fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Fit `content` into `max` keeping the aspect ratio, never enlarging past `content` itself.
pub fn fit_content(content: Size, max: Size) -> Size {
    if content.w <= 0.0 || content.h <= 0.0 {
        return Size { w: 0.0, h: 0.0 };
    }
    let s = (max.w / content.w).min(max.h / content.h).min(1.0);
    Size {
        w: (content.w * s).round(),
        h: (content.h * s).round(),
    }
}

/// Target rectangle: the fitted content centred in `work_area` (the share of the work area the
/// panel may occupy, `max_fraction`, is approximate at 0.8).
pub fn panel_frame(content: Size, work_area: Rect, max_fraction: f64) -> Rect {
    let fitted = fit_content(
        content,
        Size {
            w: work_area.w * max_fraction,
            h: work_area.h * max_fraction,
        },
    );
    Rect::new(
        work_area.x + (work_area.w - fitted.w) / 2.0,
        work_area.y + (work_area.h - fitted.h) / 2.0,
        fitted.w,
        fitted.h,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub rect: Rect,
    /// Opacity of the item icon snapshot (fades out) and of the content (fades in).
    pub icon_alpha: f64,
    pub content_alpha: f64,
}

/// Zoom from `source` (the item's rectangle on screen) to `target`; reversible for closing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomTransition {
    pub source: Rect,
    pub target: Rect,
    pub duration: f64,
    pub closing: bool,
}

impl ZoomTransition {
    pub fn open(source: Rect, target: Rect) -> Self {
        ZoomTransition {
            source,
            target,
            duration: resize_duration(&source, &target, DEFAULT_VELOCITY),
            closing: false,
        }
    }

    pub fn close(self) -> Self {
        ZoomTransition {
            closing: true,
            ..self
        }
    }

    /// Slow-motion factor applied by the shell (Shift held): see [`crate::metrics::timing`].
    pub fn slowed(self, factor: f64) -> Self {
        ZoomTransition {
            duration: self.duration * factor,
            ..self
        }
    }

    /// Frame at `elapsed` seconds after the start of the transition.
    pub fn at(&self, elapsed: f64) -> Frame {
        let t = if self.duration <= 0.0 {
            1.0
        } else {
            (elapsed / self.duration).clamp(0.0, 1.0)
        };
        let p = ease_in_out(if self.closing { 1.0 - t } else { t });
        let l = |a: f64, b: f64| a + (b - a) * p;
        let rect = Rect::new(
            l(self.source.x, self.target.x),
            l(self.source.y, self.target.y),
            l(self.source.w, self.target.w),
            l(self.source.h, self.target.h),
        );
        Frame {
            rect,
            icon_alpha: 1.0 - p,
            content_alpha: p,
        }
    }

    pub fn finished(&self, elapsed: f64) -> bool {
        elapsed >= self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_is_capped_and_grows_with_distance() {
        let a = Rect::new(0.0, 0.0, 64.0, 64.0);
        let near = Rect::new(10.0, 10.0, 64.0, 64.0);
        let far = Rect::new(900.0, 400.0, 800.0, 600.0);
        let d_near = resize_duration(&a, &near, DEFAULT_VELOCITY);
        let d_far = resize_duration(&a, &far, DEFAULT_VELOCITY);
        assert!(d_near < d_far);
        assert_eq!(resize_duration(&a, &a, DEFAULT_VELOCITY), 0.0);
        assert!(d_far <= 0.35 + 1e-12);
        assert_eq!(
            resize_duration(&a, &Rect::new(1e6, 0.0, 1.0, 1.0), 10.0),
            0.35
        );
    }

    #[test]
    fn fit_keeps_aspect_and_never_enlarges() {
        assert_eq!(
            fit_content(
                Size {
                    w: 4000.0,
                    h: 2000.0
                },
                Size {
                    w: 1000.0,
                    h: 1000.0
                }
            ),
            Size {
                w: 1000.0,
                h: 500.0
            }
        );
        assert_eq!(
            fit_content(
                Size { w: 100.0, h: 50.0 },
                Size {
                    w: 1000.0,
                    h: 1000.0
                }
            ),
            Size { w: 100.0, h: 50.0 }
        );
        assert_eq!(
            fit_content(Size { w: 0.0, h: 5.0 }, Size { w: 10.0, h: 10.0 }),
            Size { w: 0.0, h: 0.0 }
        );
    }

    #[test]
    fn panel_is_centred_in_the_work_area() {
        let wa = Rect::new(0.0, 28.0, 1440.0, 872.0);
        let f = panel_frame(
            Size {
                w: 3000.0,
                h: 1000.0,
            },
            wa,
            0.8,
        );
        assert!((f.center().x - 720.0).abs() < 1.0);
        assert!((f.center().y - (28.0 + 436.0)).abs() < 1.0);
        assert!(f.w <= 1152.0 && f.h <= 697.6);
    }

    #[test]
    fn open_and_close_are_mirror_images() {
        let t = ZoomTransition::open(
            Rect::new(100.0, 100.0, 64.0, 64.0),
            Rect::new(300.0, 200.0, 600.0, 400.0),
        );
        let start = t.at(0.0);
        let end = t.at(t.duration);
        assert_eq!(start.rect, t.source);
        assert_eq!(end.rect, t.target);
        assert_eq!((start.icon_alpha, start.content_alpha), (1.0, 0.0));
        let c = t.close();
        assert_eq!(c.at(0.0).rect, t.target);
        assert_eq!(c.at(c.duration).rect, t.source);
        let mid = t.at(t.duration / 2.0).rect;
        assert!((mid.x - 200.0).abs() < 1e-9);
        assert!(t.finished(t.duration + 0.001));
        assert_eq!(t.slowed(10.0).duration, t.duration * 10.0);
    }

    #[test]
    fn easing_is_monotonic_and_symmetric() {
        let mut prev = 0.0;
        for i in 0..=100 {
            let v = ease_in_out(i as f64 / 100.0);
            assert!(v >= prev);
            prev = v;
        }
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-12);
        assert!((ease_in_out(0.25) + ease_in_out(0.75) - 1.0).abs() < 1e-12);
    }
}
