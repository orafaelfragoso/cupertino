//! The idle policy of the display and the system: dim, display sleep, the lock that follows display sleep, and system
//! sleep, from the time of the last input. Pure: the caller passes the model time (seconds) and the facts it knows
//! (power source, inhibitors, lock), and carries out the [`Effect`]s; `power.rs` is that caller.
//!
//! The sequence is the design's display wrangler: with a display sleep timer `T`, the display dims
//! at `last input + T - 15 s` and sleeps at `last input + T`. Input within 20 s of the dimming makes the next countdown
//! `T2 = max(min(2 T, 600 s), T) until the display next sleeps. Dimming ramps the display factor from 1 to 0.01 over
//! 0.26667 s and undimming ramps it back over the same time; sleeping and waking switch it at once.
//! Display sleep locks the session after the password delay. Timers are in [`PowerSettings`].

use cupertino_defaults::power::{PowerSettings, Timer};

/// The display dims this long before it sleeps.
pub const DIM_WINDOW_S: f64 = 15.0;
/// Input this soon after the dimming lengthens the next countdown to `T2` (the 0x14 s timer that
/// arms when the display leaves full power, read by).
pub const DIM_ACTIVITY_WINDOW_S: f64 = 20.0;
/// Upper bound of the lengthened countdown, unless `T` itself is longer.
pub const LENGTHENED_CAP_S: f64 = 600.0;
/// Duration of the dim and undim ramps (`ON_TO_DIM_RAMP_TIME`, `DIM_TO_ON_RAMP_TIME`).
pub const RAMP_S: f64 = 0.26667;
/// Display factor while dimmed.
pub const DIM_FACTOR: f64 = 0.01;

/// What the display shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayState {
    On,
    Dimmed,
    Asleep,
}

/// What the caller must do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The display entered `DisplayState`; the factor ramp (if any) is in [`IdlePolicy::factor`].
    Display(DisplayState),
    /// Lock the session: the display has slept for the password delay.
    Lock,
    /// Put the system to sleep.
    SystemSleep,
}

/// What the caller knows at a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub on_battery: bool,
    /// A client or the login manager holds back idle actions (`zwp_idle_inhibitor_v1`, a logind `idle` block).
    pub inhibited: bool,
    pub locked: bool,
    /// A lock screen can be asked to lock the session; without one (the login screen's own server) display sleep
    /// never asks for a lock.
    pub can_lock: bool,
}

/// A ramp of the display factor.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ramp {
    from: f64,
    to: f64,
    start: f64,
}

impl Ramp {
    fn at(&self, now: f64) -> f64 {
        let t = ((now - self.start) / RAMP_S).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * raised_cosine(t)
    }

    fn done(&self, now: f64) -> bool {
        now - self.start >= RAMP_S
    }
}

/// The fade curve of the design's display fades, `(cos((t + 1) pi) + 1) / 2` (for the display
/// fades). The curve of the dim ramp itself was not read; using this one for it is approximate.
pub fn raised_cosine(t: f64) -> f64 {
    ((t + 1.0) * std::f64::consts::PI).cos() * 0.5 + 0.5
}

/// Black overlay alpha that shows a display factor `f` (linear light, like the backlight factor it stands for) over
/// sRGB-encoded pixels: `1 - encode(f) with the sRGB transfer function (exact for black and white, close for
/// the other colours, since the overlay scales encoded values).
pub fn overlay_alpha(factor: f64) -> f64 {
    let f = factor.clamp(0.0, 1.0);
    let encoded = if f <= 0.003_130_8 {
        12.92 * f
    } else {
        1.055 * f.powf(1.0 / 2.4) - 0.055
    };
    (1.0 - encoded).clamp(0.0, 1.0)
}

/// The policy's state.
#[derive(Debug)]
pub struct IdlePolicy {
    settings: PowerSettings,
    last_input: f64,
    state: DisplayState,
    /// When the display entered its state.
    since: f64,
    /// The next countdown is `T2`.
    lengthened: bool,
    ramp: Option<Ramp>,
    lock_sent: bool,
    sleep_sent: bool,
    /// The session is in the background (another console has the seat): nothing counts down.
    paused: bool,
}

impl IdlePolicy {
    pub fn new(settings: PowerSettings, now: f64) -> Self {
        Self {
            settings,
            last_input: now,
            state: DisplayState::On,
            since: now,
            lengthened: false,
            ramp: None,
            lock_sent: false,
            sleep_sent: false,
            paused: false,
        }
    }

    /// Take new timers (`power.conf` changed): the countdown in progress continues with them.
    pub fn set_settings(&mut self, settings: PowerSettings) {
        self.settings = settings;
    }

    pub fn settings(&self) -> &PowerSettings {
        &self.settings
    }

    pub fn state(&self) -> DisplayState {
        self.state
    }

    /// Input arrived: wake or undim the display and start the countdowns again. While paused it changes nothing:
    /// the countdowns start afresh at [`IdlePolicy::resume`].
    pub fn input(&mut self, now: f64) -> Vec<Effect> {
        if self.paused {
            return Vec::new();
        }
        self.restart(now, true)
    }

    /// The session went to the background (another session has the seat and the display): until
    /// [`IdlePolicy::resume`] nothing counts down, and the display is never dimmed or put to sleep, the session never
    /// locked and the system never put to sleep for idleness here. The display keeps the state it had.
    pub fn pause(&mut self) {
        self.paused = true;
        self.ramp = None;
    }

