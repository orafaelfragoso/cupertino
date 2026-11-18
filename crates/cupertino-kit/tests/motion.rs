//! `kit::motion`: a value moves along a named timing or spring from the time it is told, rests on its target, and
//! reads the same at a time whatever times were read before it.

use cupertino_kit::motion::{Animated, Easing, SpringPreset};

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn a_value_follows_its_named_timing_and_rests_on_its_target_and_a_release_settles_on_a_named_spring(
) {
    let mut value = Animated::new(0.0_f64);
    assert!(!value.is_animating(0.0));
    value.animate_to(10.0, Easing::LINEAR.over(0.5), 1.0);
    assert_eq!(value.target(), 10.0);
    assert!(near(value.value(1.0), 0.0));
    assert!(
        near(value.value(1.25), 5.0),
        "half way through a linear half second"
    );
    assert!(value.is_animating(1.25));
    assert!(near(value.value(1.5), 10.0));
    assert!(!value.is_animating(1.6));
    assert!(near(value.value(9.0), 10.0));

    // reading the value at other times first changes nothing
    let mut other = Animated::new(0.0_f64);
    other.animate_to(10.0, Easing::LINEAR.over(0.5), 1.0);
    for t in [1.01, 1.02, 1.4, 1.1] {
        other.value(t);
    }
    assert!(near(other.value(1.25), 5.0));

    let mut dragged = Animated::new(0.0_f64);
    dragged.track(0.0, 0.0);
    dragged.track(50.0, 0.05);
    dragged.release(100.0, SpringPreset::DEFAULT, 0.05);
    assert!(dragged.is_animating(0.06));
    assert!(near(dragged.value(30.0), 100.0));
    assert!(!dragged.is_animating(30.0));
}
