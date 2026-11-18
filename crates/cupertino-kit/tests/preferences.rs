//! `kit::test::preferences`: the theme preferences a driver is built with follow the session's appearance and the
//! display options it sets.

use cupertino_kit::platform::SystemAppearance;
use cupertino_kit::test::preferences;
use cupertino_kit::theme::Appearance;

#[test]
fn the_session_appearance_and_contrast_choose_one_of_four_themes_and_the_display_options_pass_through(
) {
    let rows = [
        (false, false, Appearance::Light),
        (true, false, Appearance::Dark),
        (false, true, Appearance::LightHighContrast),
        (true, true, Appearance::DarkHighContrast),
    ];
    for (dark, increase_contrast, expected) in rows {
        let session = SystemAppearance {
            dark,
            increase_contrast,
            ..SystemAppearance::default()
        };
        let got = preferences(&session);
        assert_eq!(
            got.appearance, expected,
            "dark {dark}, contrast {increase_contrast}"
        );
        assert!(!got.reduce_transparency && !got.reduce_motion);
    }
    let session = SystemAppearance {
        reduce_transparency: true,
        reduce_motion: true,
        ..SystemAppearance::default()
    };
    let got = preferences(&session);
    assert!(got.reduce_transparency && got.reduce_motion);
}
