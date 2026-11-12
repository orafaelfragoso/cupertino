//! Time-parameterised placements for window and desktop transitions.
//!
//! Each effect maps a progress value (driven by a [`ProgressAnimator`]) to a [`Placement`], the
//! destination rectangle and opacity of a window texture; [`Placement::mesh`] turns it into a quad.
//!
//! * window order-in / order-out transforms ([`WindowKind`], [`TransformKind`]): which kind of
//!   window gets which transform, the durations, the progress functions and the `sin²` ease are as designed. The design also defines two spring pop variants;
//!   no window kind selects them, so they are not modelled. Plain windows open and close without a
//!   transform, and the window server adds none.
//! * sheets ([`WindowKind::Sheet`], [`TransformKind::SheetIn`] / [`TransformKind::SheetOut`]): 0.26 s
//!   on linear time, a 32 pt slide eased with `sin²` and a linear fade (magnitudes and
//!   timing); the slide direction (down into place, back up on close) is derived
//!   from the sign convention of the inverse transform.
//! * zoom and programmatic frame changes: `T = min(0.35, 181·ln(maxΔ/100 + 1)/750) with the
//!   `sin²` ease.
//! * full screen: 0.5 s, frame curve bézier (0.25, 0, 0, 1), overlay fade (0, 1, 1, 1) over the
//!   first half.
//! * show desktop: spring with damping ratio 1.0 and response 0.3 s;
//!   the slide-out geometry is.
//! * overview zoom: duration 0.25 s, 2.0 s with the slow-motion modifier; the easing curve and the window-to-slot interpolation are approximate.
//! * window-server moves (tile, status item): 0.25 s on (0.42, 0, 0.58, 1);
//!   tiling springs: damping ratio 1.0 with responses 0.38 / 0.3 / 0.2 s and 0.86 / 0.15 s
//!   (roles);
//! * workspace slide geometry ([`WorkspaceSlide`]): neighbouring workspaces are one output width
//!   plus [`SPACE_GAP`] apart; the timing is the solver in [`crate::space_slide`].

use crate::ease::{cosine, Driver, Ease, ProgressAnimator};
use crate::mesh::Mesh;
use crate::scale::lerp_rect;
use crate::space_slide::SPACE_GAP;
use cupertino_animation::CubicBezier;
use cupertino_geometry::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub rect: Rect,
    pub opacity: f64,
}

impl Placement {
    pub fn mesh(&self) -> Mesh {
        Mesh::quad(self.rect)
    }
}

/// Scale `rect` about its centre.
pub fn scale_about_center(rect: Rect, s: f64) -> Rect {
    let c = rect.center();
    let (w, h) = (rect.width() * s, rect.height() * s);
    Rect::new(c.x - w / 2.0, c.y - h / 2.0, w, h)
}

/// Window class that selects the order-in / order-out transform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// Ordinary titled window: no transform either way.
    #[default]
    Plain,
    /// Window that shows a document: grows in, no close transform.
    Document,
    /// Utility panel: no open transform, fades out.
    Utility,
    /// Alert run modally: pops in, scales down and out.
    Alert,
    /// Application-modal window: grows in, fades out.
    AppModal,
    /// Window-modal child attached under its parent's title bar: slides down into place while
    /// fading in, slides back up while fading out (ordinary sheet style 0).
    Sheet,
}

impl WindowKind {
    /// Transform played when the window is ordered in.
    pub fn order_in(self) -> Option<TransformKind> {
        match self {
            WindowKind::Plain | WindowKind::Utility | WindowKind::Document => {
                Some(TransformKind::WindowOpen)
            }
            WindowKind::AppModal => Some(TransformKind::PopIn),
            WindowKind::Alert => Some(TransformKind::PopIn),
            WindowKind::Sheet => Some(TransformKind::SheetIn),
        }
    }

    /// Transform played when the window is ordered out (the alert entry is the
    /// one the modal alert runner sets explicitly).
    pub fn order_out(self) -> Option<TransformKind> {
        match self {
            WindowKind::Plain | WindowKind::Document => Some(TransformKind::WindowClose),
            WindowKind::Utility | WindowKind::AppModal => Some(TransformKind::FadeOut),
            WindowKind::Alert => Some(TransformKind::ScaleDownAndOut),
            WindowKind::Sheet => Some(TransformKind::SheetOut),
        }
    }
}

/// Scale, opacity and vertical offset of a window under a transform. The scale is a magnification
/// about the window centre (the default anchor); `dy` moves the window down by
/// that many points (negative = up); only the sheet transforms set it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformSample {
    pub scale: f64,
    pub alpha: f64,
    pub dy: f64,
}

impl TransformSample {
    /// The window as it is at rest: no scale, opaque, in place.
    pub const IDENTITY: TransformSample = TransformSample {
        scale: 1.0,
        alpha: 1.0,
        dy: 0.0,
    };
}

