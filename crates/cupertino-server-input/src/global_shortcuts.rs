//! Pure modifier and reserved-chord helpers for `cupertino_global_shortcuts_v1`.

use cupertino_protocols::server::global_shortcuts::cupertino_global_shortcuts_v1::Modifier;
use smithay::input::keyboard::keysyms;

use crate::input::{Chord, KeyBindings, Mods};

/// The modifiers of a chord as the protocol carries them.
pub fn modifier_bits(mods: Mods) -> Modifier {
    let mut bits = Modifier::empty();
    for (held, bit) in [
        (mods.shift, Modifier::Shift),
        (mods.ctrl, Modifier::Ctrl),
        (mods.alt, Modifier::Alt),
        (mods.logo, Modifier::Logo),
    ] {
        if held {
            bits |= bit;
        }
    }
    bits
}

/// The modifiers of the protocol's bits; unknown bits are ignored.
pub fn modifiers_of(bits: u32) -> Mods {
    let bits = Modifier::from_bits_truncate(bits);
    Mods {
        shift: bits.contains(Modifier::Shift),
        ctrl: bits.contains(Modifier::Ctrl),
        alt: bits.contains(Modifier::Alt),
        logo: bits.contains(Modifier::Logo),
    }
}

/// Whether `sym` is a modifier or lock key (Shift to Hyper, Caps Lock and Shift Lock among them; the ISO level, latch
/// and group keys; `Mode_switch` and `Num_Lock`): no shortcut of the portal takes it, so the focused client always sees
/// its modifiers.
pub fn is_modifier_key(sym: u32) -> bool {
    matches!(
        sym,
        keysyms::KEY_ISO_Lock..=keysyms::KEY_ISO_Level5_Lock
            | keysyms::KEY_Mode_switch..=keysyms::KEY_Num_Lock
            | keysyms::KEY_Shift_L..=keysyms::KEY_Hyper_R
    )
}

/// The chords the server keeps for itself: its bindings, the console switches, the screen reader's chord and the
/// chord that ends desktop control, each once.
pub fn reserved(bindings: &KeyBindings) -> Vec<Chord> {
    let ctrl_alt = Mods {
        ctrl: true,
        alt: true,
        ..Mods::NONE
    };
    let mut chords: Vec<Chord> = bindings.entries().iter().map(|(c, _)| *c).collect();
    chords.extend((keysyms::KEY_F1..=keysyms::KEY_F12).map(|sym| Chord {
        mods: ctrl_alt,
        sym,
    }));
    chords.push(Chord {
        mods: Mods {
            logo: true,
            alt: true,
            ..Mods::NONE
        },
        sym: keysyms::KEY_s,
    });
    chords.push(Chord {
        mods: ctrl_alt,
        sym: keysyms::KEY_Escape,
    });
    let mut unique = Vec::with_capacity(chords.len());
    for chord in chords {
        if !unique.contains(&chord) {
            unique.push(chord);
        }
    }
    unique
}
