//! The launcher's panel moves on the springs and fades it was designed with.

use cupertino_theme::animation::{Easing, SpringPreset, Timing};

/// The progress of a spring of undamped period `response` and damping ratio `damping` (under 1) `t` seconds after
/// it starts from rest.
fn underdamped(response: f64, damping: f64, t: f64) -> f64 {
    let omega = std::f64::consts::TAU / response;
    let decay = damping * omega;
    let ringing = omega * (1.0 - damping * damping).sqrt();
    1.0 - (-decay * t).exp() * ((ringing * t).cos() + decay / ringing * (ringing * t).sin())
}

#[test]
fn the_enclosure_and_the_mode_buttons_move_on_underdamped_springs_that_overshoot() {
    for (spring, response, damping) in [
        (SpringPreset::LAUNCHER_GROW, 0.27, 0.83),
        (SpringPreset::LAUNCHER_SHRINK, 0.295, 0.86),
        (SpringPreset::LAUNCHER_FIELD, 0.305, 0.68),
        (SpringPreset::LAUNCHER_SPLIT, 0.575, 0.69),
    ] {
        let timing = Timing::from(spring);
        for t in [0.02_f64, 0.05, 0.1, 0.2, 0.3, 0.5] {
            let expected = underdamped(response, damping, t);
            assert!(
                (timing.value(t) - expected).abs() < 1e-9,
                "{spring:?} at {t}: {} against {expected}",
                timing.value(t)
            );
        }
        let most = (1..400)
            .map(|i| timing.value(f64::from(i) / 200.0))
            .fold(0.0, f64::max);
        assert!(most > 1.0, "{spring:?} overshoots");
    }
}

#[test]
fn the_width_comes_back_from_its_dip_critically_damped_in_a_quarter_second_period() {
    let back = Timing::from(SpringPreset::LAUNCHER_DIP_BACK);
    let rate = std::f64::consts::TAU / 0.25;
    for t in [0.01_f64, 0.03, 0.06, 0.1, 0.2, 0.4] {
        let expected = 1.0 - (1.0 + rate * t) * (-rate * t).exp();
        assert!((back.value(t) - expected).abs() < 1e-12, "{t}");
    }
}

#[test]
fn the_fades_run_on_the_timing_tables_curves_for_their_durations() {
    for (timing, easing, seconds) in [
        (Timing::LAUNCHER_OPEN_FADE, Easing::LINEAR, 0.10),
        (Timing::LAUNCHER_FOLD_FADE, Easing::LINEAR, 0.05),
        (Timing::LAUNCHER_UNFOLD_CONTENT, Easing::EASE_OUT, 0.12),
        (Timing::LAUNCHER_FILES_CONTENT, Easing::EASE_OUT, 0.10),
        (Timing::LAUNCHER_CLOSE, Easing::EASE_OUT, 0.10),
        (Timing::LAUNCHER_GLYPH_SHARPEN, Easing::EASE_OUT, 0.2),
        (Timing::LAUNCHER_MERGE, Easing::EASE_OUT, 0.05),
        (Timing::LAUNCHER_DIP, Easing::EASE_IN_OUT, 0.14),
        (Timing::LAUNCHER_SCROLLER_FADE, Easing::LINEAR, 0.25),
    ] {
        assert_eq!(timing, easing.over(seconds));
        assert_eq!(timing.duration(), seconds);
    }
    let open = Timing::LAUNCHER_OPEN_FADE;
    for t in [0.0, 0.025, 0.05, 0.075, 0.1] {
        assert!((open.value(t) - t / 0.1).abs() < 1e-9, "{t}");
    }
}

#[test]
fn a_mode_button_swells_as_one_minus_the_square_of_what_is_left() {
    for i in 0..=20 {
        let t = f64::from(i) / 20.0;
        let expected = 1.0 - (1.0 - t) * (1.0 - t);
        assert!(
            (Easing::LAUNCHER_BUD.eval(t) - expected).abs() < 1e-9,
            "{t}"
        );
    }
}
