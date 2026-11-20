//! Layer animations a gesture drives (`cupertino_transaction_v1.bind_gesture`). When the recogniser claims the gesture,
//! the animations the transaction's parts start are tracked: each shows its start value plus the gesture's progress
//! times the way to its target, whatever its curve, and one through samples shows the sample at the progress. The
//! progress is shared ([`GestureProgress`]): the transaction moves it with the fingers and along the binding's spring
//! when they lift, and every tracked animation reads it as its frame is built, so nothing is sent per frame. Under
//! reduced motion the progress drives opacity alone: a spatial property keeps its start value until the progress comes
//! to rest at 1 and takes its target there.
//!
//! At rest at 1 a tracked animation has finished; while its transaction keeps the progress for a reverse gesture it stays
//! tracked, so that gesture drives it back. Once the transaction lets the progress go, a property at rest at 1 keeps its
//! target and one anywhere else stays where it shows, its animation answered `replaced` there: back at its start when
//! the gesture reverted ([`Cupertino::tick_layers`](crate::state::Cupertino::tick_layers)).

use std::sync::{Arc, Mutex, MutexGuard};

use cupertino_animation::{LayerCurve, LayerProperty};

/// Where a gesture's progress is.
#[derive(Clone, Debug)]
enum Phase {
    /// The fingers are on the touchpad, at this progress.
    Tracking(f64),
    /// The fingers lifted: the progress moves along the binding's spring.
    Settling(Box<LayerProperty>),
    /// At rest, at 0 or 1.
    Rest(f64),
}

#[derive(Debug)]
struct State {
    phase: Phase,
    /// The progress last came to rest at 1: what a spatial property shows under reduced motion.
    rested_at_one: bool,
    /// The transaction still drives its animations.
    bound: bool,
}

/// The progress (0 where the gesture began, 1 for a whole gesture) of one gesture-bound transaction: moved by the
/// transaction, read by the layer animations it tracks.
#[derive(Clone, Debug)]
pub struct GestureProgress(Arc<Mutex<State>>);

impl Default for GestureProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl GestureProgress {
    /// A progress at rest at 0, driving nothing yet.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(State {
            phase: Phase::Rest(0.0),
            rested_at_one: false,
            bound: true,
        })))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The fingers are at `progress`.
    pub fn track(&self, progress: f64) {
        self.state().phase = Phase::Tracking(progress);
    }

    /// The fingers lifted at `now` (model seconds): the progress moves to `to` along the spring of `response` and
    /// `damping`, from what it shows and the gesture's `velocity` (progress per second).
    pub fn settle(&self, to: f64, response: f64, damping: f64, now: f64, velocity: f64) {
        let mut state = self.state();
        let mut spring = LayerProperty::new(phase_value(&state.phase, now));
        spring.animate(to, LayerCurve::Spring { response, damping }, now, velocity);
        state.phase = Phase::Settling(Box::new(spring));
    }

    /// Come to rest at `at`, 0 or 1.
    pub fn rest(&self, at: f64) {
        let mut state = self.state();
        state.phase = Phase::Rest(at);
        state.rested_at_one = at >= 0.5;
    }

    /// The transaction no longer drives its animations: each stays where it shows ([`GestureProgress`]).
    pub fn let_go(&self) {
        self.state().bound = false;
    }

    /// The progress shown at `now`.
    pub fn value(&self, now: f64) -> f64 {
        phase_value(&self.state().phase, now)
    }

    /// The progress's velocity at `now`, per second: the spring's while it settles, 0 otherwise.
    pub fn velocity(&self, now: f64) -> f64 {
        match &self.state().phase {
            Phase::Settling(spring) => spring.velocity(now),
            Phase::Tracking(_) | Phase::Rest(_) => 0.0,
        }
    }

    /// The target of the spring, once it came to rest at `now`; `None` while it moves or when it does not settle.
    pub fn settled(&self, now: f64) -> Option<f64> {
        match &self.state().phase {
            Phase::Settling(spring) if !spring.is_animating(now) => Some(spring.target()),
            _ => None,
        }
    }

    /// When the settling spring comes to rest, in model seconds; `None` when it is not settling.
    pub fn settle_end(&self) -> Option<f64> {
        match &self.state().phase {
            Phase::Settling(spring) => Some(spring.end().unwrap_or(f64::NEG_INFINITY)),
            _ => None,
        }
    }

    /// The value the progress rests at, `None` while the fingers move it or it settles.
    pub fn at_rest(&self) -> Option<f64> {
        match self.state().phase {
            Phase::Rest(at) => Some(at),
            _ => None,
        }
    }

    /// Whether the progress moves: the fingers are on the touchpad or it settles.
    pub fn moving(&self) -> bool {
        !matches!(self.state().phase, Phase::Rest(_))
    }

    /// Whether the transaction still drives its animations.
    pub fn bound(&self) -> bool {
        self.state().bound
    }

    /// Whether the progress last came to rest at 1.
    pub fn rested_at_one(&self) -> bool {
        self.state().rested_at_one
    }

    /// Whether `other` is this same progress.
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

fn phase_value(phase: &Phase, now: f64) -> f64 {
    match phase {
        Phase::Tracking(p) | Phase::Rest(p) => *p,
        Phase::Settling(spring) => spring.value(now),
    }
}

/// An animation of a layer property a gesture drives.
#[derive(Clone, Debug)]
pub(super) struct Tracked {
    pub(super) progress: GestureProgress,
    /// The value the property showed when the animation started, and its target.
    pub(super) from: f64,
    pub(super) to: f64,
    /// An animation through samples: their motion over a unit duration from 0, read at the progress.
    samples: Option<LayerProperty>,
    /// Reduced motion and a spatial property: the start value until the progress rests at 1, the target there.
    still: bool,
}

impl Tracked {
    pub(super) fn new(
        progress: GestureProgress,
        from: f64,
        to: f64,
        samples: Option<Arc<[f32]>>,
        still: bool,
    ) -> Self {
        let samples = samples.map(|values| {
            let mut motion = LayerProperty::new(from);
            motion.animate_samples(values, 1.0, 0.0);
            motion
        });
        let (from, to) = samples
            .as_ref()
            .map_or((from, to), |s| (s.value(0.0), s.target()));
        Self {
            progress,
            from,
            to,
            samples,
            still,
        }
    }

    /// The value shown at `now`.
    pub(super) fn value(&self, now: f64) -> f64 {
        if self.still {
            return if self.progress.rested_at_one() {
                self.to
            } else {
                self.from
            };
        }
        let p = self.progress.value(now);
        match &self.samples {
            Some(samples) => samples.value(p),
            None => self.from + p * (self.to - self.from),
        }
    }

    /// The velocity shown at `now`, units per second.
    pub(super) fn velocity(&self, now: f64) -> f64 {
        if self.still {
            return 0.0;
        }
        let dp = self.progress.velocity(now);
        match &self.samples {
            Some(samples) => samples.velocity(self.progress.value(now)) * dp,
            None => dp * (self.to - self.from),
        }
    }
}
