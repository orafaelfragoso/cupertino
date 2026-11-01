//! The keyboard of an open choice menu: Up and Down move the highlight (no wrap), Home and End jump to the ends,
//! Return and Space choose, Escape and the command key with . close without choosing, and typing jumps to the next item whose title
//! starts with the typed text. Every other key is swallowed so none reaches the pane underneath.

pub use cupertino_typeahead::TypeAhead;

use crate::keys::{code, Mods};

/// What the window does with a key while a menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The highlight moved to this item.
    Highlight(usize),
    /// This item was chosen.
    Choose(usize),
    /// The menu closes and the value stays.
    Cancel,
    /// The key does nothing, and goes nowhere.
    Ignore,
}

/// The result of `key` with `highlighted` (the item the keyboard or pointer is on, else the selected one) among
/// `choices`, at `now` seconds.
pub fn press(
    choices: &[String],
    highlighted: usize,
    type_ahead: &mut TypeAhead,
    now: f64,
    key: u32,
    mods: Mods,
    character: Option<char>,
) -> Outcome {
    let last = choices.len().saturating_sub(1);
    if choices.is_empty() {
        return if cancels(key, mods, character) {
            Outcome::Cancel
        } else {
            Outcome::Ignore
        };
    }
    let highlighted = highlighted.min(last);
    if cancels(key, mods, character) {
        return Outcome::Cancel;
    }
    if mods.logo || mods.ctrl || mods.alt {
        return Outcome::Ignore;
    }
    match key {
        code::UP => Outcome::Highlight(highlighted.saturating_sub(1)),
        code::DOWN => Outcome::Highlight((highlighted + 1).min(last)),
        code::HOME => Outcome::Highlight(0),
        code::END => Outcome::Highlight(last),
        code::ENTER | code::KEYPAD_ENTER => Outcome::Choose(highlighted),
        _ => match character {
            Some(' ') if !type_ahead.typing(now) => Outcome::Choose(highlighted),
            Some(c) if !c.is_control() => {
                let titles: Vec<Option<&str>> = choices.iter().map(|c| Some(c.as_str())).collect();
                match type_ahead.find(c, now, highlighted, &titles) {
                    Some(i) => Outcome::Highlight(i),
                    None => Outcome::Ignore,
                }
            }
            _ => Outcome::Ignore,
        },
    }
}

fn cancels(key: u32, mods: Mods, character: Option<char>) -> bool {
    key == code::ESCAPE || (mods.logo && !mods.ctrl && !mods.alt && character == Some('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<String> {
        ["Automatic", "400", "800", "Custom", "Cursor"]
            .map(String::from)
            .to_vec()
    }

    fn typed(at: f64, c: char, from: usize, ta: &mut TypeAhead) -> Outcome {
        press(&items(), from, ta, at, 0, Mods::default(), Some(c))
    }

    #[test]
    fn arrows_move_the_highlight_and_stop_at_the_ends() {
        let go = |key, from| {
            press(
                &items(),
                from,
                &mut TypeAhead::default(),
                0.0,
                key,
                Mods::default(),
                None,
            )
        };
        assert_eq!(go(code::DOWN, 1), Outcome::Highlight(2));
        assert_eq!(go(code::DOWN, 4), Outcome::Highlight(4));
        assert_eq!(go(code::UP, 0), Outcome::Highlight(0));
        assert_eq!(go(code::END, 0), Outcome::Highlight(4));
        assert_eq!(go(code::HOME, 3), Outcome::Highlight(0));
    }

    #[test]
    fn return_and_space_choose_and_escape_or_super_period_cancel() {
        let mut ta = TypeAhead::default();
        let m = Mods::default();
        let it = items();
        assert_eq!(
            press(&it, 2, &mut ta, 0.0, code::ENTER, m, None),
            Outcome::Choose(2)
        );
        assert_eq!(
            press(&it, 2, &mut ta, 0.0, code::KEYPAD_ENTER, m, None),
            Outcome::Choose(2)
        );
        assert_eq!(
            press(&it, 2, &mut ta, 0.0, 57, m, Some(' ')),
            Outcome::Choose(2)
        );
        assert_eq!(
            press(&it, 2, &mut ta, 0.0, code::ESCAPE, m, None),
            Outcome::Cancel
        );
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            press(&it, 2, &mut ta, 0.0, 52, logo, Some('.')),
            Outcome::Cancel
        );
    }

    #[test]
    fn typing_jumps_to_a_matching_item_and_a_repeated_letter_steps_through_them() {
        let mut ta = TypeAhead::default();
        assert_eq!(typed(0.0, 'c', 0, &mut ta), Outcome::Highlight(3));
        let mut ta = TypeAhead::default();
        assert_eq!(typed(5.0, 'c', 3, &mut ta), Outcome::Highlight(4));
        let mut ta = TypeAhead::default();
        assert_eq!(typed(0.0, 'c', 0, &mut ta), Outcome::Highlight(3));
        assert_eq!(typed(0.3, 'u', 3, &mut ta), Outcome::Highlight(3));
        assert_eq!(typed(0.6, 'r', 3, &mut ta), Outcome::Highlight(4));
        let mut ta = TypeAhead::default();
        assert_eq!(typed(0.0, 'z', 0, &mut ta), Outcome::Ignore);
        let mut ta = TypeAhead::default();
        typed(0.0, '8', 0, &mut ta);
        assert_eq!(typed(0.2, '0', 2, &mut ta), Outcome::Highlight(2));
    }

    #[test]
    fn a_pause_starts_a_new_search_and_space_is_a_character_while_typing() {
        let mut ta = TypeAhead::default();
        let it = items();
        typed(0.0, 'c', 0, &mut ta);
        assert_eq!(
            press(&it, 3, &mut ta, 0.5, 57, Mods::default(), Some(' ')),
            Outcome::Ignore
        );
        assert_eq!(typed(3.0, 'a', 3, &mut ta), Outcome::Highlight(0));
    }

    #[test]
    fn other_keys_and_chords_are_swallowed() {
        let mut ta = TypeAhead::default();
        let it = items();
        assert_eq!(
            press(&it, 1, &mut ta, 0.0, code::TAB, Mods::default(), None),
            Outcome::Ignore
        );
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        assert_eq!(
            press(&it, 1, &mut ta, 0.0, 26, logo, Some('[')),
            Outcome::Ignore
        );
    }

    #[test]
    fn the_command_key_with_a_period_cancels_whichever_key_that_is() {
        use cupertino_modifiers::Profile;
        use cupertino_ui_text::Keymap;
        let it = items();
        let cancels_with = |profile: Profile, ctrl: bool, logo: bool| {
            let keymap = Keymap::standard_for(profile);
            let mods = crate::keys::command_mods(&keymap, false, ctrl, false, logo);
            press(&it, 2, &mut TypeAhead::default(), 0.0, 52, mods, Some('.')) == Outcome::Cancel
        };
        assert!(
            cancels_with(Profile::cupertino(), false, true),
            "Super+. under cupertino"
        );
        assert!(
            !cancels_with(Profile::cupertino(), true, false),
            "Ctrl+. under cupertino"
        );
        assert!(
            cancels_with(Profile::classic_pc(), true, false),
            "Ctrl+. under classic-pc"
        );
        assert!(
            !cancels_with(Profile::classic_pc(), false, true),
            "Super+. under classic-pc"
        );
    }
}
