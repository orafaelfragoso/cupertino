//! The floating thumbnail: its size, its lifetime and the drag that swipes it away. A capture shows as a thumbnail in
//! the bottom-right corner; it commits (the file is written) when it expires after five seconds, when it is swiped to
//! the right or clicked, and then slides off to the right while it fades out.
//!
//! Times are seconds on a clock the caller chooses; positions are logical points on the thumbnail's surface.

use cupertino_geometry::Point;
use cupertino_kit::motion::Easing;

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
                let eased = Easing::EASE_IN_OUT.eval(t);
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
