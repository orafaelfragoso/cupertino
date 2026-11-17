//! Pointer acceleration: parametric curves interpolated between settings, linear gain, and the wheel and click helpers.

use cupertino_input::{
    double_click_space, scroll_point_delta, wheel_points, AccelCurve, CurveParams, ParametricCurve,
    PointerAccel, DEFAULT_TRACKING_SPEED, MOUSE_CURVES, TRACKPAD_CURVES, TRACKPAD_REPORT_RATE_HZ,
};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn trackpad(setting: f64) -> PointerAccel {
    PointerAccel::new(&TRACKPAD_CURVES, setting, 400.0, TRACKPAD_REPORT_RATE_HZ)
}

#[test]
fn fixed_point_values_are_sixteen_sixteen() {
    let p = CurveParams::from_fixed([65536, 32768, -65536, 0, 131072, 0, 0]);
    assert_eq!(
        (p.index, p.gain_linear, p.gain_parabolic, p.gain_quartic),
        (1.0, 0.5, -1.0, 2.0)
    );
}

#[test]
fn a_negative_setting_or_no_curve_turns_acceleration_off() {
    assert!(ParametricCurve::new(&MOUSE_CURVES, -0.1).is_none());
    assert!(ParametricCurve::new(&[], 0.5).is_none(), "no usable curve");
    for (name, accel) in [
        (
            "negative setting",
            PointerAccel::new(&MOUSE_CURVES, -1.0, 400.0, 0),
        ),
        ("no curve", PointerAccel::with_curve(None, 400.0, 0)),
        ("negative linear gain", PointerAccel::linear(-1.0)),
    ] {
        assert_eq!(accel.accelerate(3.0, -4.0, 8.0), (3.0, -4.0), "{name}");
    }
    let curve = ParametricCurve::new(&MOUSE_CURVES, 0.5).unwrap();
    let wrapped = PointerAccel::with_curve(Some(AccelCurve::Parametric(curve)), 400.0, 0);
    assert_ne!(
        wrapped.accelerate(5.0, 0.0, 8.0).0,
        5.0,
        "an explicit curve"
    );
}

#[test]
fn a_curve_output_rises_with_input_speed_and_with_the_setting() {
    let curve = ParametricCurve::new(&TRACKPAD_CURVES, DEFAULT_TRACKING_SPEED).unwrap();
    assert_eq!(curve.speed(0.0), 0.0);
    let mut last = 0.0;
    for i in 1..=60 {
        let v = f64::from(i) * 0.5;
        let out = curve.speed(v);
        assert!(out.is_finite() && out > last, "v={v}: {out} after {last}");
        last = out;
    }
    let at = |setting: f64, v: f64| {
        ParametricCurve::new(&TRACKPAD_CURVES, setting)
            .unwrap()
            .speed(v)
    };
    assert!(at(1.5, 20.0) > at(0.125, 20.0));
    let (lo, mid, hi) = (at(0.5, 10.0), at(0.6, 10.0), at(0.6875, 10.0));
    assert!(mid > lo.min(hi) && mid < lo.max(hi), "between two stops");
    assert_eq!(
        ParametricCurve::new(&MOUSE_CURVES, 3.0),
        ParametricCurve::new(&MOUSE_CURVES, 9.0),
        "beyond the last stop"
    );
}

#[test]
fn linear_gain_scales_motion_and_zero_means_the_minimum() {
    assert_eq!(
        PointerAccel::linear(2.0).accelerate(3.0, -4.0, 8.0),
        (6.0, -8.0)
    );
    let (x, y) = PointerAccel::linear(0.0).accelerate(10.0, 20.0, 8.0);
    assert!(close(x, 1.0) && close(y, 2.0));
}

#[test]
fn a_curve_keeps_the_direction_and_amplifies_a_fast_stroke_more() {
    let accel = trackpad(DEFAULT_TRACKING_SPEED);
    let (x, y) = accel.accelerate(6.0, -8.0, 8.0);
    assert!(x > 0.0 && y < 0.0);
    assert!(close(x / y, -0.75));
    assert_eq!(accel.accelerate(0.0, 0.0, 8.0), (0.0, 0.0));
    let slow = accel.accelerate(1.0, 0.0, 8.0).0;
    let fast = accel.accelerate(40.0, 0.0, 8.0).0 / 40.0;
    assert!(fast > slow, "{fast} {slow}");
}

#[test]
fn a_late_report_is_scaled_down_to_the_report_period() {
    let accel = trackpad(DEFAULT_TRACKING_SPEED);
    let on_time = accel.accelerate(10.0, 0.0, 1000.0 / 120.0).0;
    let late = accel.accelerate(10.0, 0.0, 100.0).0;
    assert!(late < on_time, "{late} vs {on_time}");
    assert!(
        close(accel.accelerate(10.0, 0.0, 0.0).0, on_time),
        "an unknown interval"
    );
    assert_eq!(
        PointerAccel::new(&TRACKPAD_CURVES, 0.6875, 0.0, 120),
        PointerAccel::new(&TRACKPAD_CURVES, 0.6875, 400.0, 120),
        "a zero resolution falls back to the default"
    );
}

#[test]
fn wheel_notches_scroll_three_lines() {
    for (value, points) in [(120.0, 48.0), (-60.0, -24.0), (0.0, 0.0)] {
        assert_eq!(wheel_points(value), points, "{value}");
    }
}

#[test]
fn scroll_lines_become_whole_points_truncated_towards_zero() {
    for (lines, points) in [(1.0, 10), (0.19, 1), (-0.19, -1), (0.09, 0)] {
        assert_eq!(scroll_point_delta(lines), points, "{lines}");
    }
}

#[test]
fn the_double_click_space_grows_with_the_interval_up_to_a_cap() {
    for (interval, space) in [(0.0, 2.0), (0.5, 5.0), (10.0, 14.0)] {
        assert_eq!(double_click_space(interval), space, "{interval}");
    }
}
