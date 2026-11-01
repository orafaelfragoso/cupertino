//! The sidebar's keyboard navigation. Ctrl+Tab and Ctrl+Shift+Tab, the key-view chords of the toolkit, move the
//! keyboard between the folder's items and the sidebar; Tab stays with the items, whose next item in name order it
//! selects. In the sidebar, Up and Down move the focus a row and stop at the ends, Home and End go to the first and the
//! last row, Return opens the focused row as a click does, and Primary+E ejects it when it is a mounted removable file
//! system. Escape and a press anywhere give the keyboard back to the items.

use std::path::Path;

use crate::places::{Place, PlaceKind, Volume};

/// A key the sidebar takes while it has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarKey {
    Up,
    Down,
    Home,
    End,
    /// Return or Enter.
    Open,
    /// Primary+E.
    Eject,
}

/// What a key in the sidebar asks the window to do, for the row at the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarAction {
    /// The focus moves to the row.
    Focus(usize),
    /// Show the row's folder.
    Go(usize),
    /// Mount the row's file system.
    Mount(usize),
    /// Eject the row's drive.
    Eject(usize),
}

/// The row that takes the focus when the sidebar gets the keyboard: the one of the folder shown, else the first.
pub fn entry_row(places: &[Place], dir: &Path) -> usize {
    places.iter().position(|p| p.path == dir).unwrap_or(0)
}

/// What `key` does with the focus on row `focus` of `places`; `None` when it does nothing there.
pub fn sidebar_key(places: &[Place], focus: usize, key: SidebarKey) -> Option<SidebarAction> {
    let last = places.len().checked_sub(1)?;
    let focus = focus.min(last);
    let moved = |to: usize| (to != focus).then_some(SidebarAction::Focus(to));
    match key {
        SidebarKey::Up => moved(focus.saturating_sub(1)),
        SidebarKey::Down => moved((focus + 1).min(last)),
        SidebarKey::Home => moved(0),
        SidebarKey::End => moved(last),
        SidebarKey::Open => match places[focus].kind {
            PlaceKind::Removable(Volume::Unmounted) => Some(SidebarAction::Mount(focus)),
            PlaceKind::Removable(Volume::Busy) => None,
            _ => Some(SidebarAction::Go(focus)),
        },
        SidebarKey::Eject => (places[focus].kind == PlaceKind::Removable(Volume::Mounted))
            .then_some(SidebarAction::Eject(focus)),
    }
}
