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
    /// Appearance change, 0.5 s: the window as it was before the change fades out over the redrawn one on
    /// [`APPEARANCE_FADE_CURVE`], without scaling.
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
/// How the appearance crossfade advances: a quick start that settles slowly, half done before a third of the way in.
pub const APPEARANCE_FADE_CURVE: CubicBezier = CubicBezier::DEFAULT;
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
                    ease: if self == TransformKind::AppearanceFade {
                        Ease::Bezier(APPEARANCE_FADE_CURVE)
                    } else {
                        Ease::Cosine
                    },
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