/// Window order-in / order-out transforms with progress functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransformKind {
    /// Grow from the initial scale 0 while fading in, 0.2 s.
    ScaleUpAndIn,
    /// Alert pop-in, 0.31 s: grows quickly, overshoots to 1.18× at half way and settles back to 1.
    PopIn,
    /// Mirror of [`TransformKind::PopIn`], 0.31 s.
    PopOut,
    /// Shrink to 0.9× while fading out, 0.15 s.
    ScaleDownAndOut,
    /// Fade out without scaling, 0.15 s.
    FadeOut,
    /// Sheet open, 0.26 s: starts [`SHEET_SLIDE`] above its place and slides down while fading in.
    SheetIn,
    /// Sheet close, 0.26 s: slides [`SHEET_SLIDE`] back up while fading out.
    SheetOut,
    /// Ordinary window open, 0.16 s: grows from [`WINDOW_OPEN_SCALE`] to 1 at once-opaque, quick at first then
    /// settling (exponential ease-out); its shadow darkens with it.
    WindowOpen,
    /// Ordinary window close, 0.15 s: fades out quickly (exponential) while shrinking a little.
    WindowClose,
    /// Appearance change, 0.5 s: the window as it was before the change fades out over the redrawn one, eased in and out,
    /// without scaling.
    AppearanceFade,
}

/// Scale an ordinary window starts from when it opens (0.98).
pub const WINDOW_OPEN_SCALE: f64 = 0.98;
/// Duration of the ordinary window open (0.16 s).
pub const WINDOW_OPEN_DURATION: f64 = 0.16;
/// Rate of the window-open ease-out, in 1/duration: larger settles sooner.
const WINDOW_OPEN_RATE: f64 = 3.0;
/// Share of its final darkness the shadow of an opening window starts with.
const WINDOW_OPEN_SHADOW_START: f64 = 0.45;
/// Duration of the crossfade when the appearance changes (0.5 s).
pub const APPEARANCE_FADE_DURATION: f64 = 0.5;
/// Scale an ordinary window ends at when it closes (0.983).
pub const WINDOW_CLOSE_SCALE: f64 = 0.983;
/// Duration of the ordinary window close (0.15 s).
pub const WINDOW_CLOSE_DURATION: f64 = 0.15;
/// Rate of the window-close fade, in 1/duration (time constant about 35 ms).
const WINDOW_CLOSE_FADE_RATE: f64 = 4.3;
/// Rate of the window-close shrink, in 1/duration (settles within about 60 ms).
const WINDOW_CLOSE_SHRINK_RATE: f64 = 7.5;

/// Exponential ease-out of `t` in `[0, 1]`, normalised to reach exactly 1 at the end.
fn exp_out(t: f64, rate: f64) -> f64 {
    (1.0 - (-rate * t).exp()) / (1.0 - (-rate).exp())
}

/// Overshoot of the pop transforms (0.18).
pub const POP_SCALE_AMOUNT: f64 = 0.18;
/// Progress at which the pop-in reaches its peak (0.5).
pub const POP_SPLIT: f64 = 0.5;
/// Final scale of the scale-down-and-out transform (0.9).
pub const SCALE_DOWN_AND_OUT_SCALE: f64 = 0.9;
/// Initial scale of the scale-up-and-in transform (default 0).
pub const SCALE_UP_AND_IN_INITIAL_SCALE: f64 = 0.0;
/// Factor applied to transform durations while the slow-motion modifier is held (4,
/// ).
pub const TRANSFORM_SLOW_MOTION_FACTOR: f64 = 4.0;
/// Sheet open and close duration, the sheet animation time default (0.26 s, the
/// close reusing it is ).
pub const SHEET_DURATION: f64 = 0.26;
/// Pop-in and pop-out duration (0.31 s).
pub const POP_DURATION: f64 = 0.31;
/// Distance a sheet slides in and out, in points (32).
pub const SHEET_SLIDE: f64 = 32.0;

/// Scale a pop-in starts from.
const POP_START_SCALE: f64 = 0.07;
/// Exponent of the pop-in's rise: above 1 it starts slowly and flattens at the peak.
const POP_RISE_EXPONENT: f64 = 1.7;
/// Exponent of the pop-in's settling: above 1 it leaves the peak quickly and creeps in at the end.
const POP_SETTLE_EXPONENT: f64 = 1.35;
/// Share of the pop-in's opacity ramp, relative to its duration: it is opaque from here on.
const POP_FADE_SHARE: f64 = 0.35;

