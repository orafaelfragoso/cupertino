//! The window's keyboard: the navigation chords (names,: Cmd-[ back, Cmd-] forward, Cmd-L Show All,
//! Cmd-F search; `logo` is the command key, which is Super under `cupertino` and Ctrl under `classic-pc`: see
//! [`command_mods`]) and the keys of the focused part (§7.1). There is no in-window
//! menu, so the chords are read from key events (gap G9).

/// Where the keyboard input goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Focus {
    Search,
    #[default]
    Sidebar,
    Pane,
}

/// What a key asks the window to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Back,
    Forward,
    /// Focus the search field (Super+F).
    FocusSearch,
    /// Show All: the initial pane, pushed on the history (Super+L; O9).
    ShowAll,
    /// Sidebar keys: the previous or next row, the first or last one (native sidebar behaviour).
    Previous,
    Next,
    First,
    Last,
    /// Tab and Shift+Tab: the next or previous part (search → sidebar → pane; approximate).
    FocusNext,
    FocusPrevious,
    /// Escape: back to the sidebar from the pane.
    FocusSidebar,
    /// A key of the search field: an editing key or a result key.
    Search(SearchKey),
    None,
}

/// A key the search field takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchKey {
    /// Characters and the editing keys: the text field takes them (`cupertino-text-field`).
    Edit,
    /// Up and Down move the highlighted result.
    Up,
    Down,
    /// Return chooses the highlighted result.
    Choose,
    /// Escape clears the field.
    Clear,
}

/// The modifiers the router reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Mods {
    pub logo: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// The physical modifiers held, as the window's chords read them: `logo` is the command key of the user's profile, `ctrl`
/// the key that has the Ctrl role (Super under `classic-pc`).
pub fn command_mods(
    keymap: &cupertino_text_field::Keymap,
    shift: bool,
    ctrl: bool,
    alt: bool,
    logo: bool,
) -> Mods {
    use cupertino_modifiers::Mods as Role;
    let m = keymap.logical(shift, ctrl, alt, logo);
    Mods {
        logo: m.contains(Role::PRIMARY),
        shift: m.contains(Role::SHIFT),
        ctrl: m.contains(Role::CTRL),
        alt: m.contains(Role::ALT),
    }
}

/// Evdev codes of the keys the router reads (as `cupertino_wayland_client::key`).
pub mod code {
    pub const ESCAPE: u32 = 1;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const ENTER: u32 = 28;
    pub const KEYPAD_ENTER: u32 = 96;
    pub const HOME: u32 = 102;
    pub const UP: u32 = 103;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
}

/// The action of a key press with `focus`, the evdev `code`, the modifiers and the character it types.
pub fn route(focus: Focus, code: u32, mods: Mods, character: Option<char>) -> Action {
    if mods.logo && !mods.ctrl && !mods.alt {
        return match character.map(|c| c.to_ascii_lowercase()) {
            Some('[') => Action::Back,
            Some(']') => Action::Forward,
            Some('f') => Action::FocusSearch,
            Some('l') => Action::ShowAll,
            _ if focus == Focus::Search => Action::Search(SearchKey::Edit),
            _ => Action::None,
        };
    }
    if code == code::TAB {
        return if mods.shift {
            Action::FocusPrevious
        } else {
            Action::FocusNext
        };
    }
    match focus {
        Focus::Search => match code {
            code::ESCAPE => Action::Search(SearchKey::Clear),
            code::UP => Action::Search(SearchKey::Up),
            code::DOWN => Action::Search(SearchKey::Down),
            code::ENTER | code::KEYPAD_ENTER => Action::Search(SearchKey::Choose),
            _ => Action::Search(SearchKey::Edit),
        },
        Focus::Sidebar => match code {
            code::UP => Action::Previous,
            code::DOWN => Action::Next,
            code::HOME => Action::First,
            code::END => Action::Last,
            _ => Action::None,
        },
        Focus::Pane => match code {
            code::ESCAPE => Action::FocusSidebar,
            _ => Action::None,
        },
    }
}

