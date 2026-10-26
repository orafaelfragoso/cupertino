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

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> GestureRecognizer {
        GestureRecognizer::new(GestureConfig::default())
    }

    fn swipe(
        r: &mut GestureRecognizer,
        fingers: u32,
        steps: &[(f64, f64)],
        dt: f64,
        cancelled: bool,
    ) -> Vec<GestureOutput> {
        let mut all = r.handle(GestureEvent::SwipeBegin { time: 0.0, fingers });
        let mut t = 0.0;
        for &(dx, dy) in steps {
            t += dt;
            all.extend(r.handle(GestureEvent::SwipeUpdate { time: t, dx, dy }));
        }
        all.extend(r.handle(GestureEvent::SwipeEnd {
            time: t + dt,
            cancelled,
        }));
        all
    }

    fn last_end(v: &[GestureOutput]) -> GestureOutput {
        *v.iter()
            .rev()
            .find(|o| matches!(o, GestureOutput::End { .. }))
            .expect("end")
    }

    #[test]
    fn horizontal_three_finger_swipe_switches_workspace() {
        let mut r = rec();
        let out = swipe(&mut r, 3, &[(-40.0, 1.0); 8], 0.016, false);
        assert!(matches!(
            out[0],
            GestureOutput::Begin {
                kind: GestureKind::WorkspaceSwitch,
                fingers: 3
            }
        ));
        let GestureOutput::End {
            outcome,
            direction,
            steps,
            target,
            ..
        } = last_end(&out)
        else {
            unreachable!()
        };
        assert_eq!(
            (outcome, direction, steps),
            (Outcome::Commit, Some(Direction::Next), 1)
        );
        assert_eq!(target, 1.0);
        assert_eq!(r.disposition(), Disposition::Idle);
    }

    #[test]
    fn swipe_right_goes_to_previous_and_natural_flag_flips_it() {
        let mut r = rec();
        let out = swipe(&mut r, 4, &[(60.0, 0.0); 6], 0.016, false);
        let GestureOutput::End {
            direction, target, ..
        } = last_end(&out)
        else {
            unreachable!()
        };
        assert_eq!(direction, Some(Direction::Previous));
        assert_eq!(target, -1.0);
        let mut r = GestureRecognizer::new(GestureConfig {
            natural_direction: false,
            ..Default::default()
        });
        let out = swipe(&mut r, 4, &[(60.0, 0.0); 6], 0.016, false);
        let GestureOutput::End { direction, .. } = last_end(&out) else {
            unreachable!()
        };
        assert_eq!(direction, Some(Direction::Next));
    }

    #[test]
    fn vertical_up_opens_overview_and_down_app_overview() {
        let mut r = rec();
        let out = swipe(&mut r, 3, &[(0.0, -50.0); 6], 0.016, false);
        assert!(matches!(
            out[0],
            GestureOutput::Begin {
                kind: GestureKind::Overview,
                ..
            }
        ));
        let GestureOutput::End {
            outcome,
            target,
            kind,
            ..
        } = last_end(&out)
        else {
            unreachable!()
        };
        assert_eq!(
            (kind, outcome, target),
            (GestureKind::Overview, Outcome::Commit, 1.0)
        );

        let out = swipe(&mut r, 3, &[(0.0, 50.0); 6], 0.016, false);
        assert!(matches!(
            out[0],
            GestureOutput::Begin {
                kind: GestureKind::AppOverview,
                ..
            }
        ));
        let GestureOutput::End { outcome, .. } = last_end(&out) else {
            unreachable!()
        };
        assert_eq!(outcome, Outcome::Commit);
    }

    #[test]
    fn progress_and_velocity_follow_the_fingers() {
        let mut r = rec();
        let mut last = None;
        r.handle(GestureEvent::SwipeBegin {
            time: 0.0,
            fingers: 3,
        });
        for i in 1..=10 {
            for o in r.handle(GestureEvent::SwipeUpdate {
                time: i as f64 * 0.01,
                dx: 0.0,
                dy: -10.0,
            }) {
                if let GestureOutput::Update {
                    progress, velocity, ..
                } = o
                {
                    last = Some((progress, velocity));
                }
            }
        }
        let (p, v) = last.unwrap();
        assert!((p - 100.0 / 250.0).abs() < 1e-9);
        assert!((v - 1000.0 / 250.0).abs() < 0.2, "velocity {v}");
    }

    #[test]
    fn short_slow_swipe_cancels_but_a_flick_commits() {
        let mut r = rec();
        let out = swipe(&mut r, 3, &[(0.0, -3.0); 5], 0.1, false);
        let GestureOutput::End {
            outcome, target, ..
        } = last_end(&out)
        else {
            unreachable!()
        };
        assert_eq!((outcome, target), (Outcome::Cancel, 0.0));
        let mut r = rec();
        let out = swipe(&mut r, 3, &[(0.0, -12.0); 4], 0.008, false);
        let GestureOutput::End { outcome, .. } = last_end(&out) else {
            unreachable!()
        };
        assert_eq!(
            outcome,
            Outcome::Commit,
            "fast flick projects past the threshold"
        );
    }

    #[test]
    fn reversing_before_release_cancels() {
        let mut r = rec();
        let mut steps = vec![(0.0, -40.0); 4];
        steps.extend(vec![(0.0, 40.0); 4]);
        let out = swipe(&mut r, 3, &steps, 0.05, false);
        let GestureOutput::End {
            outcome, progress, ..
        } = last_end(&out)
        else {
            unreachable!()
        };
        assert_eq!(outcome, Outcome::Cancel);
        assert!(progress.abs() < 0.05);
    }

    #[test]
    fn libinput_cancel_cancels() {
        let mut r = rec();
        let out = swipe(&mut r, 3, &[(-80.0, 0.0); 6], 0.016, true);
        let GestureOutput::End { outcome, .. } = last_end(&out) else {
            unreachable!()
        };
        assert_eq!(outcome, Outcome::Cancel);
    }

    #[test]
    fn unsupported_finger_counts_are_ignored() {
        let mut r = rec();
        assert!(swipe(&mut r, 2, &[(-80.0, 0.0); 6], 0.016, false).is_empty());
        r.handle(GestureEvent::SwipeBegin {
            time: 0.0,
            fingers: 5,
        });
        assert_eq!(r.disposition(), Disposition::Ignored);
        let cfg = GestureConfig {
            horizontal_fingers: vec![4],
            ..Default::default()
        };
        let mut r = GestureRecognizer::new(cfg);
        assert!(
            swipe(&mut r, 3, &[(-80.0, 0.0); 6], 0.016, false).is_empty(),
            "3 fingers no longer switch workspaces"
        );
        assert!(!swipe(&mut r, 4, &[(-80.0, 0.0); 6], 0.016, false).is_empty());
    }

    #[test]
    fn pending_until_the_lock_distance_and_diagonals_wait() {
        let mut r = rec();
        r.handle(GestureEvent::SwipeBegin {
            time: 0.0,
            fingers: 3,
        });
        assert_eq!(r.disposition(), Disposition::Pending);
        assert!(r
            .handle(GestureEvent::SwipeUpdate {
                time: 0.01,
                dx: 2.0,
                dy: 2.0
            })
            .is_empty());
        assert!(
            r.handle(GestureEvent::SwipeUpdate {
                time: 0.02,
                dx: 6.0,
                dy: 6.0
            })
            .is_empty(),
            "perfect diagonal never locks"
        );
        let out = r.handle(GestureEvent::SwipeUpdate {
            time: 0.03,
            dx: 20.0,
            dy: 0.0,
        });
        assert!(matches!(
            out[0],
            GestureOutput::Begin {
                kind: GestureKind::WorkspaceSwitch,
                ..
            }
        ));
        assert_eq!(r.disposition(), Disposition::Claimed);
        assert_eq!(r.active_kind(), Some(GestureKind::WorkspaceSwitch));
    }

    #[test]
    fn end_before_lock_emits_nothing() {
        let mut r = rec();
        assert!(swipe(&mut r, 3, &[(1.0, 0.0)], 0.01, false).is_empty());
        assert_eq!(r.disposition(), Disposition::Idle);
    }

    #[test]
    fn axis_ratio_demands_a_clear_axis() {
        let cfg = GestureConfig {
            axis_ratio: 2.0,
            ..Default::default()
        };
        let mut r = GestureRecognizer::new(cfg);
        r.handle(GestureEvent::SwipeBegin {
            time: 0.0,
            fingers: 3,
        });
        assert!(r
            .handle(GestureEvent::SwipeUpdate {
                time: 0.01,
                dx: 15.0,
                dy: 10.0
            })
            .is_empty());
        let out = r.handle(GestureEvent::SwipeUpdate {
            time: 0.02,
            dx: 30.0,
            dy: 0.0,
        });
        assert!(!out.is_empty());
    }

    #[test]
    fn max_steps_allows_multi_workspace_swipes() {
        let cfg = GestureConfig {
            max_steps: 3,
            ..Default::default()
        };
        let mut r = GestureRecognizer::new(cfg);
        let out = swipe(&mut r, 3, &[(-100.0, 0.0); 8], 0.016, false);
        let GestureOutput::End { steps, target, .. } = last_end(&out) else {
            unreachable!()
        };
        assert!((2..=3).contains(&steps));
        assert_eq!(target, steps as f64);
    }

    fn pinch(
        r: &mut GestureRecognizer,
        fingers: u32,
        scales: &[f64],
        cancelled: bool,
    ) -> Vec<GestureOutput> {
        let mut all = r.handle(GestureEvent::PinchBegin { time: 0.0, fingers });
        let mut t = 0.0;
        for &s in scales {
            t += 0.016;
            all.extend(r.handle(GestureEvent::PinchUpdate {
                time: t,
                dx: 0.0,
                dy: 0.0,
                scale: s,
                rotation: 0.0,
            }));
        }
        all.extend(r.handle(GestureEvent::PinchEnd {
            time: t + 0.016,
            cancelled,
        }));
        all
    }

    #[test]
    fn pinch_in_opens_app_grid_and_spread_shows_desktop() {
        let mut r = rec();
        let out = pinch(&mut r, 4, &[0.95, 0.9, 0.8, 0.7, 0.6, 0.55], false);
        assert!(matches!(
            out[0],
            GestureOutput::Begin {
                kind: GestureKind::AppGrid,
                fingers: 4
            }
        ));
        let GestureOutput::End { outcome, kind, .. } = last_end(&out) else {
            unreachable!()
        };
        assert_eq!((kind, outcome), (GestureKind::AppGrid, Outcome::Commit));
        let out = pinch(&mut r, 4, &[1.05, 1.1, 1.2, 1.3, 1.4, 1.5], false);
        let GestureOutput::End { outcome, kind, .. } = last_end(&out) else {
            unreachable!()
        };
        assert_eq!((kind, outcome), (GestureKind::ShowDesktop, Outcome::Commit));
    }

    #[test]
    fn small_pinch_never_locks_and_wrong_fingers_are_ignored() {
        let mut r = rec();
        assert!(pinch(&mut r, 4, &[0.99, 1.01, 0.98], false).is_empty());
        assert!(pinch(&mut r, 2, &[0.5, 0.4], false).is_empty());
    }

    #[test]
    fn pinch_progress_is_normalised_by_the_configured_distance() {
        let mut r = rec();
        let out = pinch(&mut r, 4, &[0.9, 0.75], false);
        let p = out
            .iter()
            .rev()
            .find_map(|o| {
                if let GestureOutput::Update { progress, .. } = o {
                    Some(*progress)
                } else {
                    None
                }
            })
            .unwrap();
        assert!((p - 0.5).abs() < 1e-9);
    }

    #[test]
    fn hold_events_pass_through() {
        let mut r = rec();
        let b = r.handle(GestureEvent::HoldBegin {
            time: 0.0,
            fingers: 2,
        });
        assert_eq!(b, vec![GestureOutput::HoldBegin { fingers: 2 }]);
        let e = r.handle(GestureEvent::HoldEnd {
            time: 0.2,
            cancelled: false,
        });
        assert_eq!(
            e,
            vec![GestureOutput::HoldEnd {
                fingers: 2,
                cancelled: false
            }]
        );
        assert!(r
            .handle(GestureEvent::HoldEnd {
                time: 0.3,
                cancelled: false
            })
            .is_empty());
    }

    #[test]
    fn new_begin_resets_a_stuck_gesture() {
        let mut r = rec();
        r.handle(GestureEvent::SwipeBegin {
            time: 0.0,
            fingers: 3,
        });
        r.handle(GestureEvent::SwipeUpdate {
            time: 0.01,
            dx: -30.0,
            dy: 0.0,
        });
        assert_eq!(r.disposition(), Disposition::Claimed);
        r.handle(GestureEvent::SwipeBegin {
            time: 1.0,
            fingers: 4,
        });
        assert_eq!(r.disposition(), Disposition::Pending);
    }
}