/// Pop-in scale at progress `t` (linear in time): it rises to the overshoot at [`POP_SPLIT`] and settles to 1.
fn pop_in_scale(t: f64) -> f64 {
    let peak = 1.0 + POP_SCALE_AMOUNT;
    if t <= POP_SPLIT {
        let u = (t / POP_SPLIT).clamp(0.0, 1.0);
        POP_START_SCALE
            + (peak - POP_START_SCALE)
                * (u * std::f64::consts::FRAC_PI_2)
                    .sin()
                    .powf(POP_RISE_EXPONENT)
    } else {
        let v = ((t - POP_SPLIT) / (1.0 - POP_SPLIT)).clamp(0.0, 1.0);
        1.0 + POP_SCALE_AMOUNT * (1.0 - v).powf(POP_SETTLE_EXPONENT)
    }
}

impl TransformKind {
    /// Duration in seconds (tables).
    pub fn duration(self) -> f64 {
        match self {
            TransformKind::ScaleUpAndIn => 0.2,
            TransformKind::PopIn | TransformKind::PopOut => POP_DURATION,
            TransformKind::ScaleDownAndOut | TransformKind::FadeOut => 0.15,
            TransformKind::SheetIn | TransformKind::SheetOut => SHEET_DURATION,
            TransformKind::WindowOpen => WINDOW_OPEN_DURATION,
            TransformKind::WindowClose => WINDOW_CLOSE_DURATION,
            TransformKind::AppearanceFade => APPEARANCE_FADE_DURATION,
        }
    }

    /// How the transform advances. The window transforms use the `sin²` ease,
    /// stretched ×4 in slow motion. The sheets run on linear time and ease inside
    /// [`TransformKind::sample`]. Whether slow motion stretches a sheet is
    /// unknown (recovers only the duration default), so `slow_motion` leaves them as they are.
    pub fn driver(self, slow_motion: bool) -> Driver {
        match self {
            TransformKind::SheetIn
            | TransformKind::SheetOut
            | TransformKind::PopIn
            | TransformKind::PopOut
            | TransformKind::WindowOpen
            | TransformKind::WindowClose => Driver::Timed {
                duration: self.duration(),
                ease: Ease::Linear,
            },
            _ => {
                let factor = if slow_motion {
                    TRANSFORM_SLOW_MOTION_FACTOR
                } else {
                    1.0
                };
                Driver::Timed {
                    duration: self.duration() * factor,
                    ease: Ease::Cosine,
                }
            }
        }
    }

    pub fn animator(self, slow_motion: bool) -> ProgressAnimator {
        ProgressAnimator::new(self.driver(slow_motion), 0.0)
    }

    /// Scale, opacity and offset at `t`, the value of the transform's [`ProgressAnimator`]: already
    /// eased for the window transforms, linear time for the sheets, which ease the slide
    /// themselves. Progress functions.
    pub fn sample(self, t: f64) -> TransformSample {
        let c = t.clamp(0.0, 1.0);
        let scaled = |scale: f64, alpha: f64| TransformSample {
            scale,
            alpha,
            dy: 0.0,
        };
        match self {
            TransformKind::ScaleUpAndIn => {
                let a = SCALE_UP_AND_IN_INITIAL_SCALE;
                scaled(a + (1.0 - a) * c, c)
            }
            TransformKind::PopIn => scaled(pop_in_scale(c), (c / POP_FADE_SHARE).min(1.0)),
            TransformKind::PopOut => {
                scaled(pop_in_scale(1.0 - c), ((1.0 - c) / POP_FADE_SHARE).min(1.0))
            }
            TransformKind::ScaleDownAndOut => {
                scaled(1.0 - (1.0 - SCALE_DOWN_AND_OUT_SCALE) * c, 1.0 - c)
            }
            TransformKind::FadeOut | TransformKind::AppearanceFade => scaled(1.0, 1.0 - c),
            TransformKind::SheetIn => TransformSample {
                scale: 1.0,
                alpha: (2.0 * c).min(1.0),
                dy: -SHEET_SLIDE * (1.0 - cosine(c)),
            },
            TransformKind::SheetOut => TransformSample {
                scale: 1.0,
                alpha: 1.0 - c,
                dy: -SHEET_SLIDE * cosine(c),
            },
            TransformKind::WindowOpen => scaled(
                WINDOW_OPEN_SCALE + (1.0 - WINDOW_OPEN_SCALE) * exp_out(c, WINDOW_OPEN_RATE),
                1.0,
            ),
            TransformKind::WindowClose => scaled(
                1.0 - (1.0 - WINDOW_CLOSE_SCALE) * exp_out(c, WINDOW_CLOSE_SHRINK_RATE),
                1.0 - exp_out(c, WINDOW_CLOSE_FADE_RATE),
            ),
        }
    }

    /// How dark the window's shadow is at `t`, as a share of its resting darkness: the opacity, except that an
    /// opening window's shadow darkens while the window itself is already opaque.
    pub fn shadow_factor(self, t: f64) -> f64 {
        match self {
            TransformKind::WindowOpen => {
                let p = exp_out(t.clamp(0.0, 1.0), WINDOW_OPEN_RATE);
                WINDOW_OPEN_SHADOW_START + (1.0 - WINDOW_OPEN_SHADOW_START) * p
            }
            _ => self.sample(t).alpha,
        }
    }

