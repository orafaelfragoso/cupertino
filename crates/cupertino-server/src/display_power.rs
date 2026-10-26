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

/// How long the screen takes to fade in from black when this session becomes the active one
/// (the design's session transition is a 0.25 s fade; the fade is approximate here, see [`ActivationFade`]).
pub const ACTIVATION_FADE_S: f64 = 0.25;

/// The fade from black after logind resumes this session (fast user switching, a console switch back). The design cross-fades
/// the old session's snapshot into the new one with `e = 0.5 (1 - cos(pi p))` over [`ACTIVATION_FADE_S`]; each session
/// here is a separate compositor on its own terminal and has no image of the old one, so approximate: the compositor that
/// becomes active fades in from black with the same duration and curve.
#[derive(Debug, Default)]
pub struct ActivationFade {
    started: Option<f64>,
}

impl ActivationFade {
    /// The session became active at `now`: start (or restart) the fade.
    pub fn begin(&mut self, now: f64) {
        self.started = Some(now);
    }

    /// Alpha of the black overlay at `now`: 1 at the start, `1 - e` with `e = 0.5 (1 - cos(pi p))`, 0 from the end.
    pub fn alpha(&self, now: f64) -> f64 {
        let Some(started) = self.started else {
            return 0.0;
        };
        let p = ((now - started) / ACTIVATION_FADE_S).clamp(0.0, 1.0);
        1.0 - 0.5 * (1.0 - (std::f64::consts::PI * p).cos())
    }

    /// The fade is running, so every frame differs.
    pub fn animating(&self, now: f64) -> bool {
        self.started
            .is_some_and(|started| now - started < ACTIVATION_FADE_S)
    }
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