    /// Whether the session is in the background ([`IdlePolicy::pause`]).
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The session is the active one again at `now`: the display is on at once, whatever it was when the session
    /// went to the background, and every countdown starts afresh, the lengthened one too.
    pub fn resume(&mut self, now: f64) -> Vec<Effect> {
        self.paused = false;
        self.last_input = now;
        self.lock_sent = false;
        self.sleep_sent = false;
        self.lengthened = false;
        self.ramp = None;
        match self.state {
            DisplayState::On => Vec::new(),
            DisplayState::Dimmed | DisplayState::Asleep => self.enter(DisplayState::On, now),
        }
    }

    /// Start the countdowns at `now` and bring the display back on. Only user input counts towards the lengthened
    /// countdown.
    fn restart(&mut self, now: f64, user: bool) -> Vec<Effect> {
        self.last_input = now;
        self.lock_sent = false;
        self.sleep_sent = false;
        match self.state {
            DisplayState::On => Vec::new(),
            DisplayState::Dimmed => {
                if user && now - self.since < DIM_ACTIVITY_WINDOW_S {
                    self.lengthened = true;
                }
                self.ramp = Some(Ramp {
                    from: self.factor(now),
                    to: 1.0,
                    start: now,
                });
                self.enter(DisplayState::On, now)
            }
            DisplayState::Asleep => {
                self.ramp = None;
                self.enter(DisplayState::On, now)
            }
        }
    }

    /// Advance to `now`; returns what changed.
    pub fn tick(&mut self, now: f64, facts: Facts) -> Vec<Effect> {
        if self.ramp.is_some_and(|r| r.done(now)) {
            self.ramp = None;
        }
        if self.paused {
            return Vec::new();
        }
        // an inhibitor holds every countdown at its start, as continuous input would
        if facts.inhibited {
            return self.restart(now, false);
        }
        let mut effects = Vec::new();
        let timers = self.settings.timers(facts.on_battery);
        let display_sleep = if facts.locked {
            Some(self.settings.lock_screen_display_sleep.as_secs_f64())
        } else {
            timers.display_sleep.duration().map(|d| d.as_secs_f64())
        };
        if let Some(t) = display_sleep.map(|t| {
            if self.lengthened {
                t.max((2.0 * t).min(LENGTHENED_CAP_S))
            } else {
                t
            }
        }) {
            let sleep_at = self.last_input + t;
            let dim_at = self.last_input + (t - DIM_WINDOW_S).max(0.0);
            if self.state == DisplayState::On
                && self.settings.dim_before_display_sleep
                && now >= dim_at
                && now < sleep_at
            {
                self.ramp = Some(Ramp {
                    from: self.factor(now),
                    to: DIM_FACTOR,
                    start: now,
                });
                effects.extend(self.enter(DisplayState::Dimmed, now));
            }
            if self.state != DisplayState::Asleep && now >= sleep_at {
                self.ramp = None;
                self.lengthened = false;
                effects.extend(self.enter(DisplayState::Asleep, now));
            }
        }
        if let (DisplayState::Asleep, Timer::After(delay), false, false, true) = (
            self.state,
            self.settings.require_password_after,
            facts.locked,
            self.lock_sent,
            facts.can_lock,
        ) {
            if now >= self.since + delay.as_secs_f64() {
                self.lock_sent = true;
                effects.push(Effect::Lock);
            }
        }
        if let (Some(s), false) = (timers.system_sleep.duration(), self.sleep_sent) {
            if now >= self.last_input + s.as_secs_f64() {
                self.sleep_sent = true;
                effects.push(Effect::SystemSleep);
            }
        }
        effects
    }

    /// The display factor at `now`: 1 on, 0.01 dimmed, 0 asleep, with the ramp in between.
    pub fn factor(&self, now: f64) -> f64 {
        match (self.ramp, self.state) {
            (Some(r), _) => r.at(now),
            (None, DisplayState::On) => 1.0,
            (None, DisplayState::Dimmed) => DIM_FACTOR,
            (None, DisplayState::Asleep) => 0.0,
        }
    }

    /// A ramp is running, so every frame differs.
    pub fn animating(&self, now: f64) -> bool {
        self.ramp.is_some_and(|r| !r.done(now))
    }

    /// When [`IdlePolicy::tick`] next has something to do under `facts`: dimming, display sleep, the lock after sleep
    /// or system sleep. `None` while an inhibitor holds every countdown, while the session is paused, or when nothing is
    /// left to count down.
    pub fn next_deadline(&self, facts: Facts) -> Option<f64> {
        if facts.inhibited || self.paused {
            return None;
        }
        let timers = self.settings.timers(facts.on_battery);
        let mut next: Option<f64> = None;
        let mut at = |t: f64| next = Some(next.map_or(t, |n: f64| n.min(t)));
        let display_sleep = if facts.locked {
            Some(self.settings.lock_screen_display_sleep.as_secs_f64())
        } else {
            timers.display_sleep.duration().map(|d| d.as_secs_f64())
        };
        if let Some(t) = display_sleep.map(|t| {
            if self.lengthened {
                t.max((2.0 * t).min(LENGTHENED_CAP_S))
            } else {
                t
            }
        }) {
            if self.state == DisplayState::On && self.settings.dim_before_display_sleep {
                at(self.last_input + (t - DIM_WINDOW_S).max(0.0));
            }
            if self.state != DisplayState::Asleep {
                at(self.last_input + t);
            }
        }
        if let (DisplayState::Asleep, Timer::After(delay), false, false, true) = (
            self.state,
            self.settings.require_password_after,
            facts.locked,
            self.lock_sent,
            facts.can_lock,
        ) {
            at(self.since + delay.as_secs_f64());
        }
        if let (Some(s), false) = (timers.system_sleep.duration(), self.sleep_sent) {
            at(self.last_input + s.as_secs_f64());
        }
        next
    }

    fn enter(&mut self, state: DisplayState, now: f64) -> Vec<Effect> {
        self.state = state;
        self.since = now;
        vec![Effect::Display(state)]
    }
}
