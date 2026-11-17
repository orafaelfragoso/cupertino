//! The timing-function table names the design's curves and its default spring.

use cupertino_theme::animation::{
    TimingPreset, DEFAULT_SPRING, EASE_CURVE, EASE_IN_CURVE, EASE_IN_OUT_CURVE, EASE_OUT_CURVE,
    LINEAR_CURVE,
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
