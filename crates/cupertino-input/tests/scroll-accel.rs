//! Scroll acceleration turns touchpad deltas into lines, remembering recent speed and emitting whole tenths of a line.

use cupertino_input::{
    AccelCurve, ParametricCurve, ScrollAxisAccel, ScrollConsume, TouchpadSettings,
    DEFAULT_SCROLL_REPORT_RATE_HZ, TRACKPAD_SCROLL_CURVES, TRACKPAD_SCROLL_RESOLUTION,
};

fn axis() -> ScrollAxisAccel {
    let curve = ParametricCurve::new(&TRACKPAD_SCROLL_CURVES, 0.3125).unwrap();
    ScrollAxisAccel::new(
        AccelCurve::Parametric(curve),
        TRACKPAD_SCROLL_RESOLUTION,
        DEFAULT_SCROLL_REPORT_RATE_HZ,
    )
}

/// Ten fast deltas, 15 ms apart; returns the time of the last one.
fn build_speed(a: &mut ScrollAxisAccel) -> f64 {
    let mut t = 0.0;
    for _ in 0..10 {
        t += 15.0;
        a.accelerate(30.0, t, None);
    }
    t
}

#[test]
fn a_delta_keeps_its_direction_and_no_delta_scrolls_nothing() {
    assert!(axis().accelerate(8.0, 0.0, None) > 0.0);
    assert!(axis().accelerate(-8.0, 0.0, None) < 0.0);
    let mut a = axis();
    assert_eq!(a.accelerate(0.0, 10.0, None), 0.0);
    assert_eq!(a.scroll_lines(0.0, 20.0), 0.0);
}

#[test]
fn a_fast_flick_scrolls_further_per_unit_than_a_slow_drag() {
    let mut slow = axis();
    let mut fast = axis();
    let mut slow_out = 0.0;
    let mut fast_out = 0.0;
    for i in 0..20 {
        let t = f64::from(i) * 15.0;
        slow_out = slow.accelerate(2.0, t, None) / 2.0;
        fast_out = fast.accelerate(40.0, t, None) / 40.0;
    }
    assert!(fast_out > slow_out, "{fast_out} {slow_out}");
}

#[test]
fn a_pause_or_a_reversal_forgets_the_remembered_speed() {
    let first = axis().accelerate(5.0, 0.0, None);
    let mut a = axis();
    let t = build_speed(&mut a);
    assert!(
        (a.accelerate(5.0, t + 1000.0, None) - first).abs() < 1e-9,
        "a pause"
    );

    let mut a = axis();
    let t = build_speed(&mut a);
    let on = a.clone().accelerate(5.0, t + 15.0, None);
    let reversed = a.accelerate(-5.0, t + 15.0, None);
    assert!(reversed < 0.0);
    assert!(reversed.abs() < on.abs(), "{reversed} {on}");
}

#[test]
fn momentum_events_at_the_base_dispatch_rate_scroll_like_plain_ones() {
    let at_base = axis().accelerate(10.0, 0.0, Some(60.0));
    let plain = axis().accelerate(10.0, 0.0, None);
    assert!((at_base - plain).abs() < 1e-9);
    let at_double = axis().accelerate(10.0, 0.0, Some(120.0));
    assert!(at_double.is_finite() && at_double > 0.0);
}

#[test]
#[should_panic(expected = "momentum dispatch rate")]
fn a_momentum_rate_must_be_positive() {
    axis().accelerate(1.0, 0.0, Some(0.0));
}

#[test]
fn lines_are_emitted_in_tenths_carrying_the_remainder_in_one_direction() {
    let mut free = ScrollConsume::new(0.0);
    assert_eq!(
        free.consume(3.0, 1.25),
        1.25,
        "no threshold passes lines through"
    );
    assert_eq!(
        free.consume(-3.0, -0.04),
        -0.1,
        "nothing smaller than a tenth line is emitted"
    );

    let mut c = ScrollConsume::new(400.0);
    assert_eq!(c.consume(20.0, 1.234), 1.2, "a large motion at once");
    let next = c.consume(20.0, 0.07);
    assert!((next - 0.1).abs() < 1e-9, "the remainder carries: {next}");
    let back = c.consume(-20.0, -0.5);
    assert!((back + 0.5).abs() < 1e-9, "a reversal drops it: {back}");
}

#[test]
fn small_motions_are_held_back_until_enough_has_accumulated() {
    let mut c = ScrollConsume::new(400.0);
    // The unit is 8 counts, the threshold 24.
    assert_eq!(c.consume(3.0, 0.5), 0.0);
    assert_eq!(c.consume(3.0, 0.5), 0.0);
    let total: f64 = (0..10).map(|_| c.consume(3.0, 0.5)).sum();
    assert!(total > 0.0);
}

#[test]
fn the_touchpad_settings_build_the_accelerators() {
    let settings = TouchpadSettings::default();
    let mut scroll = settings.scroll_accel().expect("a scrolling speed in range");
    assert!(scroll.scroll_lines(40.0, 0.0) != 0.0);
    let off = TouchpadSettings {
        scrolling_speed: -1.0,
        ..TouchpadSettings::default()
    };
    assert!(off.scroll_accel().is_none());
    let (x, y) = settings.pointer_accel().accelerate(5.0, 0.0, 8.0);
    assert!(x > 0.0 && y == 0.0);
    let linear = TouchpadSettings {
        acceleration: false,
        tracking_speed: 2.0,
        ..TouchpadSettings::default()
    };
    assert_eq!(
        linear.pointer_accel().accelerate(5.0, 1.0, 8.0),
        (10.0, 2.0)
    );
}