    /// Input arrived: wake or undim the display and start the countdowns again.
    pub fn input(&mut self, now: f64) -> Vec<Effect> {
        self.restart(now, true)
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
        if let (DisplayState::Asleep, Timer::After(delay), false, false) = (
            self.state,
            self.settings.require_password_after,
            facts.locked,
            self.lock_sent,
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

    fn enter(&mut self, state: DisplayState, now: f64) -> Vec<Effect> {
        self.state = state;
        self.since = now;
        vec![Effect::Display(state)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const AC: Facts = Facts {
        on_battery: false,
        inhibited: false,
        locked: false,
    };
    const BATTERY: Facts = Facts {
        on_battery: true,
        ..AC
    };

    fn settings() -> PowerSettings {
        PowerSettings {
            require_password_after: Timer::Never,
            ..PowerSettings::default()
        }
    }

    /// Tick every 0.1 s from `from` to `to` and collect the effects with their times.
    fn run(policy: &mut IdlePolicy, from: f64, to: f64, facts: Facts) -> Vec<(f64, Effect)> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= to + 1e-9 {
            out.extend(policy.tick(t, facts).into_iter().map(|e| (t, e)));
            t = ((t + 0.1) * 10.0).round() / 10.0;
        }
        out
    }

    #[test]
    fn the_activation_fade_runs_from_black_to_clear_on_a_cosine_over_a_quarter_second() {
        let mut fade = ActivationFade::default();
        assert_eq!(
            (fade.alpha(5.0), fade.animating(5.0)),
            (0.0, false),
            "no fade before a resume"
        );
        fade.begin(10.0);
        assert!((fade.alpha(10.0) - 1.0).abs() < 1e-12);
        assert!(
            (fade.alpha(10.125) - 0.5).abs() < 1e-12,
            "the middle of the curve"
        );
        assert!(
            (fade.alpha(10.0625) - (1.0 - 0.5 * (1.0 - (std::f64::consts::PI / 4.0).cos()))).abs()
                < 1e-12
        );
        assert!(fade.animating(10.24));
        assert!((fade.alpha(10.25)).abs() < 1e-12 && !fade.animating(10.25));
        assert_eq!(fade.alpha(11.0), 0.0);
        // a second resume starts again
        fade.begin(20.0);
        assert!((fade.alpha(20.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn on_mains_the_display_dims_15_s_before_it_sleeps_at_10_minutes_and_the_system_sleeps_with_it()
    {
        let mut p = IdlePolicy::new(settings(), 0.0);
        let effects = run(&mut p, 0.0, 601.0, AC);
        assert_eq!(
            effects,
            vec![
                (585.0, Effect::Display(DisplayState::Dimmed)),
                (600.0, Effect::Display(DisplayState::Asleep)),
                (600.0, Effect::SystemSleep)
            ]
        );
    }

    #[test]
    fn on_battery_the_display_sleeps_at_2_minutes() {
        let mut p = IdlePolicy::new(settings(), 0.0);
        let effects = run(&mut p, 0.0, 121.0, BATTERY);
        assert_eq!(
            effects,
            vec![
                (105.0, Effect::Display(DisplayState::Dimmed)),
                (120.0, Effect::Display(DisplayState::Asleep))
            ]
        );
    }

    #[test]
    fn the_dim_ramps_to_the_dim_factor_and_input_ramps_back_while_sleep_and_wake_switch_at_once() {
        let mut p = IdlePolicy::new(settings(), 0.0);
        run(&mut p, 0.0, 105.0, BATTERY);
        assert_eq!(p.state(), DisplayState::Dimmed);
        assert!((p.factor(105.0) - 1.0).abs() < 1e-9);
        let mid = p.factor(105.0 + RAMP_S / 2.0);
        assert!(
            (mid - (1.0 + DIM_FACTOR) / 2.0).abs() < 1e-9,
            "half way on the raised cosine: {mid}"
        );
        assert!((p.factor(105.0 + RAMP_S) - DIM_FACTOR).abs() < 1e-9);
        assert!(p.animating(105.1) && !p.animating(105.0 + RAMP_S));

        assert_eq!(p.input(110.0), vec![Effect::Display(DisplayState::On)]);
        assert!(
            (p.factor(110.0) - DIM_FACTOR).abs() < 1e-9,
            "the undim starts from where the display is"
        );
        assert!((p.factor(110.0 + RAMP_S) - 1.0).abs() < 1e-9);

        let mut p = IdlePolicy::new(settings(), 0.0);
        run(&mut p, 0.0, 120.0, BATTERY);
        assert_eq!(
            p.factor(120.0),
            0.0,
            "sleep is instant (ON_TO_OFF and DIM_TO_OFF ramps are 0)"
        );
        p.input(130.0);
        assert_eq!(
            p.factor(130.0),
            1.0,
            "wake is instant (OFF_TO_ON ramp is 0)"
        );
    }

    #[test]
    fn input_within_20_s_of_the_dim_lengthens_the_next_countdown_until_the_display_sleeps() {
        let mut p = IdlePolicy::new(settings(), 0.0);
        run(&mut p, 0.0, 106.0, BATTERY);
        p.input(106.0);
        // T = 120 s, T2 = max(min(240, 600), 120) = 240 s: dim at 106 + 225
        let effects = run(&mut p, 106.1, 347.0, BATTERY);
        assert_eq!(
            effects,
            vec![
                (331.0, Effect::Display(DisplayState::Dimmed)),
                (346.0, Effect::Display(DisplayState::Asleep))
            ]
        );
        p.input(400.0);
        let effects = run(&mut p, 400.1, 521.0, BATTERY);
        assert_eq!(
            effects[0],
            (505.0, Effect::Display(DisplayState::Dimmed)),
            "sleeping ends the lengthened countdown"
        );
    }

    #[test]
    fn the_lengthened_countdown_is_capped_at_600_s_unless_t_is_longer() {
        let mut s = settings();
        s.ac.display_sleep = Timer::After(Duration::from_secs(400));
        s.ac.system_sleep = Timer::Never;
        let mut p = IdlePolicy::new(s, 0.0);
        run(&mut p, 0.0, 386.0, AC);
        p.input(386.0);
        let effects = run(&mut p, 386.1, 987.0, AC);
        assert_eq!(
            effects[0],
            (386.0 + 600.0 - 15.0, Effect::Display(DisplayState::Dimmed))
        );
    }

    #[test]
    fn without_dim_the_display_goes_straight_to_sleep() {
        let s = PowerSettings {
            dim_before_display_sleep: false,
            ..settings()
        };
        let mut p = IdlePolicy::new(s, 0.0);
        assert_eq!(
            run(&mut p, 0.0, 121.0, BATTERY),
            vec![(120.0, Effect::Display(DisplayState::Asleep))]
        );
    }

    #[test]
    fn an_inhibitor_holds_the_countdowns_and_its_release_starts_them_again() {
        let mut p = IdlePolicy::new(settings(), 0.0);
        run(&mut p, 0.0, 106.0, BATTERY);
        let held = Facts {
            inhibited: true,
            ..BATTERY
        };
        assert_eq!(
            p.tick(107.0, held),
            vec![Effect::Display(DisplayState::On)],
            "an inhibitor undims the display"
        );
        assert!(run(&mut p, 107.0, 1000.0, held).is_empty());
        let effects = run(&mut p, 1000.1, 1121.0, BATTERY);
        assert_eq!(effects[0].1, Effect::Display(DisplayState::Dimmed));
        assert!(effects[0].0 > 1000.0 + 120.0 - 15.0 - 0.2);
    }

    #[test]
    fn display_sleep_locks_after_the_password_delay_and_input_cancels_the_pending_lock() {
        let s = PowerSettings {
            require_password_after: Timer::After(Duration::from_secs(5)),
            ..settings()
        };
        let mut p = IdlePolicy::new(s, 0.0);
        let effects = run(&mut p, 0.0, 126.0, BATTERY);
        assert_eq!(effects.last(), Some(&(125.0, Effect::Lock)));
        assert_eq!(
            effects.iter().filter(|(_, e)| *e == Effect::Lock).count(),
            1,
            "the lock is asked for once"
        );

        let mut p = IdlePolicy::new(s, 0.0);
        run(&mut p, 0.0, 122.0, BATTERY);
        p.input(122.0);
        assert!(!run(&mut p, 122.1, 200.0, BATTERY)
            .iter()
            .any(|(_, e)| *e == Effect::Lock));

        let immediate = PowerSettings {
            require_password_after: Timer::After(Duration::ZERO),
            ..settings()
        };
        let mut p = IdlePolicy::new(immediate, 0.0);
        assert!(run(&mut p, 0.0, 120.0, BATTERY).contains(&(120.0, Effect::Lock)));
    }

    #[test]
    fn the_lock_screen_sleeps_the_display_after_its_own_30_s() {
        let mut p = IdlePolicy::new(settings(), 0.0);
        let locked = Facts { locked: true, ..AC };
        assert_eq!(
            run(&mut p, 0.0, 31.0, locked),
            vec![
                (15.0, Effect::Display(DisplayState::Dimmed)),
                (30.0, Effect::Display(DisplayState::Asleep))
            ]
        );
    }

    #[test]
    fn system_sleep_is_asked_for_once_until_input() {
        let mut p = IdlePolicy::new(settings(), 0.0);
        let effects = run(&mut p, 0.0, 700.0, BATTERY);
        assert_eq!(
            effects
                .iter()
                .filter(|(_, e)| *e == Effect::SystemSleep)
                .count(),
            1
        );
        assert!(effects.contains(&(600.0, Effect::SystemSleep)));
        p.input(700.0);
        assert!(run(&mut p, 700.1, 1301.0, BATTERY).contains(&(1300.0, Effect::SystemSleep)));
    }

    #[test]
    fn never_switches_a_timer_off() {
        let mut s = settings();
        s.battery.display_sleep = Timer::Never;
        s.battery.system_sleep = Timer::Never;
        let mut p = IdlePolicy::new(s, 0.0);
        assert!(run(&mut p, 0.0, 5000.0, BATTERY).is_empty());
    }

    #[test]
    fn the_overlay_alpha_shows_the_factor_in_linear_light() {
        assert!(overlay_alpha(1.0) < 1e-9);
        assert_eq!(overlay_alpha(0.0), 1.0);
        let dim = overlay_alpha(DIM_FACTOR);
        assert!((dim - 0.9001).abs() < 1e-3, "{dim}");
    }
}
