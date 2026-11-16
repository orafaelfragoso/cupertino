//! Dim, display sleep and wake against a headless compositor on a fixed model clock: the dim overlay darkens the output to
//! the dim factor, sleep turns it black, and input brings the picture back. Tests skip (pass) when no EGL device is
//! available.

use crate::common;

use std::time::{Duration, Instant};

use cupertino_defaults::power::{PowerSettings, SourceTimers, Timer};
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::display_power::{DisplayState, RAMP_S};
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

const OUTPUT: (i32, i32) = (400, 300);
/// Display sleep after 30 s, so the dim starts at 15 s.
const SLEEP_S: f64 = 30.0;
const T0: f64 = 1000.0;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new(power: PowerSettings) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
        let config = Config {
            power,
            ..Config::default()
        };
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("create test compositor");
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        state.fixed_time = Some(T0);
        state.power.set_on_battery(false);
        state.input_activity();
        Some(Harness { state, event_loop })
    }

    /// Move the model clock to `t` and let the policy and the renderer catch up.
    fn at(&mut self, t: f64) {
        // This harness has no live clients; power deadlines advance without a pong barrier.
        self.state.fixed_time = Some(t);
        for _ in 0..3 {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(1),
            )
            .unwrap();
        }
    }

    /// Mean of the red, green and blue channels over the output.
    fn brightness(&mut self, name: &str) -> f64 {
        let path = std::env::temp_dir().join(format!(
            "cupertino-test-{}-power-{name}.png",
            std::process::id()
        ));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let end = Instant::now() + common::HANG_GUARD;
        while !path.exists() {
            assert!(
                Instant::now() < end,
                "timed out waiting for the screenshot {name}"
            );
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
        }
        let img = image::open(&path).unwrap().to_rgb8();
        std::fs::remove_file(&path).unwrap();
        let sum: u64 = img
            .pixels()
            .map(|p| u64::from(p.0[0]) + u64::from(p.0[1]) + u64::from(p.0[2]))
            .sum();
        sum as f64 / (img.width() * img.height() * 3) as f64
    }
}

fn short_timers() -> PowerSettings {
    let timers = SourceTimers {
        display_sleep: Timer::After(Duration::from_secs(SLEEP_S as u64)),
        system_sleep: Timer::Never,
    };
    PowerSettings {
        ac: timers,
        battery: timers,
        require_password_after: Timer::Never,
        ..PowerSettings::default()
    }
}

#[test]
fn the_display_dims_then_goes_black_and_input_wakes_it() {
    let Some(mut h) = Harness::new(short_timers()) else {
        return;
    };
    h.at(T0 + 1.0);
    let lit = h.brightness("lit");
    assert!(lit > 20.0, "the wallpaper is drawn: {lit}");

    h.at(T0 + SLEEP_S - 15.0);
    assert_eq!(h.state.power.policy().state(), DisplayState::Dimmed);
    h.at(T0 + SLEEP_S - 15.0 + RAMP_S / 2.0);
    let ramping = h.brightness("ramping");
    h.at(T0 + SLEEP_S - 15.0 + RAMP_S + 0.1);
    let dimmed = h.brightness("dimmed");
    assert!(
        ramping < lit && dimmed < ramping,
        "the ramp darkens step by step: {lit} {ramping} {dimmed}"
    );
    // factor 0.01 in linear light is about 0.1 of the encoded value
    let ratio = dimmed / lit;
    assert!(
        (0.07..0.14).contains(&ratio),
        "dimmed to the dim factor: {ratio}"
    );

    h.at(T0 + SLEEP_S);
    assert_eq!(h.state.power.policy().state(), DisplayState::Asleep);
    assert_eq!(h.brightness("asleep"), 0.0, "a sleeping display is black");

    h.state.input_activity();
    assert_eq!(h.state.power.policy().state(), DisplayState::On);
    let woken = h.brightness("woken");
    assert!(
        (woken - lit).abs() < 0.5,
        "waking switches the picture back at once: {woken} against {lit}"
    );
}

