//! The floating thumbnail: its size, its lifetime and the drag that swipes it away. A capture shows as a thumbnail in
//! the bottom-right corner; it commits (the file is written) when it expires after five seconds, when it is swiped to
//! the right or clicked, and then slides off to the right while it fades out.
//!
//! Times are seconds on a clock the caller chooses; positions are logical points on the thumbnail's surface.

use cupertino_animation::CubicBezier;
use cupertino_geometry::Point;

use crate::values::{
    DRAG_SLOP, SWIPE_DISMISS_FACTOR, SWIPE_DISMISS_MIN, SWIPE_THRESHOLD, THUMBNAIL_DISMISS,
    THUMBNAIL_EXPIRATION, THUMBNAIL_MIN_ASPECT, THUMBNAIL_SCALE,
};

/// Size of the thumbnail of an image of `image` points on a first screen of `screen` points:
/// a tenth of the screen along the image's long axis, the other side following the image's aspect between a quarter
/// of that and a tenth of the screen.
pub fn size(image: (f64, f64), screen: (f64, f64)) -> (f64, f64) {
    let (w, h) = image;
    let (sw, sh) = screen;
    if h <= w {
        let width = THUMBNAIL_SCALE * sw;
        (
            width,
            (h * width / w).clamp(THUMBNAIL_MIN_ASPECT * width, THUMBNAIL_SCALE * sh),
        )
    } else {
        let height = THUMBNAIL_SCALE * sh;
        (
            (w * height / h).clamp(THUMBNAIL_MIN_ASPECT * height, THUMBNAIL_SCALE * sw),
            height,
        )
    }
}

/// Why the thumbnail went away; every reason commits the capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    /// It expired.
    Expired,
    /// It was swiped to the right.
    Swiped,
    /// It was clicked. The design opens the markup editor; this tool has none and commits instead (approximate).
    Clicked,
}

/// Where the thumbnail is drawn and how opaque, at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Horizontal offset from its resting place, positive to the right.
    pub offset: f64,
    pub alpha: f64,
}

/// The thumbnail's state at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Shown(Look),
    /// Committed and sliding away.
    Dismissing(Look),
    /// The dismiss animation ended.
    Gone,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    start: Point,
    /// Whether the pointer left the slop square, which makes the press a drag rather than a click.
    moved: bool,
    offset: f64,
    /// The last two samples `(time, offset)`, for the release velocity.
    samples: [(f64, f64); 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Dismiss {
    start: f64,
    duration: f64,
    from: f64,
}

/// A thumbnail of `width` points, shown at `shown`.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumbnail {
    width: f64,
    shown: f64,
    drag: Option<Drag>,
    dismiss: Option<(Commit, Dismiss)>,
}

impl Thumbnail {
    pub fn new(width: f64, shown: f64) -> Self {
        Self {
            width,
            shown,
            drag: None,
            dismiss: None,
        }
    }

    /// Distance the dismiss animation travels: twice the width toward the placement side.
    fn travel(&self) -> f64 {
        2.0 * self.width
    }

    /// Why it was committed, once it was.
    pub fn committed(&self) -> Option<Commit> {
        self.dismiss.map(|(c, _)| c)
    }

    fn start_dismiss(&mut self, commit: Commit, now: f64, duration: f64, from: f64) {
        if self.dismiss.is_none() {
            self.dismiss = Some((
                commit,
                Dismiss {
                    start: now,
                    duration,
                    from,
                },
            ));
        }
    }

    /// Advance to `now`: an expired thumbnail starts its dismiss unless it is being dragged (the drag decides on
    /// release). Hover does not pause the expiry (approximate: the design's hover-pause rule is unknown).
    pub fn tick(&mut self, now: f64) {
        if self.drag.is_none() && now >= self.shown + THUMBNAIL_EXPIRATION {
            self.start_dismiss(Commit::Expired, now, THUMBNAIL_DISMISS, 0.0);
        }
    }

    /// When [`Thumbnail::tick`] next changes something on its own: the expiry, or `None` while dismissing or dragged
    /// (the caller redraws every frame then).
    pub fn deadline(&self) -> Option<f64> {
        (self.drag.is_none() && self.dismiss.is_none()).then_some(self.shown + THUMBNAIL_EXPIRATION)
    }

    pub fn phase(&self, now: f64) -> Phase {
        match self.dismiss {
            Some((_, d)) => {
                let t = ((now - d.start) / d.duration).clamp(0.0, 1.0);
                if t >= 1.0 {
                    return Phase::Gone;
                }
                let eased = CubicBezier::EASE_IN_EASE_OUT.eval(t);
                Phase::Dismissing(Look {
                    offset: d.from + (self.travel() - d.from) * eased,
                    alpha: 1.0 - eased,
                })
            }
            None => Phase::Shown(Look {
                offset: self.drag.map_or(0.0, |d| d.offset),
                alpha: 1.0,
            }),
        }
    }

    pub fn press(&mut self, p: Point, now: f64) {
        if self.dismiss.is_none() {
            self.drag = Some(Drag {
                start: p,
                moved: false,
                offset: 0.0,
                samples: [(now, 0.0); 2],
            });
        }
    }

