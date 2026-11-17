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

/// The power item: its items, and whether its menu is open. The menu itself is the toolkit's: it moves its highlight with
/// the arrows, does an item on Return or a click and closes on Escape or a press outside.
#[derive(Debug)]
pub struct Menu {
    items: Vec<Action>,
    /// A user session is open, so Restart and Shut Down ask for an administrator and show an ellipsis.
    will_prompt: bool,
    open: bool,
}

impl Menu {
    /// The menu of `items`; `None` when there are none, so there is no item.
    pub fn new(items: Vec<Action>) -> Option<Menu> {
        (!items.is_empty()).then_some(Menu {
            items,
            will_prompt: false,
            open: false,
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

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the menu, or close it.
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Do item `row` and close the menu.
    pub fn activate(&mut self, row: usize) -> Option<Action> {
        self.open = false;
        self.items.get(row).copied()
    }
}
