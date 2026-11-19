//! The icon of a client's drag that ended without a drop: a drag released where nothing takes it, refused by its
//! target or cancelled with Escape does not lose its icon at once. A snapshot of the icon flies back to where the drag
//! started and then goes; when the window the drag started in is no longer shown, the snapshot fades out where the
//! drag ended instead. A drag that drops removes its icon at once.

use cupertino_effects::{Driver, Ease, ProgressAnimator};
use cupertino_theme::animation::EASE_OUT_CURVE;
use cupertino_wm::{Rect, WindowId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

use crate::render::snapshots::SnapshotOwner;

/// How long the icon of a drag that ended without a drop takes to fly back to where the drag started, in seconds.
pub const DRAG_RETURN_DURATION: f64 = 0.4;
/// How long the icon of a drag that ended without a drop takes to fade out where it was let go, when there is nothing
/// to fly back to, in seconds.
pub const DRAG_FADE_DURATION: f64 = 0.25;
/// The easing of the flight back: fast at first, settling into place.
const RETURN_EASE: Ease = Ease::Bezier(EASE_OUT_CURVE);

/// Where a client's drag started, kept while it runs.
#[derive(Clone, Debug)]
pub struct DragStart {
    /// The surface the drag started from.
    pub origin: Option<WlSurface>,
    /// The window holding that surface, and the press from its frame's top-left corner, so the icon returns to where
    /// that point of the window is at the end.
    pub window: Option<(WindowId, Point<f64, Logical>)>,
    /// The press in the global space, for a drag from a surface that is not a window's.
    pub at: Point<f64, Logical>,
}

/// A drag that just ended without a drop, waiting for its icon to be captured: the icon surface, where it was drawn
/// from the pointer, and where the pointer was.
#[derive(Clone, Debug)]
pub struct DragEnd {
    pub icon: WlSurface,
    pub offset: Point<i32, Logical>,
    pub start: Option<DragStart>,
}

/// What the icon does after a drag without a drop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Back {
    /// Flies to this top-left corner of its area, in the global space.
    To(f64, f64),
    /// Fades out where it is.
    Fade,
}

/// The area and opacity of an icon let go in `from` that goes `back`, at `progress` (0 to 1) of its animation.
pub fn placement(from: Rect, back: Back, progress: f64) -> (Rect, f64) {
    let p = progress.clamp(0.0, 1.0);
    match back {
        Back::To(x, y) => (
            Rect::new(
                from.x + (x - from.x) * p,
                from.y + (y - from.y) * p,
                from.w,
                from.h,
            ),
            1.0,
        ),
        Back::Fade => (from, 1.0 - p),
    }
}

/// The captured icon of a drag that ended without a drop, on its way back or fading out.
pub struct DragReturn {
    /// The captured pixels, kept while the icon shows.
    pub snapshot: SnapshotOwner,
    /// The icon's area where it was let go, in global logical points.
    pub from: Rect,
    pub back: Back,
    anim: ProgressAnimator,
}

impl DragReturn {
    /// The icon let go in `from` at `now`, going `back`.
    pub fn new(snapshot: SnapshotOwner, from: Rect, back: Back, now: f64) -> DragReturn {
        let driver = match back {
            Back::To(..) => Driver::Timed {
                duration: DRAG_RETURN_DURATION,
                ease: RETURN_EASE,
            },
            Back::Fade => Driver::Timed {
                duration: DRAG_FADE_DURATION,
                ease: Ease::Linear,
            },
        };
        let mut anim = ProgressAnimator::new(driver, 0.0);
        anim.animate_to(1.0, now);
        DragReturn {
            snapshot,
            from,
            back,
            anim,
        }
    }

    pub fn progress(&self, now: f64) -> f64 {
        self.anim.value(now).clamp(0.0, 1.0)
    }

    /// The area it is drawn in and its opacity at `now`.
    pub fn placement(&self, now: f64) -> (Rect, f64) {
        placement(self.from, self.back, self.progress(now))
    }

    pub fn is_done(&self, now: f64) -> bool {
        !self.anim.is_animating(now)
    }
}
