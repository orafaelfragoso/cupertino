//! Keyboard filter state and pure rewrite helpers (slow keys, sticky keys, mouse keys, globe tap,
//! Super tap, compositor key repeat, and modifier-mask rewriting).

use std::collections::{HashMap, HashSet};

use cupertino_keyboard::modifiers::RawMods;
use cupertino_keyboard::modifiers::{FnUsage, GlobeTap, GLOBE_DEFAULT_REPEAT_MS};
use cupertino_keyboard::mouse_keys::MouseKeys;
use cupertino_keyboard::repeat::KeyRepeater;
use cupertino_keyboard::slow_keys::SlowKeys;
use cupertino_keyboard::sticky_keys::StickyKeys;
use smithay::input::keyboard::{xkb, Keycode, SerializedMods};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::kbd::KeyboardSettings;
use crate::overlay_types::OverlayKey;

/// evdev `KEY_FN` (the globe / fn key on keyboards that report it).
pub const KEY_FN: u32 = 464;
/// Offset between evdev codes and the keycodes of the keymap.
pub const XKB_OFFSET: u32 = 8;

pub fn evdev(code: Keycode) -> u32 {
    code.raw().saturating_sub(XKB_OFFSET)
}

/// evdev `KEY_LEFTMETA` and `KEY_RIGHTMETA`, the Super keys.
const SUPER_KEYS: [u32; 2] = [125, 126];

/// How long the Super key may stay down alone and still be a tap, in ms. approximate.
const SUPER_TAP_MAX_MS: u64 = 750;

/// The Super key pressed and released alone: no other key and no pointer button between, and not held long.
#[derive(Debug, Default)]
pub struct SuperTap {
    /// When the Super key went down with nothing else down; `None` once anything else happened.
    down: Option<u64>,
}

impl SuperTap {
    /// A key event. `others_held`: another key was held when this one went down. True when it is the release that
    /// completes a tap.
    pub fn key(&mut self, evdev: u32, pressed: bool, now_ms: u64, others_held: bool) -> bool {
        if !SUPER_KEYS.contains(&evdev) {
            if pressed {
                self.down = None;
            }
            return false;
        }
        if pressed {
            self.down = (!others_held).then_some(now_ms);
            return false;
        }
        self.down
            .take()
            .is_some_and(|down| now_ms.saturating_sub(down) <= SUPER_TAP_MAX_MS)
    }

    /// A pointer button or a scroll: the Super key is a modifier of it, not a tap.
    pub fn disturb(&mut self) {
        self.down = None;
    }
}

/// What a compositor key repeat acts on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RepeatTarget {
    /// A key the compositor consumed: its press is replayed while it and the same modifiers stay down.
    Compositor(OverlayKey),
    /// A held letter with accent alternatives: the first repeat opens the accent picker.
    AccentTrigger(char),
}

/// State of the filters (built from `keyboard.conf`).
pub struct KeyFilters {
    /// `Some` while slow keys are on.
    pub slow: Option<SlowKeys>,
    pub sticky: StickyKeys,
    pub mouse: MouseKeys,
    pub globe: GlobeTap,
    pub super_tap: SuperTap,
    pub fn_usage: FnUsage,
    /// The compositor's repeating key (xkb keycode) and what it repeats.
    pub repeat: KeyRepeater,
    pub repeat_target: Option<RepeatTarget>,
    /// Evdev codes of keys found held when a device was (re)read; they reach the keyboard unfiltered until released.
    pub unfiltered: HashSet<u32>,
    /// Keys held with a rewrite (xkb keycode of the physical key): what the client got for each.
    pub rewritten: HashMap<u32, Sent>,
    /// The client was last told modifiers that differ from the keyboard's.
    pub override_active: bool,
}

/// What a client was sent in place of a rewritten key.
pub struct Sent {
    /// The key that went to the client: the physical key, or the replacement of a cursor chord.
    pub code: Keycode,
    /// The client it went to; a release is not followed by a modifier update when the focus has moved on.
    pub focus: WlSurface,
}

/// xkb indices of the real modifiers the rewrite changes, read from the live keymap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModIndices {
    pub shift: u32,
    pub control: u32,
    pub mod1: u32,
    pub mod4: u32,
    pub mod5: u32,
}

impl ModIndices {
    /// The real modifiers are part of every xkb keymap, so a missing one is a broken keymap.
    pub fn of(keymap: &xkb::Keymap) -> Self {
        let index = |name: &str| {
            let index = keymap.mod_get_index(name);
            assert_ne!(index, xkb::MOD_INVALID, "the keymap has no {name} modifier");
            index
        };
        Self {
            shift: index(xkb::MOD_NAME_SHIFT),
            control: index(xkb::MOD_NAME_CTRL),
            mod1: index(xkb::MOD_NAME_ALT),
            mod4: index(xkb::MOD_NAME_LOGO),
            mod5: index(xkb::MOD_NAME_ISO_LEVEL3_SHIFT),
        }
    }

