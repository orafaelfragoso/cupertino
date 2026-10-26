//! Compiling the default `evdev`/`pc105`/`us` keymap against the host's XKB data logs no unknown keysym and, at the
//! default verbosity, no unresolved modifier map entry, and the modifier map still makes Alt_L the Mod1 modifier.

use std::sync::Mutex;
use xkbcommon::xkb;

const EVDEV: u32 = 8;
const KEY_LEFTALT: u32 = 56;

static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Collector;

impl log::Log for Collector {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        WARNINGS.lock().unwrap().push(record.args().to_string());
    }

    fn flush(&self) {}
}

#[test]
fn default_keymap_has_no_unknown_keysyms_and_alt_is_mod1() {
    log::set_logger(&Collector).unwrap();
    log::set_max_level(log::LevelFilter::Warn);
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_names(&context, "evdev", "pc105", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS).expect("the keymap compiles");

    let unknown: Vec<String> = WARNINGS.lock().unwrap().iter().filter(|w| w.contains("Unrecognized_keysym")).cloned().collect();
    assert_eq!(unknown, Vec::<String>::new());
    // pc105 maps Meta_R to Mod1 but `us` binds no key to it; libxkbcommon reports that only at verbosity 5
    let unresolved: Vec<String> = WARNINGS.lock().unwrap().iter().filter(|w| w.contains("not found in symbol map")).cloned().collect();
    assert_eq!(unresolved, Vec::<String>::new());

    let mut state = xkb::State::new(&keymap);
    state.update_key(xkb::Keycode::new(KEY_LEFTALT + EVDEV), xkb::KeyDirection::Down);
    assert!(state.mod_name_is_active("Mod1", xkb::STATE_MODS_EFFECTIVE));
}

#[test]
fn keysyms_added_since_the_engine_release_resolve() {
    for (name, value) in [("XF86AudioBassBoost", 0x1008_10d1), ("XF86OK", 0x1008_1160), ("XF86Fn_F1", 0x1008_11d2)] {
        assert_eq!(xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS).raw(), value, "{name}");
    }
}