#[test]
fn input_while_dimmed_ramps_back_to_full_brightness() {
    let Some(mut h) = Harness::new(short_timers()) else {
        return;
    };
    h.at(T0 + 1.0);
    let lit = h.brightness("lit2");
    h.at(T0 + SLEEP_S - 15.0);
    h.at(T0 + SLEEP_S - 14.0);
    h.state.input_activity();
    assert_eq!(h.state.power.policy().state(), DisplayState::On);
    let start = h.brightness("undim-start");
    assert!(start < lit / 2.0, "the undim starts dark: {start}");
    h.at(T0 + SLEEP_S - 14.0 + RAMP_S + 0.1);
    let back = h.brightness("undim-end");
    assert!(
        (back - lit).abs() < 0.5,
        "and ends at full brightness: {back} against {lit}"
    );
}

/// While another console has the seat nothing dims or sleeps, however long; back in front the display is on, with
/// fresh countdowns, and fades in from black. A display that slept before the switch is on again at the return.
#[test]
fn a_session_in_the_background_never_dims_or_sleeps_and_comes_back_awake() {
    let Some(mut h) = Harness::new(short_timers()) else {
        return;
    };
    h.at(T0 + 1.0);
    let lit = h.brightness("before-pause");
    h.state.session_deactivated();
    assert_eq!(h.state.display_power_deadline(), None);
    h.at(T0 + SLEEP_S);
    h.at(T0 + 3600.0);
    assert_eq!(h.state.power.policy().state(), DisplayState::On);

    let back = T0 + 3600.0;
    h.state.session_activated();
    assert_eq!(
        h.state.display_power_deadline(),
        Some(back + SLEEP_S - 15.0),
        "the countdown starts at the return"
    );
    h.at(back + 1.0);
    let shown = h.brightness("after-resume");
    assert!(
        (shown - lit).abs() < 0.5,
        "faded in to full brightness: {shown} against {lit}"
    );

    // asleep when the console went elsewhere: on again at the return, without input
    h.at(back + 1.0 + SLEEP_S);
    assert_eq!(h.state.power.policy().state(), DisplayState::Asleep);
    h.state.session_deactivated();
    h.at(back + 7200.0);
    h.state.session_activated();
    assert_eq!(h.state.power.policy().state(), DisplayState::On);
    h.at(back + 7201.0);
    let woken = h.brightness("after-resume-asleep");
    assert!((woken - lit).abs() < 0.5, "{woken} against {lit}");
}

/// A session with a lock screen counts down to the lock after display sleep; the login screen's own server, with the
/// same process configured, has no lock screen to ask, and a display asleep past the password delay asks for no lock.
#[test]
fn the_login_screens_server_never_counts_down_to_a_lock() {
    let mut power = short_timers();
    power.require_password_after = Timer::After(Duration::from_secs(5));
    let Some(mut h) = Harness::new(power) else {
        return;
    };
    // configured, never started: the server only reads its roles here
    h.state.config.shell_processes = vec![ShellProcess {
        name: "lock screen".into(),
        command: vec!["/usr/bin/true".into()],
        roles: vec![ShellRole::Session, ShellRole::SessionLock],
    }];
    h.at(T0 + SLEEP_S);
    assert_eq!(h.state.power.policy().state(), DisplayState::Asleep);
    assert_eq!(
        h.state.display_power_deadline(),
        Some(T0 + SLEEP_S + 5.0),
        "a session counts down to its lock"
    );
    h.state.login_screen = true;
    assert_eq!(
        h.state.display_power_deadline(),
        None,
        "the login screen's server has nothing left to count down to"
    );
    h.at(T0 + SLEEP_S + 10.0);
    assert_eq!(h.state.power.policy().state(), DisplayState::Asleep);
}
