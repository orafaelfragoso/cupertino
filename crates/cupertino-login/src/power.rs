//! The login screen's power item: a menu of Restart, Shut Down and Sleep behind a power glyph. Which of the three
//! show is decided by the session manager's configuration. Pure: no drawing, no logind.

/// What a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Restart,
    ShutDown,
    Sleep,
}

impl Action {
    /// The item's title (`Restart` / `Shut Down` / `Sleep`). The first two get an ellipsis when `will_prompt`:
    /// the action will ask for an administrator because a user session is open.
    pub fn title(self, will_prompt: bool) -> &'static str {
        match (self, will_prompt) {
            (Action::Restart, false) => "Restart",
            (Action::Restart, true) => "Restart…",
            (Action::ShutDown, false) => "Shut Down",
            (Action::ShutDown, true) => "Shut Down…",
            (Action::Sleep, _) => "Sleep",
        }
    }
}

impl Action {
    /// The name in the log line `power action: <name>`.
    pub fn log_name(self) -> &'static str {
        match self {
            Action::Restart => "restart",
            Action::ShutDown => "shut-down",
            Action::Sleep => "sleep",
        }
    }
}

/// A key the open menu reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    Return,
    Escape,
}

/// The power item: closed, or open with one menu row selected.
#[derive(Debug)]
pub struct Menu {
    items: Vec<Action>,
    /// A user session is open, so Restart and Shut Down ask for an administrator and show an ellipsis.
    will_prompt: bool,
    /// The selected row while the menu is open.
    open: Option<usize>,
}

impl Menu {
    /// The menu of `items`; `None` when there are none, so there is no item.
    pub fn new(items: Vec<Action>) -> Option<Menu> {
        (!items.is_empty()).then_some(Menu {
            items,
            will_prompt: false,
            open: None,
        })
    }

    pub fn items(&self) -> &[Action] {
        &self.items
    }

    /// Whether Restart and Shut Down will ask for an administrator (shown with an ellipsis).
    pub fn will_prompt(&self) -> bool {
        self.will_prompt
    }

    pub fn set_will_prompt(&mut self, will_prompt: bool) {
        self.will_prompt = will_prompt;
    }

    /// The selected row while the menu is open.
    pub fn open(&self) -> Option<usize> {
        self.open
    }

    /// Open the menu on its first row, or close it.
    pub fn toggle(&mut self) {
        self.open = match self.open {
            Some(_) => None,
            None => Some(0),
        };
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// Select row `row` of an open menu, for a pointer over it.
    pub fn select(&mut self, row: usize) {
        assert!(row < self.items.len(), "the row exists");
        if self.open.is_some() {
            self.open = Some(row);
        }
    }

    /// A key on an open menu: the arrows move the selection, Return does the selected item (and closes), Escape closes.
    /// A closed menu takes no keys.
    pub fn press(&mut self, key: MenuKey) -> Option<Action> {
        let selected = self.open?;
        match key {
            MenuKey::Up => self.open = Some(selected.saturating_sub(1)),
            MenuKey::Down => self.open = Some((selected + 1).min(self.items.len() - 1)),
            MenuKey::Return => return self.activate(selected),
            MenuKey::Escape => self.open = None,
        }
        None
    }

    /// Do item `row` and close the menu.
    pub fn activate(&mut self, row: usize) -> Option<Action> {
        self.open = None;
        self.items.get(row).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_nothing_to_show_there_is_no_menu() {
        assert!(Menu::new(Vec::new()).is_none());
    }

    #[test]
    fn the_menu_opens_on_the_first_row_moves_and_does_the_selected_item() {
        let mut menu = Menu::new(vec![Action::Restart, Action::ShutDown, Action::Sleep]).unwrap();
        assert_eq!(menu.open(), None);
        // a closed menu takes no keys
        assert_eq!(menu.press(MenuKey::Down), None);
        assert_eq!(menu.open(), None);
        menu.toggle();
        assert_eq!(menu.open(), Some(0));
        for _ in 0..3 {
            menu.press(MenuKey::Down);
        }
        assert_eq!(menu.open(), Some(2));
        menu.press(MenuKey::Up);
        assert_eq!(menu.press(MenuKey::Return), Some(Action::ShutDown));
        assert_eq!(menu.open(), None);
    }

    #[test]
    fn restart_and_shut_down_get_an_ellipsis_when_they_will_ask_for_an_administrator() {
        assert_eq!(
            [Action::Restart, Action::ShutDown, Action::Sleep].map(|action| action.title(false)),
            ["Restart", "Shut Down", "Sleep"]
        );
        assert_eq!(
            [Action::Restart, Action::ShutDown, Action::Sleep].map(|action| action.title(true)),
            ["Restart…", "Shut Down…", "Sleep"]
        );
        let mut menu = Menu::new(vec![Action::Restart]).unwrap();
        assert!(!menu.will_prompt());
        menu.set_will_prompt(true);
        assert!(menu.will_prompt());
    }

    #[test]
    fn escape_and_a_second_toggle_close_the_menu_without_an_action() {
        let mut menu = Menu::new(vec![Action::Sleep]).unwrap();
        menu.toggle();
        assert_eq!(menu.press(MenuKey::Escape), None);
        assert_eq!(menu.open(), None);
        menu.toggle();
        menu.toggle();
        assert_eq!(menu.open(), None);
    }
}
