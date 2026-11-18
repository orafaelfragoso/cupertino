//! CupertinoKit's file family, which programs name as `cupertino_kit::files`.
//!
//! * [`entry`]: an item of a folder; [`natural`], [`sort`]: the order of names and of items;
//! * [`mime`], [`tags`]: an item's type and its tags; [`applications`], [`recents`]: the two virtual folders;
//! * [`places`]: the places of a file browser's sidebar and of a file chooser's, over the user directories;
//! * [`layout`]: the icon layout, with the measured metrics of [`metrics`]; [`geom`]: the direction a key moves in it;
//! * [`rename`], [`name_edit`]: name checks, free names and the name being edited; [`open`]: default applications and
//!   the Open With list; [`operations`]: new folders, renames, aliases, copies and moves, as blocking functions;
//! * [`item_art`]: the art of the common items;
//! * [`grid`]: the folder as a grid of icons ([`grid::IconGrid`]), [`list`]: as an outline table
//!   ([`list::FileList`]), and [`columns`]: a chain of folders side by side ([`columns::FileColumns`]), which report
//!   through [`ViewEvent`].

use std::path::PathBuf;

use cupertino_geometry::{Point, Rect};
use cupertino_ui_controls::{Selection, SortOrder, TreeToggle};
use cupertino_ui_core::layout::ScrollState;

pub mod applications;
mod band;
mod clicks;
pub mod columns;
pub mod entry;
pub mod geom;
pub mod grid;
pub mod item_art;
pub mod layout;
pub mod list;
pub mod metrics;
pub mod mime;
pub mod name_edit;
pub mod natural;
pub mod open;
pub mod operations;
pub mod places;
pub mod recents;
pub mod rename;
pub mod sort;
pub mod tags;

/// What a file view reports. The program gives each view one function that turns an event into its message, or into
/// none.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewEvent {
    /// The selection a press, a key, the band or typing made.
    Select(Selection),
    /// Open item `index`: a double click or Return.
    Open(usize),
    /// A secondary press on item `index`, or on empty space.
    SecondaryPress(Option<usize>),
    /// Open the context menu at this point of the window.
    ContextMenu(Point),
    /// A press on items of the grid went far enough to drag them.
    DragOut(grid::Grab),
    /// A press on rows of the list, or of the shown folder's column, went far enough to drag them.
    DragRows(list::RowGrab),
    /// The rubber band being drawn, in the view's content; `None` once it is let go.
    Band(Option<Rect>),
    /// The view scrolled: its scroll view, or a band held past its edge.
    Scroll(ScrollState),
    /// A press on the name of the only selected item had no second press: edit the name of item `index`.
    Rename(usize),
    /// Tab chose row `index` of the list.
    FocusRow(usize),
    /// A column header of the list was chosen.
    Sort((usize, SortOrder)),
    /// A folder's row of the list opens or closes.
    Toggle(TreeToggle),
    /// The column view's own events.
    Column(ColumnEvent),
}

/// What the column view reports besides what every view does.
#[derive(Clone, Debug, PartialEq)]
pub enum ColumnEvent {
    /// The column of this folder scrolled.
    Scroll(PathBuf, ScrollState),
    /// Row `index` of the column of `folder`, a folder other than the one shown, was chosen; `secondary` when a
    /// secondary press chose it, whose context menu follows.
    Pick {
        folder: PathBuf,
        index: usize,
        secondary: bool,
    },
    /// Right (1) or Left (-1) in the column of the folder shown.
    Step(isize),
    /// A divider was dragged to make column `index` `width` wide; `all` with Option held.
    Resize { index: usize, width: f64, all: bool },
    /// The divider was let go.
    Resized,
}
