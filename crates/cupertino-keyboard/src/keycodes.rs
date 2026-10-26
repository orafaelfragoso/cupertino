//! Key numbering used by the layout data ("layout key codes") and its relation to evdev codes and XKB key names.
//!
//! The numbers are the hardware-independent key codes of the design platform's public key code header;
//! the evdev side is the Linux input header. The two keys that differ between keyboard
//! families are position dependent: on ISO keyboards layout code 0x0A is the key at the top left (`TLDE`) and 0x32 the
//! key left of Z (`LSGT`); on ANSI keyboards 0x32 is the top-left key and 0x0A does not exist (derived from the
//! layout data).

/// (layout key code, evdev code, XKB key name) for every key of the table; ANSI positions for 0x0A / 0x32.
pub const LAYOUT_KEY_TABLE: &[(u8, u32, &str)] = &[
    (0x00, 30, "AC01"),
    (0x01, 31, "AC02"),
    (0x02, 32, "AC03"),
    (0x03, 33, "AC04"),
    (0x04, 35, "AC06"),
    (0x05, 34, "AC05"),
    (0x06, 44, "AB01"),
    (0x07, 45, "AB02"),
    (0x08, 46, "AB03"),
    (0x09, 47, "AB04"),
    (0x0a, 86, "LSGT"),
    (0x0b, 48, "AB05"),
    (0x0c, 16, "AD01"),
    (0x0d, 17, "AD02"),
    (0x0e, 18, "AD03"),
    (0x0f, 19, "AD04"),
    (0x10, 21, "AD06"),
    (0x11, 20, "AD05"),
    (0x12, 2, "AE01"),
    (0x13, 3, "AE02"),
    (0x14, 4, "AE03"),
    (0x15, 5, "AE04"),
    (0x16, 7, "AE06"),
    (0x17, 6, "AE05"),
    (0x18, 13, "AE12"),
    (0x19, 10, "AE09"),
    (0x1a, 8, "AE07"),
    (0x1b, 12, "AE11"),
    (0x1c, 9, "AE08"),
    (0x1d, 11, "AE10"),
    (0x1e, 27, "AD12"),
    (0x1f, 24, "AD09"),
    (0x20, 22, "AD07"),
    (0x21, 26, "AD11"),
    (0x22, 23, "AD08"),
    (0x23, 25, "AD10"),
    (0x24, 28, "RTRN"),
    (0x25, 38, "AC09"),
    (0x26, 36, "AC07"),
    (0x27, 40, "AC11"),
    (0x28, 37, "AC08"),
    (0x29, 39, "AC10"),
    (0x2a, 43, "BKSL"),
    (0x2b, 51, "AB08"),
    (0x2c, 53, "AB10"),
    (0x2d, 49, "AB06"),
    (0x2e, 50, "AB07"),
    (0x2f, 52, "AB09"),
    (0x30, 15, "TAB"),
    (0x31, 57, "SPCE"),
    (0x32, 41, "TLDE"),
    (0x33, 14, "BKSP"),
    (0x35, 1, "ESC"),
    (0x36, 126, "RWIN"),
    (0x37, 125, "LWIN"),
    (0x38, 42, "LFSH"),
    (0x39, 58, "CAPS"),
    (0x3a, 56, "LALT"),
    (0x3b, 29, "LCTL"),
    (0x3c, 54, "RTSH"),
    (0x3d, 100, "RALT"),
    (0x3e, 97, "RCTL"),
    (0x3f, 464, "FN"),
    (0x40, 187, "FK17"),
    (0x41, 83, "KPDL"),
    (0x43, 55, "KPMU"),
    (0x45, 78, "KPAD"),
    (0x47, 69, "NMLK"),
    (0x48, 115, "VOL+"),
    (0x49, 114, "VOL-"),
    (0x4a, 113, "MUTE"),
    (0x4b, 98, "KPDV"),
    (0x4c, 96, "KPEN"),
    (0x4e, 74, "KPSU"),
    (0x4f, 188, "FK18"),
    (0x50, 189, "FK19"),
    (0x51, 117, "KPEQ"),
    (0x52, 82, "KP0"),
    (0x53, 79, "KP1"),
    (0x54, 80, "KP2"),
    (0x55, 81, "KP3"),
    (0x56, 75, "KP4"),
    (0x57, 76, "KP5"),
    (0x58, 77, "KP6"),
    (0x59, 71, "KP7"),
    (0x5a, 190, "FK20"),
    (0x5b, 72, "KP8"),
    (0x5c, 73, "KP9"),
    (0x5d, 124, "AE13"),
    (0x5e, 89, "AB11"),
    (0x5f, 95, "JPCM"),
    (0x60, 63, "FK05"),
    (0x61, 64, "FK06"),
    (0x62, 65, "FK07"),
    (0x63, 61, "FK03"),
    (0x64, 66, "FK08"),
    (0x65, 67, "FK09"),
    (0x66, 94, "MUHE"),
    (0x67, 87, "FK11"),
    (0x68, 92, "HENK"),
    (0x69, 183, "FK13"),
    (0x6a, 186, "FK16"),
    (0x6b, 184, "FK14"),
    (0x6d, 68, "FK10"),
    (0x6e, 127, "COMP"),
    (0x6f, 88, "FK12"),
    (0x71, 185, "FK15"),
    (0x72, 138, "HELP"),
    (0x73, 102, "HOME"),
    (0x74, 104, "PGUP"),
    (0x75, 111, "DELE"),
    (0x76, 62, "FK04"),
    (0x77, 107, "END"),
    (0x78, 60, "FK02"),
    (0x79, 109, "PGDN"),
    (0x7a, 59, "FK01"),
    (0x7b, 105, "LEFT"),
    (0x7c, 106, "RGHT"),
    (0x7d, 108, "DOWN"),
    (0x7e, 103, "UP"),
];

