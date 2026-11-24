//! The timing-function table names the design's curves and its default spring; a named motion reads out as the
//! curves that run it.

use cupertino_animation::{CubicBezier, Curve, LayerCurve, SpringParams};
use cupertino_theme::animation::{
    Easing, SpringPreset, Timing, TimingPreset, DEFAULT_SPRING, EASE_CURVE, EASE_IN_CURVE,
    EASE_IN_OUT_CURVE, EASE_OUT_CURVE, LINEAR_CURVE,
};

#[test]
fn each_entry_is_found_by_name_and_index_and_is_its_curve_or_the_default_spring() {
    for (preset, name, bezier) in [
        (TimingPreset::Ease, "ease", Some(EASE_CURVE)),
        (TimingPreset::Linear, "linear", Some(LINEAR_CURVE)),
        (TimingPreset::EaseIn, "ease-in", Some(EASE_IN_CURVE)),
        (TimingPreset::EaseOut, "ease-out", Some(EASE_OUT_CURVE)),
        (
            TimingPreset::EaseInOut,
            "ease-in-out",
            Some(EASE_IN_OUT_CURVE),
        ),
        (TimingPreset::Spring, "spring", None),
    ] {
        assert_eq!(TimingPreset::from_name(name), Some(preset), "{name}");
        assert_eq!(
            TimingPreset::from_index(preset.index()),
            Some(preset),
            "{name}"
        );
        assert_eq!(preset.bezier(), bezier, "{name}");
        let spring = bezier.is_none().then_some(DEFAULT_SPRING);
        assert_eq!(preset.spring(), spring, "{name}");
    }
    assert_eq!(TimingPreset::from_name("bounce"), None);
}

#[test]
fn the_on_screen_displays_fade_at_constant_speed_over_seven_tenths_of_a_second() {
    let fade = Timing::OSD_FADE;
    assert_eq!(fade, Easing::LINEAR.over(0.7));
    assert_eq!(fade.duration(), 0.7);
    assert_eq!(
        fade.layer_curve(),
        LayerCurve::Timed {
            duration: 0.7,
            bezier: LINEAR_CURVE,
        }
    );
    for t in [0.0, 0.175, 0.35, 0.525, 0.7] {
        assert!(
            (fade.value(t) - t / 0.7).abs() < 1e-9,
            "{t} s into the fade: {}",
            fade.value(t)
        );
    }
}

#[test]
fn the_input_source_switcher_fades_at_constant_speed_over_three_tenths_of_a_second() {
    let fade = Timing::INPUT_SOURCES_FADE;
    assert_eq!(fade, Easing::LINEAR.over(0.3));
    assert_eq!(fade.duration(), 0.3);
    for t in [0.0, 0.075, 0.15, 0.225, 0.3] {
        assert!(
            (fade.value(t) - t / 0.3).abs() < 1e-9,
            "{t} s into the fade: {}",
            fade.value(t)
        );
    }
}

#[test]
fn a_chosen_menu_row_flashes_off_for_two_frames_then_on_for_four() {
    let at_60_hz = |timing: Timing| (timing.duration() * 60.0).round();
    assert_eq!(Timing::MENU_CHOICE_OFF, Easing::LINEAR.over(0.034));
    assert_eq!(Timing::MENU_CHOICE_ON, Easing::LINEAR.over(0.067));
    assert_eq!(at_60_hz(Timing::MENU_CHOICE_OFF), 2.0);
    assert_eq!(at_60_hz(Timing::MENU_CHOICE_ON), 4.0);
}

#[test]
fn the_desktop_band_fades_in_over_three_twentieths_and_out_over_a_fifth_of_a_second_at_constant_speed(
) {
    for (fade, seconds) in [
        (Timing::DESKTOP_BAND_FADE_IN, 0.15),
        (Timing::DESKTOP_BAND_FADE_OUT, 0.2),
    ] {
        assert_eq!(fade, Easing::LINEAR.over(seconds));
        for share in [0.0, 0.25, 0.5, 1.0] {
            assert!((fade.value(seconds * share) - share).abs() < 1e-9);
        }
    }
}

