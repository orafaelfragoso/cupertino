//! Scroll acceleration turns touchpad deltas into lines, remembering recent speed and emitting whole tenths of a line.

use cupertino_input::{
    AccelCurve, ParametricCurve, ScrollAxisAccel, ScrollConsume, TouchpadSettings,
    DEFAULT_SCROLL_REPORT_RATE_HZ, TRACKPAD_SCROLL_CURVES, TRACKPAD_SCROLL_RESOLUTION,
};

fn axis(setting: f64) -> ScrollAxisAccel {
    let curve = ParametricCurve::new(&TRACKPAD_SCROLL_CURVES, setting).unwrap();
    ScrollAxisAccel::new(
        AccelCurve::Parametric(curve),
        TRACKPAD_SCROLL_RESOLUTION,
        DEFAULT_SCROLL_REPORT_RATE_HZ,
    )
}

#[test]
fn no_delta_scrolls_nothing() {
    let mut a = axis(0.3125);
    assert_eq!(a.accelerate(0.0, 10.0, None), 0.0);
    assert_eq!(a.scroll_lines(0.0, 20.0), 0.0);
}

#[test]
fn the_direction_of_the_delta_is_kept() {
    let mut up = axis(0.3125);
    let mut down = axis(0.3125);
    assert!(up.accelerate(8.0, 0.0, None) > 0.0);
    assert!(down.accelerate(-8.0, 0.0, None) < 0.0);
}

#[test]
fn a_fast_flick_scrolls_further_per_unit_than_a_slow_drag() {
    let mut slow = axis(0.3125);
    let mut fast = axis(0.3125);
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
fn a_pause_resets_the_remembered_speed() {
    let mut a = axis(0.3125);
    let first = a.accelerate(5.0, 0.0, None);
    let mut t = 0.0;
    for _ in 0..10 {
        t += 15.0;
        a.accelerate(30.0, t, None);
    }
    let after_pause = a.accelerate(5.0, t + 1000.0, None);
    assert!((after_pause - first).abs() < 1e-9, "{after_pause} {first}");
}

#[test]
fn reversing_direction_starts_a_new_history() {
    let mut a = axis(0.3125);
    let mut t = 0.0;
    for _ in 0..10 {
        t += 15.0;
        a.accelerate(30.0, t, None);
    }
    let mut continued = a.clone();
    let on = continued.accelerate(5.0, t + 15.0, None);
    let reversed = a.accelerate(-5.0, t + 15.0, None);
    assert!(reversed < 0.0);
    assert!(
        reversed.abs() < on.abs(),
        "the reversed motion does not inherit the speed built up before: {reversed} {on}"
    );
}

#[test]
fn momentum_events_at_another_dispatch_rate_scroll_the_same_total() {
    let mut a = axis(0.3125);
    let mut b = axis(0.3125);
    let at_base = a.accelerate(10.0, 0.0, Some(60.0));
    let plain = b.accelerate(10.0, 0.0, None);
    assert!((at_base - plain).abs() < 1e-9);
    let mut c = axis(0.3125);
    let at_double = c.accelerate(10.0, 0.0, Some(120.0));
    assert!(at_double.is_finite() && at_double > 0.0);
}

#[test]
#[should_panic(expected = "momentum dispatch rate")]
fn a_momentum_rate_must_be_positive() {
    axis(0.3125).accelerate(1.0, 0.0, Some(0.0));
}

#[test]
fn a_consumer_with_no_threshold_passes_lines_through() {
    let mut c = ScrollConsume::new(0.0);
    assert_eq!(c.consume(3.0, 1.25), 1.25);
    assert_eq!(
        c.consume(-3.0, -0.04),
        -0.1,
        "nothing smaller than a tenth line is emitted"
    );
}

#[test]
fn small_motions_are_held_back_until_enough_has_accumulated() {
    let mut c = ScrollConsume::new(400.0);
    // The unit is 8 counts, the threshold 24.
    assert_eq!(c.consume(3.0, 0.5), 0.0);
    assert_eq!(c.consume(3.0, 0.5), 0.0);
    let mut total = 0.0;
    for _ in 0..10 {
        total += c.consume(3.0, 0.5);
    }
    assert!(total > 0.0);
}

#[test]
fn a_large_single_motion_is_emitted_at_once_in_tenths() {
    let mut c = ScrollConsume::new(400.0);
    assert_eq!(c.consume(20.0, 1.234), 1.2);
    // The remainder of 0.034 is carried to the next emission in the same direction.
    let next = c.consume(20.0, 0.07);
    assert!((next - 0.1).abs() < 1e-9, "{next}");
}

#[test]
fn reversing_drops_the_carried_remainder() {
    let mut c = ScrollConsume::new(400.0);
    c.consume(20.0, 1.234);
    let back = c.consume(-20.0, -0.5);
    assert!((back + 0.5).abs() < 1e-9, "{back}");
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
    let accel = settings.pointer_accel();
    let (x, y) = accel.accelerate(5.0, 0.0, 8.0);
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