    pub fn placement(self, rect: Rect, t: f64) -> Placement {
        let s = self.sample(t);
        Placement {
            rect: scale_about_center(rect, s.scale).translate(0.0, s.dy),
            opacity: s.alpha,
        }
    }
}

/// Upper bound of a programmatic frame change (0.35 s).
pub const FRAME_CHANGE_MAX_DURATION: f64 = 0.35;
/// Numerator coefficient of the frame-change duration (181).
pub const FRAME_CHANGE_COEFFICIENT: f64 = 181.0;
/// Distance scale of the frame-change duration (100 pt).
pub const FRAME_CHANGE_DELTA_SCALE: f64 = 100.0;
/// Velocity of the frame-change duration, `150 / 0.2 s` (750 pt/s).
pub const FRAME_CHANGE_VELOCITY: f64 = 750.0;

/// Duration of a zoom or programmatic frame change whose largest absolute change of x, y, width
/// or height is `max_delta` points: `min(0.35, 181·ln(maxΔ/100 + 1)/750).
pub fn frame_change_duration(max_delta: f64) -> f64 {
    let t = FRAME_CHANGE_COEFFICIENT * (max_delta.abs() / FRAME_CHANGE_DELTA_SCALE + 1.0).ln()
        / FRAME_CHANGE_VELOCITY;
    t.min(FRAME_CHANGE_MAX_DURATION)
}

/// Largest absolute change of x, y, width or height between two frames.
pub fn frame_max_delta(from: Rect, to: Rect) -> f64 {
    [
        to.min_x() - from.min_x(),
        to.min_y() - from.min_y(),
        to.width() - from.width(),
        to.height() - from.height(),
    ]
    .into_iter()
    .fold(0.0, |m, d| m.max(d.abs()))
}

/// Zoom (maximise / restore) and programmatic frame changes: a frame interpolation over
/// [`frame_change_duration`] with the `sin²` ease.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowZoom;

impl WindowZoom {
    pub fn duration(&self, from: Rect, to: Rect) -> f64 {
        frame_change_duration(frame_max_delta(from, to))
    }

    pub fn driver(&self, from: Rect, to: Rect) -> Driver {
        Driver::Timed {
            duration: self.duration(from, to),
            ease: Ease::Cosine,
        }
    }

    pub fn placement(&self, from: Rect, to: Rect, progress: f64) -> Placement {
        Placement {
            rect: lerp_rect(from, to, progress),
            opacity: 1.0,
        }
    }

    pub fn animator(&self, from: Rect, to: Rect) -> ProgressAnimator {
        ProgressAnimator::new(self.driver(from, to), 0.0)
    }
}

/// Full-screen enter / exit: the window frame and a snapshot overlay that fades in over the first
/// half. The workspace slide that runs at the same time is unknown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FullScreenTransition {
    /// 0.5 s.
    pub duration: f64,
    /// (0.25, 0, 0, 1).
    pub frame_curve: CubicBezier,
    /// (0, 1, 1, 1).
    pub fade_curve: CubicBezier,
    /// Fraction of `duration` the overlay fade takes (0.5).
    pub fade_fraction: f64,
}

impl Default for FullScreenTransition {
    fn default() -> Self {
        Self {
            duration: 0.5,
            frame_curve: CubicBezier::new(0.25, 0.0, 0.0, 1.0),
            fade_curve: CubicBezier::new(0.0, 1.0, 1.0, 1.0),
            fade_fraction: 0.5,
        }
    }
}

impl FullScreenTransition {
    /// Linear progress over [`FullScreenTransition::duration`]; the curves are applied by
    /// [`FullScreenTransition::placement`].
    pub fn animator(&self) -> ProgressAnimator {
        ProgressAnimator::new(
            Driver::Timed {
                duration: self.duration,
                ease: Ease::Linear,
            },
            0.0,
        )
    }

    /// Frame at linear progress `x`, with the overlay opacity as `opacity`.
    pub fn placement(&self, from: Rect, to: Rect, x: f64) -> Placement {
        let x = x.clamp(0.0, 1.0);
        Placement {
            rect: lerp_rect(from, to, self.frame_curve.eval(x)),
            opacity: self.fade_curve.eval(x / self.fade_fraction),
        }
    }
}

/// Horizontal workspace switch (approximate): the whole content of the workspaces slides while
/// fixed layers (menu bar, dock) stay put. `position` is the continuous index of the workspace
/// shown at the output origin, so a gesture can drive it directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkspaceSlide {
    pub output_width: f64,
    /// Space between neighbouring workspaces: [`SPACE_GAP`].
    pub gap: f64,
}

impl WorkspaceSlide {
    pub fn new(output_width: f64) -> Self {
        Self {
            output_width,
            gap: SPACE_GAP,
        }
    }