pub fn evdev_of(layout_key: u8) -> Option<u32> {
    LAYOUT_KEY_TABLE
        .iter()
        .find(|(k, _, _)| *k == layout_key)
        .map(|(_, e, _)| *e)
}

pub fn layout_key_of(evdev: u32) -> Option<u8> {
    LAYOUT_KEY_TABLE
        .iter()
        .find(|(_, e, _)| *e == evdev)
        .map(|(k, _, _)| *k)
}

pub fn xkb_name_of(layout_key: u8) -> Option<&'static str> {
    LAYOUT_KEY_TABLE
        .iter()
        .find(|(k, _, _)| *k == layout_key)
        .map(|(_, _, n)| *n)
}

/// Keys of the alphanumeric block that carry characters: everything up to 0x32 except Return and Tab, plus the
/// three extra keys of Japanese keyboards (0x5D yen, 0x5E ro, 0x5F keypad comma).
pub fn is_character_key(layout_key: u8) -> bool {
    (layout_key <= 0x32 && layout_key != 0x24 && layout_key != 0x30)
        || (0x5D..=0x5F).contains(&layout_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_consistent() {
        assert_eq!(LAYOUT_KEY_TABLE.len(), 120);
        assert_eq!(evdev_of(0x00), Some(30));
        assert_eq!(layout_key_of(57), Some(0x31));
        assert_eq!(xkb_name_of(0x32), Some("TLDE"));
        for (i, a) in LAYOUT_KEY_TABLE.iter().enumerate() {
            for b in &LAYOUT_KEY_TABLE[i + 1..] {
                assert_ne!(a.0, b.0);
                assert_ne!(a.1, b.1, "duplicate evdev {}", a.1);
            }
        }
    }

    #[test]
    fn character_keys() {
        assert!(is_character_key(0x00));
        assert!(!is_character_key(0x24));
        assert!(!is_character_key(0x30));
        assert!(is_character_key(0x31));
        assert!(is_character_key(0x5E));
        assert!(!is_character_key(0x7B));
    }
}
