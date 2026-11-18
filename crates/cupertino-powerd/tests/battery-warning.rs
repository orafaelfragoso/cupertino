use cupertino_powerd::warning::{Level, Reading, Warnings};

#[test]
fn the_warning_escalates_on_battery_and_waits_for_the_reading_to_settle_after_unplug() {
    let mut warnings = Warnings::new();
    for (seconds, plugged, percent, expected) in [
        (0.0, false, 7.0, Level::Early),
        (1.0, false, 1.0, Level::Final),
        (2.0, false, 12.0, Level::Final),
        (3.0, true, 3.0, Level::None),
        (30.0, false, 2.0, Level::None),
        (89.0, false, 2.0, Level::None),
        (90.0, false, 2.0, Level::Final),
    ] {
        let reading = Reading {
            on_battery: !plugged,
            charging: plugged,
            full: false,
            percentage: percent,
        };
        assert_eq!(
            warnings.update(seconds, Some(reading)),
            expected,
            "at {seconds}s"
        );
    }
    assert_eq!(warnings.update(91.0, None), Level::None);
}