    pub fn pitch(&self) -> f64 {
        self.output_width + self.gap
    }

    /// Horizontal translation of workspace `index` when `position` is at the origin.
    pub fn offset_x(&self, index: usize, position: f64) -> f64 {
        (index as f64 - position) * self.pitch()
    }

    pub fn placement(&self, rect: Rect, index: usize, position: f64) -> Placement {
        Placement {
            rect: rect.translate(self.offset_x(index, position), 0.0),
            opacity: 1.0,
        }
    }

    /// Whether any part of workspace `index` is on the output.
    pub fn is_visible(&self, index: usize, position: f64) -> bool {
        let x = self.offset_x(index, position);
        x < self.output_width && x + self.output_width > 0.0
    }
}

/// Duration of window-server moves (tile, status-item list): 0.25 s.
pub const SERVER_MOVE_DURATION: f64 = 0.25;

/// The driver of a window-server move: 0.25 s on the ease-in-ease-out cubic (0.42, 0, 0.58, 1)
/// (timing function 4 of the server's table).
pub fn server_move_driver() -> Driver {
    Driver::Timed {
        duration: SERVER_MOVE_DURATION,
        ease: Ease::Bezier(CubicBezier::EASE_IN_EASE_OUT),
    }
}

/// Springs of the tiling component's window properties (which property
/// takes which spring is derived from the key-path chooser). Tiling and untiling a window animate
/// `frame`; the tile preview fade most likely uses `alpha`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TilingSprings {
    /// Position, size, anchors, scale and the other listed properties: damping ratio 1.0, response 0.38 s.
    pub frame: Driver,
    /// `alpha`: damping ratio 1.0, response 0.3 s.
    pub alpha: Driver,
    /// damping ratio 1.0, response 0.2 s.
    pub corner_radius: Driver,
    /// The fourth spring of the set, damping ratio 0.86, response 0.15 s; the property it drives is unknown.
    pub quick: Driver,
}

impl Default for TilingSprings {
    fn default() -> Self {
        Self {
            frame: Driver::spring_response(0.38, 1.0),
            alpha: Driver::spring_response(0.3, 1.0),
            corner_radius: Driver::spring_response(0.2, 1.0),
            quick: Driver::spring_response(0.15, 0.86),
        }
    }
}

/// Overview open/close (windows move from their place to their overview slot).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverviewZoom {
    /// 0.25 s.
    pub duration: f64,
    /// 2.0 s while the slow-motion modifier is held.
    pub slow_motion_duration: f64,
    /// approximate.
    pub ease: Ease,
}

impl Default for OverviewZoom {
    fn default() -> Self {
        Self {
            duration: 0.25,
            slow_motion_duration: 2.0,
            ease: Ease::Bezier(CubicBezier::EASE_IN_EASE_OUT),
        }
    }
}

impl OverviewZoom {
    pub fn driver(&self, slow_motion: bool) -> Driver {
        Driver::Timed {
            duration: if slow_motion {
                self.slow_motion_duration
            } else {
                self.duration
            },
            ease: self.ease,
        }
    }

    pub fn animator(&self, slow_motion: bool, initial: f64) -> ProgressAnimator {
        ProgressAnimator::new(self.driver(slow_motion), initial)
    }

    /// `progress` is the already eased progress of the animator.
    pub fn placement(&self, window: Rect, slot: Rect, progress: f64) -> Placement {
        Placement {
            rect: lerp_rect(window, slot, progress.clamp(0.0, 1.0)),
            opacity: 1.0,
        }
    }
}

/// Show desktop: windows slide off the output edge nearest to them.
///
/// the spring (damping ratio 1.0, response 0.3 s). windows
/// leave through the nearest horizontal edge (vertical edges when `allow_vertical`), all the way out
/// except `peek` points, which is unknown and configurable (default 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShowDesktop {
    pub driver: Driver,
    pub peek: f64,
    pub allow_vertical: bool,
}

pub const SHOW_DESKTOP_DAMPING_RATIO: f64 = 1.0;
pub const SHOW_DESKTOP_RESPONSE: f64 = 0.3;

impl Default for ShowDesktop {
    fn default() -> Self {
        Self {
            driver: Driver::spring_response(SHOW_DESKTOP_RESPONSE, SHOW_DESKTOP_DAMPING_RATIO),
            peek: 0.0,
            allow_vertical: false,
        }
    }
}

impl ShowDesktop {
    /// Where `window` rests while the desktop is shown.
    pub fn hidden_rect(&self, window: Rect, screen: Rect) -> Rect {
        let left = (window.max_x() - screen.min_x() - self.peek).max(0.0);
        let right = (screen.max_x() - window.min_x() - self.peek).max(0.0);
        let mut moves = vec![(left, (-left, 0.0)), (right, (right, 0.0))];
        if self.allow_vertical {
            let up = (window.max_y() - screen.min_y() - self.peek).max(0.0);
            let down = (screen.max_y() - window.min_y() - self.peek).max(0.0);
            moves.push((up, (0.0, -up)));
            moves.push((down, (0.0, down)));
        }
        let (_, (dx, dy)) = moves
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .expect("at least two candidates");
        window.translate(dx, dy)
    }