    /// Mask of the modifier bits that go away with `mods`: Alt takes the third-symbol-level bit with it, which the
    /// layouts of the input sources set from the same keys.
    pub fn removed(&self, mods: RawMods) -> u32 {
        let bit = |index: u32| 1u32 << index;
        let mut mask = 0;
        for (raw, bits) in [
            (RawMods::SHIFT, bit(self.shift)),
            (RawMods::CTRL, bit(self.control)),
            (RawMods::ALT, bit(self.mod1) | bit(self.mod5)),
            (RawMods::SUPER, bit(self.mod4)),
        ] {
            if mods.contains(raw) {
                mask |= bits;
            }
        }
        mask
    }

    /// Mask of the modifier bits that `mods` puts down.
    pub fn added(&self, mods: RawMods) -> u32 {
        let bit = |index: u32| 1u32 << index;
        let mut mask = 0;
        for (raw, bits) in [
            (RawMods::SHIFT, bit(self.shift)),
            (RawMods::CTRL, bit(self.control)),
            (RawMods::ALT, bit(self.mod1)),
            (RawMods::SUPER, bit(self.mod4)),
        ] {
            if mods.contains(raw) {
                mask |= bits;
            }
        }
        mask
    }
}

/// The modifiers to tell a client for a rewritten key: `real` without `remove` and with `add` held down. Locked
/// modifiers (caps and num lock) and the layout stay as they are.
pub fn rewrite_masks(
    real: SerializedMods,
    idx: &ModIndices,
    remove: RawMods,
    add: RawMods,
) -> SerializedMods {
    let gone = idx.removed(remove);
    SerializedMods {
        depressed: (real.depressed & !gone) | idx.added(add),
        latched: real.latched & !gone,
        ..real
    }
}

/// The key that types `sym` at the first level of the layout `state` gives it; else, the way the shortcut matcher reads
/// a key whose own symbol is not ASCII (`raw_latin_sym_or_raw_current_sym`), the key whose first ASCII symbol in another
/// layout is `sym`: the key a Latin user presses for it. `sym` is a first-level symbol, so a letter is its lower case.
pub fn key_for_sym(keymap: &xkb::Keymap, state: &xkb::State, sym: u32) -> Option<Keycode> {
    let first = |code: Keycode, layout: u32| {
        keymap
            .key_get_syms_by_level(code, layout, 0)
            .first()
            .copied()
    };
    let latin = |code: Keycode, own: u32| {
        (0..keymap.num_layouts())
            .filter(|l| *l != own)
            .filter_map(|l| first(code, l))
            .find(|s| {
                s.key_char()
                    .is_some_and(|c| c.is_ascii() && !c.is_ascii_control())
            })
    };
    let keys = || (keymap.min_keycode().raw()..=keymap.max_keycode().raw()).map(Keycode::new);
    let typed = |code: Keycode| first(code, state.key_get_layout(code));
    keys()
        .find(|&code| typed(code).is_some_and(|s| s.raw() == sym))
        .or_else(|| {
            keys().find(|&code| {
                typed(code).is_some_and(|s| s.key_char().is_some_and(|c| !c.is_ascii()))
                    && latin(code, state.key_get_layout(code)).is_some_and(|s| s.raw() == sym)
            })
        })
}

impl KeyFilters {
    pub fn new(settings: &KeyboardSettings) -> Self {
        let a = &settings.access;
        let repeat = settings.effective_repeat();
        // the translator's tap limit follows the configured initial repeat delay, 250 ms when repeat is off
        let globe_ms = if repeat.is_enabled() {
            f64::from(repeat.initial_delay_ms)
        } else {
            GLOBE_DEFAULT_REPEAT_MS
        };
        Self {
            slow: a.slow_keys.then(|| SlowKeys::new(a.slow_keys_delay_ms)),
            sticky: StickyKeys::new(a.sticky_keys, a.sticky_keys_shortcut),
            mouse: MouseKeys::new(
                a.mouse_keys,
                a.mouse_keys_shortcut,
                a.mouse_keys_delay_ms,
                a.mouse_keys_max_speed,
            ),
            globe: GlobeTap::new(globe_ms),
            super_tap: SuperTap::default(),
            fn_usage: settings.fn_action(),
            repeat: KeyRepeater::new(repeat),
            repeat_target: None,
            unfiltered: HashSet::new(),
            rewritten: HashMap::new(),
            override_active: false,
        }
    }

    /// The device reported `code` (evdev) as already held: its press and release skip the filters, because no filter
    /// saw the keystroke that put it down.
    pub fn pass_unfiltered(&mut self, code: u32) {
        self.unfiltered.insert(code);
    }
}
