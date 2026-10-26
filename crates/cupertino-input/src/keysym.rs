//! Keysym names and values (the X11 / xkbcommon keysym namespace, an open standard).
//!
//! Only the keys a compositor shortcut map needs are named; any other keysym can be written as
//! `0x…` hexadecimal or `U00e9` (Unicode code point) in a shortcut string.

pub const TAB: u32 = 0xff09;
pub const ISO_LEFT_TAB: u32 = 0xfe20;

const NAMED: &[(&str, u32)] = &[
    ("BackSpace", 0xff08),
    ("Tab", TAB),
    ("ISO_Left_Tab", ISO_LEFT_TAB),
    ("Return", 0xff0d),
    ("Pause", 0xff13),
    ("Scroll_Lock", 0xff14),
    ("Escape", 0xff1b),
    ("Delete", 0xffff),
    ("Home", 0xff50),
    ("Left", 0xff51),
    ("Up", 0xff52),
    ("Right", 0xff53),
    ("Down", 0xff54),
    ("Page_Up", 0xff55),
    ("Prior", 0xff55),
    ("Page_Down", 0xff56),
    ("Next", 0xff56),
    ("End", 0xff57),
    ("Print", 0xff61),
    ("Insert", 0xff63),
    ("Menu", 0xff67),
    ("Num_Lock", 0xff7f),
    ("KP_Enter", 0xff8d),
    ("space", 0x20),
    ("exclam", 0x21),
    ("quotedbl", 0x22),
    ("numbersign", 0x23),
    ("dollar", 0x24),
    ("percent", 0x25),
    ("ampersand", 0x26),
    ("apostrophe", 0x27),
    ("parenleft", 0x28),
    ("parenright", 0x29),
    ("asterisk", 0x2a),
    ("plus", 0x2b),
    ("comma", 0x2c),
    ("minus", 0x2d),
    ("period", 0x2e),
    ("slash", 0x2f),
    ("colon", 0x3a),
    ("semicolon", 0x3b),
    ("less", 0x3c),
    ("equal", 0x3d),
    ("greater", 0x3e),
    ("question", 0x3f),
    ("at", 0x40),
    ("bracketleft", 0x5b),
    ("backslash", 0x5c),
    ("bracketright", 0x5d),
    ("asciicircum", 0x5e),
    ("underscore", 0x5f),
    ("grave", 0x60),
    ("braceleft", 0x7b),
    ("bar", 0x7c),
    ("braceright", 0x7d),
    ("asciitilde", 0x7e),
    ("XF86MonBrightnessUp", 0x1008ff02),
    ("XF86MonBrightnessDown", 0x1008ff03),
    ("XF86KbdBrightnessUp", 0x1008ff05),
    ("XF86KbdBrightnessDown", 0x1008ff06),
    ("XF86AudioLowerVolume", 0x1008ff11),
    ("XF86AudioMute", 0x1008ff12),
    ("XF86AudioRaiseVolume", 0x1008ff13),
    ("XF86AudioPlay", 0x1008ff14),
    ("XF86AudioStop", 0x1008ff15),
    ("XF86AudioPrev", 0x1008ff16),
    ("XF86AudioNext", 0x1008ff17),
    ("XF86PowerOff", 0x1008ff2a),
    ("XF86Eject", 0x1008ff2c),
    ("XF86ScreenSaver", 0x1008ff2d),
    ("XF86Sleep", 0x1008ff2f),
];

/// Keysym for a name such as `Tab`, `grave`, `a`, `5`, `F11`, `KP_3`, `U00e9` or `0xff09`.
/// Single ASCII letters are case-sensitive as in xkbcommon (`A` is a different keysym from `a`).
pub fn from_name(name: &str) -> Option<u32> {
    if name.chars().count() == 1 {
        let c = name.chars().next()?;
        if c.is_ascii_graphic() {
            return Some(c as u32);
        }
    }
    if let Some((_, v)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(*v);
    }
    if let Some(hex) = name.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Some(n) = name.strip_prefix('F') {
        if let Ok(n) = n.parse::<u32>() {
            if (1..=35).contains(&n) {
                return Some(0xffbd + n);
            }
        }
    }
    if let Some(n) = name.strip_prefix("KP_") {
        if let Ok(n) = n.parse::<u32>() {
            if n <= 9 {
                return Some(0xffb0 + n);
            }
        }
    }
    if let Some(cp) = name.strip_prefix('U') {
        if cp.len() >= 4 && cp.chars().all(|c| c.is_ascii_hexdigit()) {
            let cp = u32::from_str_radix(cp, 16).ok()?;
            if char::from_u32(cp).is_some() {
                return Some(if cp < 0x100 { cp } else { 0x0100_0000 + cp });
            }
        }
    }
    None
}

/// A canonical name for a keysym, if it has one in the table.
pub fn name_of(keysym: u32) -> Option<String> {
    if (0x21..=0x7e).contains(&keysym) {
        if let Some((n, _)) = NAMED.iter().find(|(_, v)| *v == keysym) {
            return Some((*n).to_string());
        }
        return char::from_u32(keysym).map(|c| c.to_string());
    }
    if (0xffbe..=0xffe0).contains(&keysym) {
        return Some(format!("F{}", keysym - 0xffbd));
    }
    if (0xffb0..=0xffb9).contains(&keysym) {
        return Some(format!("KP_{}", keysym - 0xffb0));
    }
    NAMED
        .iter()
        .find(|(_, v)| *v == keysym)
        .map(|(n, _)| (*n).to_string())
}

/// Fold a keysym to the form shortcuts are matched on: ASCII letters lower-cased.
pub fn fold_case(keysym: u32) -> u32 {
    if (0x41..=0x5a).contains(&keysym) {
        keysym + 0x20
    } else {
        keysym
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_to_xkb_values() {
        assert_eq!(from_name("Tab"), Some(0xff09));
        assert_eq!(from_name("Escape"), Some(0xff1b));
        assert_eq!(from_name("Left"), Some(0xff51));
        assert_eq!(from_name("space"), Some(0x20));
        assert_eq!(from_name("grave"), Some(0x60));
        assert_eq!(from_name("a"), Some(0x61));
        assert_eq!(from_name("A"), Some(0x41));
        assert_eq!(from_name("7"), Some(0x37));
        assert_eq!(from_name("F1"), Some(0xffbe));
        assert_eq!(from_name("F12"), Some(0xffc9));
        assert_eq!(from_name("KP_3"), Some(0xffb3));
        assert_eq!(from_name("XF86AudioMute"), Some(0x1008ff12));
        assert_eq!(from_name("0xff09"), Some(0xff09));
        assert_eq!(from_name("U00e9"), Some(0xe9));
        assert_eq!(from_name("U20ac"), Some(0x0100_20ac));
        assert_eq!(from_name("NoSuchKey"), None);
        assert_eq!(from_name("F99"), None);
    }

    #[test]
    fn reverse_lookup_and_case_folding() {
        assert_eq!(name_of(0xff09).as_deref(), Some("Tab"));
        assert_eq!(name_of(0x60).as_deref(), Some("grave"));
        assert_eq!(name_of(0x61).as_deref(), Some("a"));
        assert_eq!(name_of(0xffc9).as_deref(), Some("F12"));
        assert_eq!(fold_case(0x41), 0x61);
        assert_eq!(fold_case(0x61), 0x61);
        assert_eq!(fold_case(0x31), 0x31);
    }
}
