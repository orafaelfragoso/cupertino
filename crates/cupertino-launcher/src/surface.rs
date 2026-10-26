//! Translation of the toolkit's key events to the keys of the panel.

use crate::panel::{Key, Mods};
use cupertino_client::{key, Modifiers};

/// The panel key and modifiers of a key press, or `None` for keys the panel ignores (modifiers,
/// function keys). Named keys are recognised by their evdev code, so they work in every layout;
/// a key with a command modifier held (Ctrl+A, Alt+1, Super+Y) is named by its lower-case letter,
/// anything else types the character the layout produces.
pub fn panel_key(code: u32, mods: Modifiers, character: Option<char>) -> Option<(Key, Mods)> {
    let panel_mods = Mods { ctrl: mods.ctrl, alt: mods.alt, shift: mods.shift, primary: mods.logo };
    let named = match code {
        key::ENTER | key::KEYPAD_ENTER => Some(Key::Enter),
        key::ESCAPE => Some(Key::Escape),
        key::BACKSPACE => Some(Key::Backspace),
        key::DELETE => Some(Key::Delete),
        key::LEFT => Some(Key::Left),
        key::RIGHT => Some(Key::Right),
        key::UP => Some(Key::Up),
        key::DOWN => Some(Key::Down),
        key::HOME => Some(Key::Home),
        key::END => Some(Key::End),
        key::PAGE_UP => Some(Key::PageUp),
        key::PAGE_DOWN => Some(Key::PageDown),
        key::TAB => Some(Key::Tab),
        _ => None,
    };
    if let Some(named) = named {
        return Some((named, panel_mods));
    }
    let character = character?;
    let command = mods.ctrl || mods.alt || mods.logo;
    let character = if command { character.to_lowercase().next()? } else { character };
    Some((Key::Char(character), panel_mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: u32 = 30;
    const KEY_1: u32 = 2;
    const KEY_F5: u32 = 63;

    #[test]
    fn named_keys_keep_their_modifiers() {
        let shift = Modifiers { shift: true, ..Modifiers::default() };
        assert_eq!(panel_key(key::ENTER, Modifiers::default(), None), Some((Key::Enter, Mods::NONE)));
        assert_eq!(panel_key(key::TAB, shift, None), Some((Key::Tab, Mods { shift: true, ..Mods::NONE })));
        assert_eq!(panel_key(key::PAGE_DOWN, Modifiers::default(), None), Some((Key::PageDown, Mods::NONE)));
    }

    #[test]
    fn typing_uses_the_produced_character() {
        let shift = Modifiers { shift: true, ..Modifiers::default() };
        assert_eq!(panel_key(KEY_A, Modifiers::default(), Some('a')), Some((Key::Char('a'), Mods::NONE)));
        assert_eq!(panel_key(KEY_A, shift, Some('A')), Some((Key::Char('A'), Mods { shift: true, ..Mods::NONE })));
    }

    #[test]
    fn command_chords_are_named_by_the_lower_case_letter() {
        let ctrl = Modifiers { ctrl: true, shift: true, ..Modifiers::default() };
        assert_eq!(panel_key(KEY_A, ctrl, Some('A')), Some((Key::Char('a'), Mods { ctrl: true, shift: true, ..Mods::NONE })));
        let alt = Modifiers { alt: true, ..Modifiers::default() };
        assert_eq!(panel_key(KEY_1, alt, Some('1')), Some((Key::Char('1'), Mods::ALT)));
        let logo = Modifiers { logo: true, ..Modifiers::default() };
        assert_eq!(panel_key(KEY_A, logo, Some('a')), Some((Key::Char('a'), Mods::PRIMARY)));
    }

    #[test]
    fn keys_without_a_character_are_ignored() {
        assert_eq!(panel_key(KEY_F5, Modifiers::default(), None), None);
    }
}
