//! Pointer acceleration: parametric curves interpolated between settings, linear gain, and the wheel and click helpers.

use cupertino_input::{
    double_click_space, scroll_point_delta, wheel_points, AccelCurve, CurveParams, ParametricCurve,
    PointerAccel, DEFAULT_TRACKING_SPEED, MOUSE_CURVES, POINTS_PER_LINE, TRACKING_SPEED_STOPS,
    TRACKPAD_CURVES, TRACKPAD_REPORT_RATE_HZ, TRACKPAD_SCROLL_CURVES, WHEEL_LINES_PER_NOTCH,
    WHEEL_LINE_POINTS,
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
    assert_eq!(p.index, 1.0);
    assert_eq!(p.gain_linear, 0.5);
    assert_eq!(p.gain_parabolic, -1.0);
    assert_eq!(p.gain_quartic, 2.0);
}

#[test]
fn the_curve_tables_are_indexed_by_the_tracking_speed_stops() {
    for curves in [&MOUSE_CURVES, &TRACKPAD_CURVES, &TRACKPAD_SCROLL_CURVES] {
        for (curve, stop) in curves.iter().zip(TRACKING_SPEED_STOPS) {
            assert!((curve.index - stop).abs() < 1e-3, "{} {stop}", curve.index);
        }
    }
}

#[test]
fn a_negative_setting_turns_acceleration_off() {
    assert!(ParametricCurve::new(&MOUSE_CURVES, -0.1).is_none());
    assert!(ParametricCurve::new(&[], 0.5).is_none(), "no usable curve");
    let off = PointerAccel::new(&MOUSE_CURVES, -1.0, 400.0, 0);
    assert_eq!(off.accelerate(3.0, -4.0, 8.0), (3.0, -4.0));
}

#[test]
fn a_curve_output_rises_with_input_speed() {
    let curve = ParametricCurve::new(&TRACKPAD_CURVES, DEFAULT_TRACKING_SPEED).unwrap();
    let mut last = 0.0;
    for i in 1..=60 {
        let v = f64::from(i) * 0.5;
        let out = curve.speed(v);
        assert!(out.is_finite() && out > last, "v={v}: {out} after {last}");
        last = out;
    }
    assert_eq!(curve.speed(0.0), 0.0);
}

#[test]
fn a_higher_setting_gives_more_pointer_travel_for_a_fast_stroke() {
    let slow = ParametricCurve::new(&TRACKPAD_CURVES, 0.125).unwrap();
    let fast = ParametricCurve::new(&TRACKPAD_CURVES, 1.5).unwrap();
    assert!(fast.speed(20.0) > slow.speed(20.0));
}

#[test]
fn a_setting_between_two_stops_is_between_their_curves() {
    let lo = ParametricCurve::new(&TRACKPAD_CURVES, 0.5)
        .unwrap()
        .speed(10.0);
    let hi = ParametricCurve::new(&TRACKPAD_CURVES, 0.6875)
        .unwrap()
        .speed(10.0);
    let mid = ParametricCurve::new(&TRACKPAD_CURVES, 0.6)
        .unwrap()
        .speed(10.0);
    assert!(mid > lo.min(hi) && mid < lo.max(hi));
}

#[test]
fn a_setting_beyond_the_last_stop_uses_the_last_curve() {
    let last = ParametricCurve::new(&MOUSE_CURVES, 3.0).unwrap();
    let beyond = ParametricCurve::new(&MOUSE_CURVES, 9.0).unwrap();
    assert_eq!(last, beyond);
}

#[test]
fn linear_gain_scales_motion_and_zero_means_the_minimum() {
    let two = PointerAccel::linear(2.0);
    assert_eq!(two.accelerate(3.0, -4.0, 8.0), (6.0, -8.0));
    let zero = PointerAccel::linear(0.0);
    let (x, y) = zero.accelerate(10.0, 20.0, 8.0);
    assert!(close(x, 1.0) && close(y, 2.0));
    assert_eq!(
        PointerAccel::linear(-1.0).accelerate(3.0, 4.0, 8.0),
        (3.0, 4.0)
    );
}

#[test]
fn a_curve_keeps_the_direction_of_the_motion() {
    let accel = trackpad(DEFAULT_TRACKING_SPEED);
    let (x, y) = accel.accelerate(6.0, -8.0, 8.0);
    assert!(x > 0.0 && y < 0.0);
    assert!(close(x / y, -0.75));
}

#[test]
fn a_faster_stroke_is_amplified_more_than_a_slow_one() {
    let accel = trackpad(DEFAULT_TRACKING_SPEED);
    let slow = accel.accelerate(1.0, 0.0, 8.0).0;
    let fast = accel.accelerate(40.0, 0.0, 8.0).0 / 40.0;
    assert!(fast > slow, "{fast} {slow}");
}

#[test]
fn no_motion_gives_no_motion() {
    let accel = trackpad(DEFAULT_TRACKING_SPEED);
    assert_eq!(accel.accelerate(0.0, 0.0, 8.0), (0.0, 0.0));
}

#[test]
fn a_late_report_is_scaled_down_to_the_report_period() {
    let accel = trackpad(DEFAULT_TRACKING_SPEED);
    let on_time = accel.accelerate(10.0, 0.0, 1000.0 / 120.0).0;
    let late = accel.accelerate(10.0, 0.0, 100.0).0;
    assert!(late < on_time, "{late} vs {on_time}");
    let unknown = accel.accelerate(10.0, 0.0, 0.0).0;
    assert!(close(unknown, on_time));
}

#[test]
fn a_zero_resolution_falls_back_to_the_default() {
    let a = PointerAccel::new(&TRACKPAD_CURVES, 0.6875, 0.0, 120);
    let b = PointerAccel::new(&TRACKPAD_CURVES, 0.6875, 400.0, 120);
    assert_eq!(a, b);
}

#[test]
fn an_explicit_curve_can_be_wrapped() {
    let curve = ParametricCurve::new(&MOUSE_CURVES, 0.5).unwrap();
    let a = PointerAccel::with_curve(Some(AccelCurve::Parametric(curve)), 400.0, 0);
    assert_ne!(a.accelerate(5.0, 0.0, 8.0).0, 5.0);
    assert_eq!(
        PointerAccel::with_curve(None, 400.0, 0).accelerate(5.0, 0.0, 8.0),
        (5.0, 0.0)
    );
}

#[test]
fn wheel_notches_scroll_three_lines() {
    assert_eq!(
        wheel_points(120.0),
        WHEEL_LINES_PER_NOTCH * WHEEL_LINE_POINTS
    );
    assert_eq!(wheel_points(-60.0), -24.0);
    assert_eq!(wheel_points(0.0), 0.0);
}

#[test]
fn scroll_lines_become_whole_points_truncated_towards_zero() {
    assert_eq!(scroll_point_delta(1.0), POINTS_PER_LINE as i32);
    assert_eq!(scroll_point_delta(0.19), 1);
    assert_eq!(scroll_point_delta(-0.19), -1);
    assert_eq!(scroll_point_delta(0.09), 0);
}

#[test]
fn the_double_click_space_grows_with_the_interval_up_to_a_cap() {
    assert_eq!(double_click_space(0.0), 2.0);
    assert_eq!(double_click_space(0.5), 5.0);
    assert_eq!(double_click_space(10.0), 14.0);
}
