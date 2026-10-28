//! The chords the menu bar shows for the Kit's standard items are the keys the compositor binds to the same actions.

use cupertino_input::shortcuts::{Action, ShortcutMap};
use cupertino_input::Modifiers;
use cupertino_menu_model::standard::{self, Features};
use cupertino_menu_model::{Command, Entry};
use cupertino_modifiers::Profile;

/// Full screen in the standard View menu, as an accelerator under `profile`.
fn full_screen_accel(profile: &Profile) -> String {
    let bar = standard::menu_bar("App", &Features::default());
    let item = bar
        .menus
        .iter()
        .flat_map(|menu| menu.sections.iter().flatten())
        .find_map(|entry| match entry {
            Entry::Item(item)
                if matches!(&item.command, Command::Action(a) if a.full() == "win.fullscreen") =>
            {
                Some(item)
            }
            _ => None,
        })
        .expect("the standard View menu has full screen");
    item.chord
        .as_ref()
        .expect("full screen has a chord")
        .to_accel(profile)
        .expect("the profile makes every modifier")
}

#[test]
fn full_screen_shows_the_compositors_default_chord() {
    let accel = full_screen_accel(&Profile::cupertino());
    let modifiers = Modifiers {
        shift: accel.contains("<Shift>"),
        ctrl: accel.contains("<Control>"),
        alt: accel.contains("<Alt>"),
        super_key: accel.contains("<Super>"),
    };
    let key = accel
        .rsplit('>')
        .next()
        .expect("an accelerator names a key");
    assert_eq!(key, "f", "{accel}");
    assert_eq!(
        ShortcutMap::defaults().lookup(modifiers, u32::from('f')),
        Some(&Action::ToggleFullscreen),
        "{accel}"
    );
}