    pub fn placement(&self, window: Rect, screen: Rect, progress: f64) -> Placement {
        Placement {
            rect: lerp_rect(window, self.hidden_rect(window, screen), progress),
            opacity: 1.0,
        }
    }

    pub fn animator(&self, initial: f64) -> ProgressAnimator {
        ProgressAnimator::new(self.driver, initial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }

    #[test]
    fn transform_table_per_window_kind() {
        use TransformKind::*;
        let table: Vec<_> = [
            WindowKind::Plain,
            WindowKind::Document,
            WindowKind::Utility,
            WindowKind::Alert,
            WindowKind::AppModal,
            WindowKind::Sheet,
        ]
        .into_iter()
        .map(|k| (k.order_in(), k.order_out()))
        .collect();
        assert_eq!(
            table,
            vec![
                (None, None),
                (Some(ScaleUpAndIn), None),
                (None, Some(FadeOut)),
                (Some(PopIn), Some(ScaleDownAndOut)),
                (Some(ScaleUpAndIn), Some(FadeOut)),
                (Some(SheetIn), Some(SheetOut)),
            ]
        );
    }

    #[test]
    fn transform_durations_and_slow_motion() {
        let d = |k: TransformKind, slow| match k.driver(slow) {
            Driver::Timed { duration, ease } => {
                assert_eq!(ease, Ease::Cosine);
                duration
            }
            Driver::Spring(_) => panic!("timed expected"),
        };
        assert_eq!(d(TransformKind::ScaleUpAndIn, false), 0.2);
        assert_eq!(d(TransformKind::PopIn, false), 0.4);
        assert_eq!(d(TransformKind::PopOut, false), 0.4);
        assert_eq!(d(TransformKind::ScaleDownAndOut, false), 0.15);
        assert_eq!(d(TransformKind::FadeOut, false), 0.15);
        assert_eq!(d(TransformKind::FadeOut, true), 0.6);
    }

    #[test]
    fn document_grows_in_from_nothing_about_the_centre() {
        let w = Rect::new(400.0, 300.0, 800.0, 600.0);
        let k = TransformKind::ScaleUpAndIn;
        assert_eq!(
            k.placement(w, 0.0),
            Placement {
                rect: Rect::new(800.0, 600.0, 0.0, 0.0),
                opacity: 0.0
            }
        );
        assert_eq!(
            k.placement(w, 0.5),
            Placement {
                rect: Rect::new(600.0, 450.0, 400.0, 300.0),
                opacity: 0.5
            }
        );
        assert_eq!(
            k.placement(w, 1.0),
            Placement {
                rect: w,
                opacity: 1.0
            }
        );
        let mut a = k.animator(false);
        a.animate_to(1.0, 0.0);
        assert_close(a.value(0.1), 0.5);
        assert_close(
            a.value(0.05),
            0.5 - 0.5 * (std::f64::consts::PI / 4.0).cos(),
        );
        assert!(!a.is_animating(0.2));
    }

    #[test]
    fn alert_pop_in_overshoots_to_1_2_at_60_percent() {
        let s = |t| TransformKind::PopIn.sample(t);
        assert_close(s(0.0).scale, 1.2 * 0.2f64.powi(3));
        assert_close(s(0.3).scale, 1.2 * 0.6f64.powi(3));
        assert_close(s(0.6).scale, 1.2);
        assert_close(
            s(0.8).scale,
            1.0 + 0.2 * (0.1 * std::f64::consts::PI).cos() * 0.5f64.powf(1.5),
        );
        assert_close(s(1.0).scale, 1.0);
        assert_eq!(s(0.6).alpha, 0.6);
        let out = TransformKind::PopOut.sample(0.4);
        assert_close(out.scale, 1.2);
        assert_close(out.alpha, 0.6);
    }

    #[test]
    fn alert_and_utility_close_transforms() {
        let s = TransformKind::ScaleDownAndOut.sample(0.5);
        assert_close(s.scale, 0.95);
        assert_eq!(s.alpha, 0.5);
        assert_eq!(
            TransformKind::ScaleDownAndOut.sample(1.0),
            TransformSample {
                scale: 0.9,
                alpha: 0.0,
                dy: 0.0
            }
        );
        assert_eq!(
            TransformKind::FadeOut.sample(0.25),
            TransformSample {
                scale: 1.0,
                alpha: 0.75,
                dy: 0.0
            }
        );
    }

    #[test]
    fn sheets_slide_32_points_on_linear_time_with_an_eased_offset() {
        for k in [TransformKind::SheetIn, TransformKind::SheetOut] {
            assert_eq!(
                k.driver(false),
                Driver::Timed {
                    duration: 0.26,
                    ease: Ease::Linear
                }
            );
            assert_eq!(k.driver(true), k.driver(false), "no slow motion for sheets");
        }
        let open = |p| TransformKind::SheetIn.sample(p);
        assert_eq!(
            open(0.0),
            TransformSample {
                scale: 1.0,
                alpha: 0.0,
                dy: -32.0
            }
        );
        assert_close(open(0.25).alpha, 0.5);
        assert_close(open(0.25).dy, -32.0 * (1.0 - cosine(0.25)));
        assert_eq!(open(0.5).alpha, 1.0, "the fade completes at half time");
        assert_close(open(0.5).dy, -16.0);
        assert_eq!(open(1.0), TransformSample::IDENTITY);
        let close = |p| TransformKind::SheetOut.sample(p);
        assert_eq!(close(0.0), TransformSample::IDENTITY);
        assert_close(close(0.5).alpha, 0.5);
        assert_close(close(0.5).dy, -16.0);
        assert_eq!(
            close(1.0),
            TransformSample {
                scale: 1.0,
                alpha: 0.0,
                dy: -32.0
            }
        );
        let w = Rect::new(100.0, 200.0, 400.0, 300.0);
        assert_eq!(
            TransformKind::SheetIn.placement(w, 0.0),
            Placement {
                rect: Rect::new(100.0, 168.0, 400.0, 300.0),
                opacity: 0.0
            }
        );
    }

    #[test]
    fn frame_change_duration_follows_the_log_formula() {
        assert_close(frame_change_duration(0.0), 0.0);
        assert_close(frame_change_duration(100.0), 181.0 * 2f64.ln() / 750.0);
        assert!((frame_change_duration(100.0) - 0.1673).abs() < 1e-4);
        assert!((frame_change_duration(200.0) - 0.2651).abs() < 1e-4);
        assert_eq!(frame_change_duration(327.0), 0.35);
        assert_eq!(frame_change_duration(2000.0), 0.35);
    }

    #[test]
    fn zoom_uses_the_largest_component_change_and_the_sin2_ease() {
        let (a, b) = (
            Rect::new(100.0, 100.0, 500.0, 400.0),
            Rect::new(150.0, 40.0, 600.0, 420.0),
        );
        let z = WindowZoom;
        assert_eq!(frame_max_delta(a, b), 100.0);
        assert_eq!(
            z.driver(a, b),
            Driver::Timed {
                duration: frame_change_duration(100.0),
                ease: Ease::Cosine
            }
        );
        assert_eq!(z.placement(a, SCREEN, 0.0).rect, a);
        assert_eq!(z.placement(a, SCREEN, 1.0).rect, SCREEN);
        let mut an = z.animator(a, b);
        an.animate_to(1.0, 0.0);
        assert_close(an.value(z.duration(a, b) / 2.0), 0.5);
    }

    #[test]
    fn full_screen_frame_and_overlay_fade() {
        let f = FullScreenTransition::default();
        let (a, b) = (Rect::new(100.0, 100.0, 800.0, 600.0), SCREEN);
        assert_eq!(
            f.placement(a, b, 0.0),
            Placement {
                rect: a,
                opacity: 0.0
            }
        );
        assert_eq!(
            f.placement(a, b, 0.5).opacity,
            1.0,
            "the fade ends at half time"
        );
        assert_eq!(
            f.placement(a, b, 1.0),
            Placement {
                rect: b,
                opacity: 1.0
            }
        );
        let mid = CubicBezier::new(0.25, 0.0, 0.0, 1.0).eval(0.25);
        assert_eq!(f.placement(a, b, 0.25).rect, lerp_rect(a, b, mid));
        assert_close(
            f.placement(a, b, 0.25).opacity,
            CubicBezier::new(0.0, 1.0, 1.0, 1.0).eval(0.5),
        );
        let mut an = f.animator();
        an.animate_to(1.0, 0.0);
        assert!(an.is_animating(0.49) && !an.is_animating(0.5));
    }

    #[test]
    fn workspace_slide_geometry() {
        let s = WorkspaceSlide::new(1920.0);
        assert_eq!(s.offset_x(0, 0.0), 0.0);
        assert_eq!(
            s.offset_x(1, 0.0),
            1984.0,
            "neighbours sit one output width + 64 pt apart"
        );
        assert_eq!(s.offset_x(0, 1.0), -1984.0);
        assert_eq!(s.offset_x(0, 0.5), -992.0);
        assert_eq!(s.offset_x(1, 0.5), 992.0);
        assert!(s.is_visible(0, 0.5) && s.is_visible(1, 0.5) && !s.is_visible(2, 0.5));
        assert!(!s.is_visible(0, 1.0) && s.is_visible(1, 1.0));
        let w = Rect::new(100.0, 100.0, 300.0, 200.0);
        assert_eq!(s.placement(w, 1, 1.0).rect, w);
    }

    #[test]
    fn overview_durations_are_verified() {
        let o = OverviewZoom::default();
        assert_eq!(
            o.driver(false),
            Driver::Timed {
                duration: 0.25,
                ease: o.ease
            }
        );
        assert!(matches!(o.driver(true), Driver::Timed { duration, .. } if duration == 2.0));
        let (w, s) = (
            Rect::new(0.0, 0.0, 1000.0, 700.0),
            Rect::new(100.0, 100.0, 300.0, 200.0),
        );
        assert_eq!(o.placement(w, s, 0.0).rect, w);
        assert_eq!(o.placement(w, s, 1.0).rect, s);
        let mut a = o.animator(false, 0.0);
        a.animate_to(1.0, 0.0);
        assert!(a.is_animating(0.24) && !a.is_animating(0.26));
        let mut prev = 0.0;
        for k in 0..=50 {
            let v = a.value(k as f64 * 0.005);
            assert!(v >= prev - 1e-12);
            prev = v;
        }
    }

    #[test]
    fn server_moves_run_0_25_s_on_ease_in_ease_out() {
        let Driver::Timed { duration, ease } = server_move_driver() else {
            panic!("timed driver expected")
        };
        assert_eq!(duration, 0.25);
        assert_eq!(ease, Ease::Bezier(CubicBezier::new(0.42, 0.0, 0.58, 1.0)));
        assert_close(ease.apply(0.5), 0.5);
        assert!(ease.apply(0.25) < 0.25 && ease.apply(0.75) > 0.75);
    }

    #[test]
    fn tiling_springs_have_the_recovered_damping_and_response() {
        let t = TilingSprings::default();
        let shape = |d: Driver| {
            let Driver::Spring(p) = d else {
                panic!("spring expected")
            };
            let s = cupertino_animation::Spring::new(p);
            (s.damping_ratio(), p.perceptual_duration())
        };
        for (driver, ratio, response) in [
            (t.frame, 1.0, 0.38),
            (t.alpha, 1.0, 0.3),
            (t.corner_radius, 1.0, 0.2),
            (t.quick, 0.86, 0.15),
        ] {
            let (damping, duration) = shape(driver);
            assert_close(damping, ratio);
            assert_close(duration, response);
        }
    }

    #[test]
    fn show_desktop_spring_is_the_verified_one() {
        let d = ShowDesktop::default();
        let Driver::Spring(p) = d.driver else {
            panic!("spring expected")
        };
        let s = cupertino_animation::Spring::new(p);
        assert!((s.damping_ratio() - 1.0).abs() < 1e-12);
        assert!((p.perceptual_duration() - 0.3).abs() < 1e-12);
    }

    #[test]
    fn show_desktop_slides_to_the_nearest_horizontal_edge() {
        let d = ShowDesktop::default();
        let left = Rect::new(100.0, 200.0, 800.0, 600.0);
        let right = Rect::new(1000.0, 200.0, 800.0, 600.0);
        assert_eq!(d.hidden_rect(left, SCREEN).max_x(), 0.0);
        assert_eq!(d.hidden_rect(right, SCREEN).min_x(), 1920.0);
        assert_eq!(d.placement(left, SCREEN, 0.0).rect, left);
        assert_eq!(
            d.placement(left, SCREEN, 1.0).rect,
            d.hidden_rect(left, SCREEN)
        );
        let peek = ShowDesktop { peek: 20.0, ..d };
        assert_eq!(peek.hidden_rect(left, SCREEN).max_x(), 20.0);
        let vert = ShowDesktop {
            allow_vertical: true,
            ..d
        };
        let tall = Rect::new(400.0, 5.0, 1000.0, 700.0);
        assert_eq!(vert.hidden_rect(tall, SCREEN).max_y(), 0.0);
    }

    #[test]
    fn show_desktop_run_settles_without_overshoot() {
        let d = ShowDesktop::default();
        let mut a = d.animator(0.0);
        a.animate_to(1.0, 0.0);
        let w = Rect::new(100.0, 200.0, 800.0, 600.0);
        let mut prev_x = w.min_x() + 1.0;
        for k in 0..300 {
            let p = d.placement(w, SCREEN, a.value(k as f64 * 0.005));
            assert!(
                p.rect.min_x() <= prev_x + 1e-9
                    && p.rect.max_x() >= d.hidden_rect(w, SCREEN).max_x() - 1e-6
            );
            prev_x = p.rect.min_x();
        }
        assert!(!a.is_animating(2.0));
    }

    #[test]
    fn placements_make_quads() {
        let p = TransformKind::PopIn.placement(Rect::new(0.0, 0.0, 100.0, 100.0), 1.0);
        assert_eq!(p.mesh().bounds(), Rect::new(0.0, 0.0, 100.0, 100.0));
    }
}
