//! `kit::theme::system_color`: the colours the installed theme data names, in each appearance, once an application of
//! the process started on that data.

use cupertino_kit::app;
use cupertino_kit::theme::{system_color, Appearance};

#[test]
fn a_named_colour_is_the_theme_datas_in_each_appearance_once_an_application_started_on_it() {
    assert_eq!(system_color("system-green", Appearance::Light), None);
    app::environment().unwrap();
    let data = cupertino_assets::theme::load().unwrap();
    for appearance in Appearance::ALL {
        for name in ["system-green", "system-red", "no-such-colour"] {
            assert_eq!(
                system_color(name, appearance),
                data.tokens.color(appearance, name),
                "{name} in {appearance:?}"
            );
        }
    }
    assert!(system_color("system-green", Appearance::Dark).is_some());
}
