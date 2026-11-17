//! Touchpad gesture recognizer for compositor-level gestures.
//!
//! Input: libinput-like swipe, pinch and hold events (begin / update / end with a finger count and
//! per-event deltas). Output: claimed gestures with continuous `progress` and `velocity`, plus a
//! commit/cancel decision at the end.
//!
//! Mapping (the set of gestures follows the design's trackpad gestures; every number is
//! approximate and configurable, see [`GestureConfig`]):
//! * horizontal swipe → workspace switch ([`GestureKind::WorkspaceSwitch`])
//! * vertical swipe up → overview, down → app overview
//! * pinch in → app grid, spread → show desktop
//!
//! Until a gesture locks onto an axis the recognizer is [`Disposition::Pending`]; the compositor
//! should withhold (or keep forwarding) events until [`Disposition::Claimed`] or
//! [`Disposition::Ignored`] is reported.

use crate::velocity::VelocityTracker;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureEvent {
    SwipeBegin {
        time: f64,
        fingers: u32,
    },
    SwipeUpdate {
        time: f64,
        dx: f64,
        dy: f64,
    },
    SwipeEnd {
        time: f64,
        cancelled: bool,
    },
    PinchBegin {
        time: f64,
        fingers: u32,
    },
    /// `scale` is the absolute scale relative to the begin of the pinch (1.0 at the start).
    PinchUpdate {
        time: f64,
        dx: f64,
        dy: f64,
        scale: f64,
        rotation: f64,
    },
    PinchEnd {
        time: f64,
        cancelled: bool,
    },
    HoldBegin {
        time: f64,
        fingers: u32,
    },
    HoldEnd {
        time: f64,
        cancelled: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GestureKind {
    WorkspaceSwitch,
    Overview,
    AppOverview,
    AppGrid,
    ShowDesktop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Commit,
    Cancel,
}

/// Direction of a committed workspace switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureOutput {
    Begin {
        kind: GestureKind,
        fingers: u32,
    },
    /// `progress` is signed: 0 at the start, 1 for a full gesture; for workspace switches positive
    /// values move towards the next workspace. Values outside `[0, 1]` (or `[-1, 1]`) are for the
    /// consumer to rubber-band or clamp. `velocity` is in progress units per second.
    Update {
        kind: GestureKind,
        progress: f64,
        velocity: f64,
    },
    /// `target` is the progress value to settle to; `steps` is the number of workspaces for a
    /// committed workspace switch (0 otherwise).
    End {
        kind: GestureKind,
        outcome: Outcome,
        progress: f64,
        velocity: f64,
        target: f64,
        direction: Option<Direction>,
        steps: u32,
    },
    HoldBegin {
        fingers: u32,
    },
    /// A hold that ended without being cancelled (a rest of the fingers, used to stop kinetic
    /// scrolling).
    HoldEnd {
        fingers: u32,
        cancelled: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Idle,
    Pending,
    Claimed,
    Ignored,
}

#[derive(Clone, Debug)]
pub struct GestureConfig {
    /// Finger counts that switch workspaces with a horizontal swipe. approximate: 3 or 4.
    pub horizontal_fingers: Vec<u32>,
    /// Finger counts for the vertical-up (overview) and vertical-down (app overview) swipes.
    pub overview_fingers: Vec<u32>,
    pub app_overview_fingers: Vec<u32>,
    /// Finger counts for the app grid pinch and show-desktop spread.
    pub pinch_fingers: Vec<u32>,
    /// Swipe travel (libinput units) before the axis is decided. approximate.
    pub lock_distance: f64,
    /// The major axis must exceed the minor axis by this factor to lock. approximate.
    pub axis_ratio: f64,
    /// Travel for progress 1 on the horizontal axis / the vertical axis. approximate.
    pub workspace_distance: f64,
    pub overview_distance: f64,
    /// `|scale − 1| at which a pinch locks. approximate.
    pub pinch_lock: f64,
    /// Scale change for progress 1: `1 − scale` for a pinch, `scale − 1` for a spread. approximate.
    pub pinch_distance: f64,
    pub spread_distance: f64,
    /// Projected progress at which a release commits. approximate.
    pub commit_progress: f64,
    /// Seconds of velocity projected onto the release position. approximate.
    pub projection_s: f64,
    /// Largest number of workspaces one swipe may traverse. approximate.
    pub max_steps: u32,
    /// Content follows the fingers: moving the fingers left goes to the next workspace.
    pub natural_direction: bool,
    pub velocity_window: f64,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            horizontal_fingers: vec![3, 4],
            overview_fingers: vec![3, 4],
            app_overview_fingers: vec![3, 4],
            pinch_fingers: vec![4],
            lock_distance: 8.0,
            axis_ratio: 1.0,
            workspace_distance: 300.0,
            overview_distance: 250.0,
            pinch_lock: 0.08,
            pinch_distance: 0.5,
            spread_distance: 0.5,
            commit_progress: 0.5,
            projection_s: 0.15,
            max_steps: 1,
            natural_direction: true,
            velocity_window: 0.1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Source {
    Swipe,
    Pinch,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Idle,
    Pending {
        source: Source,
        fingers: u32,
        dx: f64,
        dy: f64,
    },
    Ignored,
    Claimed {
        source: Source,
        kind: GestureKind,
        fingers: u32,
        dx: f64,
        dy: f64,
        progress: f64,
    },
    Holding {
        fingers: u32,
    },
}

pub struct GestureRecognizer {
    pub cfg: GestureConfig,
    state: State,
    velocity: VelocityTracker,
}

impl GestureRecognizer {
    pub fn new(cfg: GestureConfig) -> Self {
        let velocity = VelocityTracker::new(cfg.velocity_window);
        Self {
            cfg,
            state: State::Idle,
            velocity,
        }
    }

    pub fn disposition(&self) -> Disposition {
        match self.state {
            State::Idle | State::Holding { .. } => Disposition::Idle,
            State::Pending { .. } => Disposition::Pending,
            State::Ignored => Disposition::Ignored,
            State::Claimed { .. } => Disposition::Claimed,
        }
    }

    pub fn active_kind(&self) -> Option<GestureKind> {
        match self.state {
            State::Claimed { kind, .. } => Some(kind),
            _ => None,
        }
    }

    pub fn handle(&mut self, ev: GestureEvent) -> Vec<GestureOutput> {
        let mut out = Vec::new();
        match ev {
            GestureEvent::SwipeBegin { fingers, .. } => self.begin(Source::Swipe, fingers),
            GestureEvent::PinchBegin { fingers, .. } => self.begin(Source::Pinch, fingers),
            GestureEvent::SwipeUpdate { time, dx, dy } => self.swipe_update(time, dx, dy, &mut out),
            GestureEvent::PinchUpdate { time, scale, .. } => {
                self.pinch_update(time, scale, &mut out)
            }
            GestureEvent::SwipeEnd { time, cancelled }
            | GestureEvent::PinchEnd { time, cancelled } => self.end(time, cancelled, &mut out),
            GestureEvent::HoldBegin { fingers, .. } => {
                self.state = State::Holding { fingers };
                out.push(GestureOutput::HoldBegin { fingers });
            }
            GestureEvent::HoldEnd { cancelled, .. } => {
                if let State::Holding { fingers } = self.state {
                    self.state = State::Idle;
                    out.push(GestureOutput::HoldEnd { fingers, cancelled });
                }
            }
        }
        out
    }

    fn begin(&mut self, source: Source, fingers: u32) {
        self.velocity = VelocityTracker::new(self.cfg.velocity_window);
        let any = match source {
            Source::Swipe => {
                self.cfg.horizontal_fingers.contains(&fingers)
                    || self.cfg.overview_fingers.contains(&fingers)
                    || self.cfg.app_overview_fingers.contains(&fingers)
            }
            Source::Pinch => self.cfg.pinch_fingers.contains(&fingers),
        };
        self.state = if any {
            State::Pending {
                source,
                fingers,
                dx: 0.0,
                dy: 0.0,
            }
        } else {
            State::Ignored
        };
    }

    fn swipe_update(&mut self, time: f64, ddx: f64, ddy: f64, out: &mut Vec<GestureOutput>) {
        match self.state {
            State::Pending {
                source: Source::Swipe,
                fingers,
                dx,
                dy,
            } => {
                let (dx, dy) = (dx + ddx, dy + ddy);
                if dx.hypot(dy) < self.cfg.lock_distance {
                    self.state = State::Pending {
                        source: Source::Swipe,
                        fingers,
                        dx,
                        dy,
                    };
                    return;
                }
                let r = self.cfg.axis_ratio.max(1.0);
                let kind = if dx.abs() > dy.abs() * r {
                    self.cfg
                        .horizontal_fingers
                        .contains(&fingers)
                        .then_some(GestureKind::WorkspaceSwitch)
                } else if dy.abs() > dx.abs() * r {
                    if dy < 0.0 {
                        self.cfg
                            .overview_fingers
                            .contains(&fingers)
                            .then_some(GestureKind::Overview)
                    } else {
                        self.cfg
                            .app_overview_fingers
                            .contains(&fingers)
                            .then_some(GestureKind::AppOverview)
                    }
                } else {
                    self.state = State::Pending {
                        source: Source::Swipe,
                        fingers,
                        dx,
                        dy,
                    };
                    return;
                };
                match kind {
                    Some(kind) => {
                        self.state = State::Claimed {
                            source: Source::Swipe,
                            kind,
                            fingers,
                            dx,
                            dy,
                            progress: 0.0,
                        };
                        out.push(GestureOutput::Begin { kind, fingers });
                        self.claimed_update(time, out);
                    }
                    None => self.state = State::Ignored,
                }
            }
            State::Claimed {
                source: Source::Swipe,
                kind,
                fingers,
                dx,
                dy,
                progress,
            } => {
                self.state = State::Claimed {
                    source: Source::Swipe,
                    kind,
                    fingers,
                    dx: dx + ddx,
                    dy: dy + ddy,
                    progress,
                };
                self.claimed_update(time, out);
            }
            _ => {}
        }
    }

    fn pinch_update(&mut self, time: f64, scale: f64, out: &mut Vec<GestureOutput>) {
        match self.state {
            State::Pending {
                source: Source::Pinch,
                fingers,
                ..
            } => {
                let d = scale - 1.0;
                if d.abs() < self.cfg.pinch_lock {
                    return;
                }
                let kind = if d < 0.0 {
                    GestureKind::AppGrid
                } else {
                    GestureKind::ShowDesktop
                };
                self.state = State::Claimed {
                    source: Source::Pinch,
                    kind,
                    fingers,
                    dx: scale,
                    dy: 0.0,
                    progress: 0.0,
                };
                out.push(GestureOutput::Begin { kind, fingers });
                self.claimed_update(time, out);
            }
            State::Claimed {
                source: Source::Pinch,
                kind,
                fingers,
                progress,
                ..
            } => {
                self.state = State::Claimed {
                    source: Source::Pinch,
                    kind,
                    fingers,
                    dx: scale,
                    dy: 0.0,
                    progress,
                };
                self.claimed_update(time, out);
            }
            _ => {}
        }
    }

    fn raw_progress(&self, kind: GestureKind, dx: f64, dy: f64) -> f64 {
        let c = &self.cfg;
        match kind {
            GestureKind::WorkspaceSwitch => {
                let d = if c.natural_direction { -dx } else { dx };
                d / c.workspace_distance
            }
            GestureKind::Overview => -dy / c.overview_distance,
            GestureKind::AppOverview => dy / c.overview_distance,
            GestureKind::AppGrid => (1.0 - dx) / c.pinch_distance,
            GestureKind::ShowDesktop => (dx - 1.0) / c.spread_distance,
        }
    }

    fn claimed_update(&mut self, time: f64, out: &mut Vec<GestureOutput>) {
        let State::Claimed {
            source,
            kind,
            fingers,
            dx,
            dy,
            ..
        } = self.state
        else {
            return;
        };
        let progress = self.raw_progress(kind, dx, dy);
        self.state = State::Claimed {
            source,
            kind,
            fingers,
            dx,
            dy,
            progress,
        };
        self.velocity.push(time, progress);
        out.push(GestureOutput::Update {
            kind,
            progress,
            velocity: self.velocity.velocity(time),
        });
    }

    fn end(&mut self, time: f64, cancelled: bool, out: &mut Vec<GestureOutput>) {
        let prev = std::mem::replace(&mut self.state, State::Idle);
        let State::Claimed { kind, progress, .. } = prev else {
            return;
        };
        let velocity = self.velocity.velocity(time);
        let projected = progress + velocity * self.cfg.projection_s;
        let commit_at = self.cfg.commit_progress;
        let (outcome, target, direction, steps) = if cancelled {
            (Outcome::Cancel, 0.0, None, 0)
        } else if kind == GestureKind::WorkspaceSwitch {
            if projected.abs() >= commit_at {
                let steps = (projected.abs().round() as u32).clamp(1, self.cfg.max_steps.max(1));
                let dir = if projected > 0.0 {
                    Direction::Next
                } else {
                    Direction::Previous
                };
                (
                    Outcome::Commit,
                    steps as f64 * projected.signum(),
                    Some(dir),
                    steps,
                )
            } else {
                (Outcome::Cancel, 0.0, None, 0)
            }
        } else if projected >= commit_at {
            (Outcome::Commit, 1.0, None, 0)
        } else {
            (Outcome::Cancel, 0.0, None, 0)
        };
        out.push(GestureOutput::End {
            kind,
            outcome,
            progress,
            velocity,
            target,
            direction,
            steps,
        });
    }
}

/// Smallest `|q| a fluid swipe may release with and still commit.
pub const FLUID_COMMIT_MIN_PROGRESS: f64 = 0.5;
/// Fraction of the peak progress the release projection must keep (0.8 for
/// the navigation and dock actions).
pub const FLUID_COMMIT_PEAK_FRACTION: f64 = 0.8;

/// Release decision of a fluid swipe, made by the device layer before the gesture reaches the
/// shell (rule, the call site that applies it to the navigation and dock actions
/// is ).
///
/// `progress` is the integrated travel over the complete distance (signed), `peak` the
/// largest-magnitude progress reached in the current direction, `rate` the filtered progress rate
/// in progress per second and `inertia_s` the swipe's inertia time. The projection is
/// `q = inertia_s · rate + progress`; the swipe commits when `progress` and `q` agree in sign,
/// `|q| > 0.5` and `|q| ≥ 0.8 · |peak|. The one-sided variants used by the edge swipe (weight
/// 0.25 against the travel, fraction 0.6) are not modelled.
pub fn fluid_swipe_commits(progress: f64, peak: f64, rate: f64, inertia_s: f64) -> bool {
    let q = inertia_s * rate + progress;
    progress * q > 0.0
        && q.abs() > FLUID_COMMIT_MIN_PROGRESS
        && q.abs() >= FLUID_COMMIT_PEAK_FRACTION * peak.abs()
}
