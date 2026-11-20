//! The control panel's detail views move on the spring and the cross-fade they were designed with.

use cupertino_theme::animation::{Easing, SpringPreset, Timing};

#[test]
fn a_detail_view_grows_critically_damped_with_an_undamped_period_of_0_35_seconds() {
    let growth = Timing::from(SpringPreset::CONTROL_PANEL_DETAIL);
    let rate = std::f64::consts::TAU / 0.35;
    for t in [0.02_f64, 0.05, 0.1, 0.2, 0.35, 0.5] {
        let expected = 1.0 - (1.0 + rate * t) * (-rate * t).exp();
        assert!((growth.value(t) - expected).abs() < 1e-12, "{t}");
    }
    assert_eq!(growth.duration(), 0.6);
}

#[test]
fn with_reduce_motion_a_detail_view_cross_fades_at_constant_speed_over_a_fifth_of_a_second() {
    let fade = Timing::CONTROL_PANEL_DETAIL_FADE;
    assert_eq!(fade, Easing::LINEAR.over(0.2));
    for t in [0.0, 0.05, 0.1, 0.15, 0.2] {
        assert!((fade.value(t) - t / 0.2).abs() < 1e-9, "{t}");
    }
}