/// The focus after Tab (`forward`) or Shift+Tab from `focus`.
pub fn cycle(focus: Focus, forward: bool) -> Focus {
    let order = [Focus::Search, Focus::Sidebar, Focus::Pane];
    let i = order
        .iter()
        .position(|f| *f == focus)
        .expect("every focus is in the order");
    order[if forward { (i + 1) % 3 } else { (i + 2) % 3 }]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPER: Mods = Mods {
        logo: true,
        shift: false,
        ctrl: false,
        alt: false,
    };

    #[test]
    fn the_navigation_chords_work_from_every_part() {
        for focus in [Focus::Search, Focus::Sidebar, Focus::Pane] {
            assert_eq!(route(focus, 26, SUPER, Some('[')), Action::Back);
            assert_eq!(route(focus, 27, SUPER, Some(']')), Action::Forward);
            assert_eq!(route(focus, 33, SUPER, Some('f')), Action::FocusSearch);
            assert_eq!(route(focus, 38, SUPER, Some('l')), Action::ShowAll);
        }
        assert_eq!(
            route(Focus::Sidebar, 33, Mods::default(), Some('f')),
            Action::None,
            "F alone is no chord"
        );
    }

    #[test]
    fn typing_searches_only_while_the_field_has_the_focus() {
        assert_eq!(
            route(Focus::Search, 30, Mods::default(), Some('a')),
            Action::Search(SearchKey::Edit)
        );
        assert_eq!(
            route(Focus::Search, code::ESCAPE, Mods::default(), None),
            Action::Search(SearchKey::Clear)
        );
        assert_eq!(
            route(Focus::Search, code::DOWN, Mods::default(), None),
            Action::Search(SearchKey::Down)
        );
        assert_eq!(
            route(Focus::Search, code::ENTER, Mods::default(), Some('\r')),
            Action::Search(SearchKey::Choose)
        );
        assert_eq!(
            route(Focus::Pane, 30, Mods::default(), Some('a')),
            Action::None,
            "typing in a pane does not search"
        );
        assert_eq!(
            route(Focus::Sidebar, code::DOWN, Mods::default(), None),
            Action::Next
        );
        assert_eq!(
            route(Focus::Sidebar, code::HOME, Mods::default(), None),
            Action::First
        );
        assert_eq!(
            route(Focus::Pane, code::ESCAPE, Mods::default(), None),
            Action::FocusSidebar
        );
    }

    #[test]
    fn editing_chords_reach_the_field_only_while_it_has_the_focus() {
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let edit = Action::Search(SearchKey::Edit);
        const A: u32 = 30;
        const LEFT: u32 = 105;
        assert_eq!(route(Focus::Search, A, SUPER, Some('a')), edit);
        assert_eq!(route(Focus::Search, LEFT, SUPER, None), edit);
        assert_eq!(route(Focus::Search, A, ctrl, Some('a')), edit);
        assert_eq!(
            route(Focus::Search, code::BACKSPACE, Mods::default(), None),
            edit
        );
        assert_eq!(
            route(Focus::Search, 26, SUPER, Some('[')),
            Action::Back,
            "window chords first"
        );
        assert_eq!(route(Focus::Sidebar, A, SUPER, Some('a')), Action::None);
        assert_eq!(route(Focus::Pane, A, ctrl, Some('a')), Action::None);
    }

    #[test]
    fn tab_cycles_search_sidebar_and_pane() {
        assert_eq!(
            route(Focus::Sidebar, code::TAB, Mods::default(), Some('\t')),
            Action::FocusNext
        );
        assert_eq!(
            route(
                Focus::Sidebar,
                code::TAB,
                Mods {
                    shift: true,
                    ..Mods::default()
                },
                None
            ),
            Action::FocusPrevious
        );
        assert_eq!(cycle(Focus::Search, true), Focus::Sidebar);
        assert_eq!(cycle(Focus::Pane, true), Focus::Search);
        assert_eq!(cycle(Focus::Search, false), Focus::Pane);
    }

    #[test]
    fn the_command_key_is_logo_whatever_key_the_profile_makes_it() {
        use cupertino_modifiers::Profile;
        use cupertino_text_field::Keymap;
        let cupertino = Keymap::standard_for(Profile::cupertino());
        let classic = Keymap::standard_for(Profile::classic_pc());
        let only = |keymap: &Keymap, ctrl: bool, logo: bool| {
            command_mods(keymap, false, ctrl, false, logo)
        };
        assert_eq!(
            only(&cupertino, false, true),
            SUPER,
            "Super under cupertino"
        );
        assert_eq!(only(&classic, true, false), SUPER, "Ctrl under classic-pc");
        // the other key keeps the Ctrl role
        assert_eq!(only(&cupertino, true, false).ctrl, true);
        assert_eq!(only(&classic, false, true).ctrl, true);
    }
}