#[test]
fn a_named_motion_reads_out_as_the_curves_that_run_it() {
    let tau = std::f64::consts::TAU;
    for (timing, layer) in [
        (
            Easing::EASE_OUT.over(0.1),
            LayerCurve::Timed {
                duration: 0.1,
                bezier: EASE_OUT_CURVE,
            },
        ),
        (
            // the cosine ease as the Bézier fitted to it
            Easing::COSINE.over(0.3),
            LayerCurve::Timed {
                duration: 0.3,
                bezier: CubicBezier::new(0.364, 0.0, 0.636, 1.0),
            },
        ),
        (
            Timing::from(SpringPreset::LAUNCHER_OPEN),
            LayerCurve::Spring {
                response: 0.30,
                damping: 0.74,
            },
        ),
        (
            // a physical spring as its undamped period and its damping ratio
            Timing::from(SpringPreset::DEFAULT),
            LayerCurve::Spring {
                response: tau / 10.0,
                damping: 0.5,
            },
        ),
    ] {
        match (timing.layer_curve(), layer) {
            (
                LayerCurve::Spring { response, damping },
                LayerCurve::Spring {
                    response: r,
                    damping: d,
                },
            ) => assert!(
                (response - r).abs() < 1e-12 && (damping - d).abs() < 1e-12,
                "{timing:?}"
            ),
            (sent, layer) => assert_eq!(sent, layer, "{timing:?}"),
        }
        match (timing.curve(), layer) {
            (
                Curve::Timed { duration, bezier },
                LayerCurve::Timed {
                    duration: d,
                    bezier: b,
                },
            ) => assert_eq!((duration, bezier), (d, b), "{timing:?}"),
            (Curve::Spring(params), LayerCurve::Spring { .. }) => assert!(
                (Timing::from(SpringPreset::DEFAULT) == timing && params == DEFAULT_SPRING)
                    || params == SpringParams::from_ratio(0.30, 0.74),
                "{timing:?}"
            ),
            other => panic!("{timing:?} reads out as {other:?}"),
        }
        assert_eq!(timing.value(0.0), 0.0, "{timing:?} starts at 0");
        assert!(
            (timing.value(timing.duration()) - 1.0).abs() < 2e-3,
            "{timing:?} rests at 1"
        );
    }
}

#[test]
fn a_toolbar_items_press_peaks_about_a_tenth_of_a_second_in_and_its_release_dips_a_little() {
    let press = Timing::from(SpringPreset::TOOLBAR_ITEM_PRESS);
    let (peak_at, peak) = (1..300)
        .map(|ms| (f64::from(ms) / 1000.0, press.value(f64::from(ms) / 1000.0)))
        .fold((0.0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
    assert!((0.09..0.12).contains(&peak_at), "peaks at {peak_at} s");
    assert!(peak > 1.05 && peak < 1.09, "peaks at {peak}");
    assert!(
        (press.value(0.2) - 1.0).abs() < 0.01,
        "{}",
        press.value(0.2)
    );
    let release = Timing::from(SpringPreset::TOOLBAR_ITEM_RELEASE);
    assert!(
        (0.88..0.95).contains(&release.value(0.095)),
        "{}",
        release.value(0.095)
    );
    let dip = (1..300)
        .map(|ms| release.value(f64::from(ms) / 1000.0))
        .fold(0.0, f64::max);
    assert!(dip > 1.0 && dip < 1.03, "passes its end by {dip}");
    assert!(
        release.duration() < 0.35,
        "settles in {} s",
        release.duration()
    );
}

#[test]
fn a_glass_buttons_press_settles_in_about_512_ms_and_its_release_in_about_965() {
    for (spring, settles) in [
        (SpringPreset::GLASS_BUTTON_PRESS, 0.512),
        (SpringPreset::GLASS_BUTTON_RELEASE, 0.965),
    ] {
        let timing = Timing::from(spring);
        assert!(
            (timing.duration() - settles).abs() < 0.005,
            "{spring:?} settles in {} s",
            timing.duration()
        );
        // a little bounce past its target, and at rest once settled
        let peak = (1..100)
            .map(|i| timing.value(settles * f64::from(i) / 100.0))
            .fold(0.0, f64::max);
        assert!(peak > 1.0 && peak < 1.05, "{spring:?} peaks at {peak}");
        assert!((timing.value(settles) - 1.0).abs() < 1e-3);
    }
}
