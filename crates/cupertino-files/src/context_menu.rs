//! The rows of the context menus: what a row is and does. What each menu offers is [`crate::menu_spec`]; the Kit
//! window shows them with the toolkit's menus ([`crate::kit`]).
//!
//! * A row can carry alternates: rows that take the place of the row while the modifiers they name are held.
//! * Two rows stand for a set of choices: the tag row of seven colours and the row of the four view modes.

use crate::sort::SortKey;
use crate::tags::TagColor;
use crate::window::ViewMode;

/// The modifier keys that select an alternate row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Mods {
    pub const ALT: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: true,
    };
    pub const CTRL: Mods = Mods {
        shift: false,
        ctrl: true,
        alt: false,
    };
    pub const SHIFT_ALT: Mods = Mods {
        shift: true,
        ctrl: false,
        alt: true,
    };
}

/// What a picked row asks the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the selection with its default applications (folders: enter the first).
    Open,
    /// Open the selection with the application of this desktop entry id.
    OpenWith(String),
    /// Open each selected folder in a window of its own.
    OpenInNewWindow,
    /// Show the target of the selected link in its folder.
    ShowOriginal,
    /// Put the selected items of the trash back where they came from.
    PutBack,
    MoveToTrash,
    /// Edit the name of the selected item in place.
    Rename,
    /// Create an empty folder in the open one and edit its name.
    NewFolder,
    /// Create a folder next to the selection, move the selection into it and edit its name.
    NewFolderWithSelection,
    /// Copy the selected items next to themselves.
    Duplicate,
    /// Make a link to the selected item next to it.
    MakeAlias,
    /// Order the folder by this key, ascending or not.
    SortBy(SortKey, bool),
    /// Add the tag of this colour to the selection, or remove it when every item has it.
    ToggleTag(TagColor),
    SetView(ViewMode),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Command(Command),
    Submenu(Vec<MenuItem>),
    Separator,
    /// A row the window cannot do anything for yet: drawn disabled, never picked.
    Unavailable,
    /// The row of colour dots; `on` are the colours every selected item has.
    TagRow {
        on: Vec<TagColor>,
    },
    /// The row of the four view modes, the current one marked.
    ViewRow {
        current: ViewMode,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub kind: ItemKind,
    pub enabled: bool,
    /// The row shows a check mark: the current choice of a list of choices.
    pub checked: bool,
    /// Rows that replace this one while exactly these modifiers are held.
    pub alternates: Vec<(Mods, MenuItem)>,
}

impl MenuItem {
    pub fn command(label: &str, command: Command, enabled: bool) -> Self {
        MenuItem {
            label: label.to_string(),
            kind: ItemKind::Command(command),
            enabled,
            checked: false,
            alternates: Vec::new(),
        }
    }

    /// A row the window cannot act on yet: disabled.
    pub fn unavailable(label: &str) -> Self {
        MenuItem {
            label: label.to_string(),
            kind: ItemKind::Unavailable,
            enabled: false,
            checked: false,
            alternates: Vec::new(),
        }
    }

    pub fn submenu(label: &str, items: Vec<MenuItem>, enabled: bool) -> Self {
        MenuItem {
            label: label.to_string(),
            kind: ItemKind::Submenu(items),
            enabled,
            checked: false,
            alternates: Vec::new(),
        }
    }

    pub fn separator() -> Self {
        MenuItem {
            label: String::new(),
            kind: ItemKind::Separator,
            enabled: false,
            checked: false,
            alternates: Vec::new(),
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// This row, replaced by `alternate` while `mods` are held.
    pub fn with_alternate(mut self, mods: Mods, alternate: MenuItem) -> Self {
        self.alternates.push((mods, alternate));
        self
    }

    /// The row can be chosen: an enabled command or submenu.
    pub fn is_selectable(&self) -> bool {
        self.enabled && matches!(self.kind, ItemKind::Command(_) | ItemKind::Submenu(_))
    }
}

/// The colours of the tag row, in order: red, orange, yellow, green, blue, purple, gray.
pub const TAG_ROW_ORDER: [TagColor; 7] = [
    TagColor::Red,
    TagColor::Orange,
    TagColor::Yellow,
    TagColor::Green,
    TagColor::Blue,
    TagColor::Purple,
    TagColor::Gray,
];

/// The view modes in the order of the view row.
pub const VIEW_ROW_ORDER: [ViewSegment; 4] = [
    ViewSegment::Icons,
    ViewSegment::List,
    ViewSegment::Columns,
    ViewSegment::Gallery,
];

/// A choice of the view row; only the first two have a view behind them in this window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewSegment {
    Icons,
    List,
    Columns,
    Gallery,
}

impl ViewSegment {
    pub fn mode(self) -> Option<ViewMode> {
        match self {
            ViewSegment::Icons => Some(ViewMode::Icons),
            ViewSegment::List => Some(ViewMode::List),
            ViewSegment::Columns | ViewSegment::Gallery => None,
        }
    }
}

/// An application that can open the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppChoice {
    /// Desktop entry id.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// The applications every list offers, in the order of the first list. An empty slice of lists offers none.
pub fn common_apps(lists: &[Vec<AppChoice>]) -> Vec<AppChoice> {
    let Some((first, rest)) = lists.split_first() else {
        return Vec::new();
    };
    first
        .iter()
        .filter(|a| rest.iter().all(|l| l.iter().any(|b| b.id == a.id)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> AppChoice {
        AppChoice {
            id: id.into(),
            name: id.into(),
            is_default: false,
        }
    }

    #[test]
    fn a_selection_is_offered_the_applications_that_open_every_item() {
        let lists = [
            vec![app("viewer"), app("editor"), app("browser")],
            vec![app("browser"), app("editor")],
        ];
        let ids: Vec<String> = common_apps(&lists).into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["editor", "browser"]);
        assert!(common_apps(&[]).is_empty());
    }
}