    /// The pointer moved with the button held: past the slop square the thumbnail follows the pointer to
    /// the right; it does not move left of its place (approximate).
    pub fn motion(&mut self, p: Point, now: f64) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        let (dx, dy) = (p.x - drag.start.x, p.y - drag.start.y);
        if !drag.moved && dx.abs() < DRAG_SLOP && dy.abs() < DRAG_SLOP {
            return;
        }
        drag.moved = true;
        drag.offset = dx.max(0.0);
        drag.samples = [drag.samples[1], (now, drag.offset)];
    }

    /// The button was released: a press that never left the slop square is a click; a drag past
    /// [`SWIPE_THRESHOLD`] swipes, with the duration from the release velocity; a shorter drag puts it back.
    pub fn release(&mut self, now: f64) {
        let Some(drag) = self.drag.take() else { return };
        if !drag.moved {
            self.start_dismiss(Commit::Clicked, now, THUMBNAIL_DISMISS, 0.0);
        } else if drag.offset > SWIPE_THRESHOLD {
            let [(t0, x0), (t1, x1)] = drag.samples;
            let velocity = if t1 > t0 { (x1 - x0) / (t1 - t0) } else { 0.0 };
            let duration = if velocity > 0.0 {
                THUMBNAIL_DISMISS.min(
                    SWIPE_DISMISS_MIN.max(SWIPE_DISMISS_FACTOR * (self.travel() / velocity).abs()),
                )
            } else {
                THUMBNAIL_DISMISS
            };
            self.start_dismiss(Commit::Swiped, now, duration, drag.offset);
        } else {
            // an expiry that passed during the drag applies now
            self.tick(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_is_a_tenth_of_the_screen_on_the_long_axis_and_keeps_a_quarter_aspect() {
        assert_eq!(size((1440.0, 900.0), (1440.0, 900.0)), (144.0, 90.0));
        // a wide strip keeps a quarter of the width as its height
        assert_eq!(size((1000.0, 10.0), (1440.0, 900.0)), (144.0, 36.0));
        // a tall image is a tenth of the screen high
        assert_eq!(size((300.0, 600.0), (1440.0, 900.0)), (45.0, 90.0));
        assert_eq!(size((10.0, 600.0), (1440.0, 900.0)), (22.5, 90.0));
    }

    #[test]
    fn it_expires_after_five_seconds_and_slides_twice_its_width_in_half_a_second() {
        let mut t = Thumbnail::new(144.0, 10.0);
        assert_eq!(t.deadline(), Some(15.0));
        t.tick(14.9);
        assert_eq!(
            t.phase(14.9),
            Phase::Shown(Look {
                offset: 0.0,
                alpha: 1.0
            })
        );
        t.tick(15.0);
        assert_eq!(t.committed(), Some(Commit::Expired));
        assert_eq!(t.deadline(), None);
        let Phase::Dismissing(mid) = t.phase(15.25) else {
            panic!("dismissing at the midpoint")
        };
        // ease-in-ease-out is symmetric: half the travel at half the time
        assert!(
            (mid.offset - 144.0).abs() < 1e-6 && (mid.alpha - 0.5).abs() < 1e-6,
            "{mid:?}"
        );
        assert_eq!(t.phase(15.5), Phase::Gone);
    }

    #[test]
    fn a_press_without_movement_is_a_click_and_a_small_drag_snaps_back() {
        let mut t = Thumbnail::new(144.0, 0.0);
        t.press(Point::new(50.0, 40.0), 1.0);
        t.motion(Point::new(54.0, 44.0), 1.1);
        assert_eq!(
            t.phase(1.1),
            Phase::Shown(Look {
                offset: 0.0,
                alpha: 1.0
            }),
            "inside the 5 pt slop"
        );
        t.release(1.2);
        assert_eq!(t.committed(), Some(Commit::Clicked));

        let mut t = Thumbnail::new(144.0, 0.0);
        t.press(Point::new(50.0, 40.0), 1.0);
        t.motion(Point::new(70.0, 40.0), 1.1);
        assert_eq!(
            t.phase(1.1),
            Phase::Shown(Look {
                offset: 20.0,
                alpha: 1.0
            })
        );
        t.release(1.2);
        assert_eq!(
            (t.committed(), t.phase(1.2)),
            (
                None,
                Phase::Shown(Look {
                    offset: 0.0,
                    alpha: 1.0
                })
            )
        );
    }

    #[test]
    fn a_swipe_past_thirty_points_dismisses_with_the_velocity_duration() {
        let mut t = Thumbnail::new(144.0, 0.0);
        t.press(Point::new(0.0, 0.0), 1.0);
        t.motion(Point::new(20.0, 0.0), 1.0);
        // 40 pt in 0.1 s: 400 pt/s, 0.015 * 288 / 400 = 0.0108, raised to the 0.08 floor
        t.motion(Point::new(60.0, 0.0), 1.1);
        t.release(1.1);
        assert_eq!(t.committed(), Some(Commit::Swiped));
        assert_eq!(t.phase(1.1 + 0.08), Phase::Gone);
        let Phase::Dismissing(start) = t.phase(1.1) else {
            panic!("dismissing")
        };
        assert_eq!(
            start.offset, 60.0,
            "the slide continues from where the drag left it"
        );
    }

    #[test]
    fn an_expiry_during_a_drag_waits_for_the_release() {
        let mut t = Thumbnail::new(144.0, 0.0);
        t.press(Point::new(0.0, 0.0), 4.0);
        t.motion(Point::new(10.0, 0.0), 4.5);
        t.tick(6.0);
        assert_eq!(t.committed(), None);
        t.release(6.5);
        assert_eq!(t.committed(), Some(Commit::Expired));
    }
}
