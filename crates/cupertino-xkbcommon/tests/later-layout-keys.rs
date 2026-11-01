//! A key that only a later layout of a keymap defines, like the extra key of the Brazilian ABNT2 layout after `us`, has
//! nothing in the first layout and its own symbols in the later one, as libxkbcommon compiles it.

use xkbcommon::xkb;

const EVDEV: u32 = 8;
const KEY_RO: u32 = 89;

#[test]
fn a_key_only_the_second_layout_defines_is_empty_in_the_first() {
    let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
    let keymap = xkb::Keymap::new_from_names(
        &context,
        "evdev",
        "pc105",
        "us,br",
        ",abnt2",
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .expect("the keymap compiles");
    let key = xkb::Keycode::new(KEY_RO + EVDEV);
    let names = |layout, level| -> Vec<String> {
        keymap
            .key_get_syms_by_level(key, layout, level)
            .iter()
            .map(|sym| xkb::keysym_get_name(*sym))
            .collect()
    };
    assert_eq!(keymap.num_layouts_for_key(key), 2);
    assert!(names(0, 0).is_empty(), "{:?}", names(0, 0));
    assert_eq!(
        (0..4)
            .map(|level| names(1, level).join(","))
            .collect::<Vec<_>>(),
        ["slash", "question", "degree", "questiondown"]
    );
}
