//! The open panel of the panes (Kit's `files::panel`): the Wallpaper pane's Add Folder and Add Photo, Clipboard
//! Access's program and the folders the launcher leaves out. The folder it shows, the row the person selected, where
//! its list and its sidebar are scrolled, and what each event of its panel and its list does. It reads no folder itself:
//! its [`Chooser::take_request`] is read by a task and handed back to [`Chooser::apply`].

use std::path::{Path, PathBuf};

use cupertino_kit::controls::{Selection, SplitLayout};
use cupertino_kit::filepicker::{Listing, Mode, Outcome as PickerOutcome, Picker, Request};
use cupertino_kit::files::entry::Entry;
use cupertino_kit::files::panel::{PanelEvent, SIDEBAR_WIDTH};
use cupertino_kit::files::places::Place;
use cupertino_kit::files::ViewEvent;
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::layout::ScrollState;

const UNSCROLLED: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

/// What happens in the panel's window.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Panel(PanelEvent),
    List(ViewEvent),
    /// Return: opening the selected folder, or the default button.
    Return,
    /// Escape, or the server closed the window.
    Cancel,
}

/// What an event decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The panel stays up, changed or not; a folder it moved to is its next [`Chooser::take_request`].
    Stay,
    Cancel,
    /// The folder, picture or program chosen; the panel closes.
    Chosen(PathBuf),
}

/// One open panel.
#[derive(Clone, Debug)]
pub struct Chooser {
    picker: Picker,
    places: Vec<Place>,
    selection: Selection,
    scroll: ScrollState,
    places_scroll: ScrollState,
    split: SplitLayout,
    /// Counts the listings taken, so the window knows when its rows changed.
    listings: u64,
}

impl Chooser {
    /// A panel that chooses in `mode` (a folder, a picture or a program), opened at `start` or its nearest folder that
    /// exists, with `places` in its sidebar. It lists nothing until its first [`Chooser::take_request`] is answered.
    pub fn new(start: &Path, mode: Mode, places: Vec<Place>) -> Self {
        Chooser {
            picker: Picker::new(start, mode),
            places,
            selection: Selection::default(),
            scroll: UNSCROLLED,
            places_scroll: UNSCROLLED,
            split: SplitLayout {
                position: SIDEBAR_WIDTH,
                collapsed: None,
            },
            listings: 0,
        }
    }

    pub fn mode(&self) -> Mode {
        self.picker.mode()
    }

    /// The open folder.
    pub fn dir(&self) -> &Path {
        self.picker.dir()
    }

    /// The last folder asked for could not be read; the open one stayed.
    pub fn unreadable(&self) -> bool {
        self.picker.unreadable()
    }

    /// The rows: the open folder's items that the mode lists.
    pub fn entries(&self) -> &[Entry] {
        self.picker.entries()
    }

    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// The place of the sidebar that is the open folder.
    pub fn place(&self) -> Option<usize> {
        self.places.iter().position(|p| p.path == self.picker.dir())
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn scroll(&self) -> ScrollState {
        self.scroll
    }

    pub fn places_scroll(&self) -> ScrollState {
        self.places_scroll
    }

    pub fn split(&self) -> SplitLayout {
        self.split
    }

    /// How many listings the panel took: its rows change with each.
    pub fn listings(&self) -> u64 {
        self.listings
    }

    /// The window's title.
    pub fn title(&self) -> &'static str {
        match self.picker.mode() {
            Mode::Folder => "Add a Folder",
            Mode::Photo => "Add a Photo",
            Mode::Program => "Choose a Program",
            Mode::File => "Choose a File",
        }
    }

    /// The default button's label.
    pub fn accept_label(&self) -> &'static str {
        match self.picker.mode() {
            Mode::Folder | Mode::File => "Choose",
            Mode::Photo => "Add",
            Mode::Program => "Allow",
        }
    }

    /// What the default button chooses now: a folder chooser the selected folder, else the open one; the others the
    /// selected file, none without one.
    pub fn choice(&self) -> Option<PathBuf> {
        let selected = self.selected().and_then(|row| self.entries().get(row));
        match (self.picker.mode(), selected) {
            (Mode::Folder, Some(entry)) => Some(entry.path.clone()),
            (Mode::Folder, None) => Some(self.picker.dir().to_path_buf()),
            (Mode::Photo | Mode::Program | Mode::File, Some(entry)) if !entry.is_dir => {
                Some(entry.path.clone())
            }
            (Mode::Photo | Mode::Program | Mode::File, _) => None,
        }
    }

    /// The folder to list next, once, for a task to read.
    pub fn take_request(&mut self) -> Option<Request> {
        self.picker.take_request()
    }

    /// Take a task's listing; one for a folder no longer asked for is dropped. In another folder nothing is selected
    /// but the folder that was left, when it is the parent that opened, and the list starts at its top.
    pub fn apply(&mut self, listing: Listing) {
        let before = self.picker.dir().to_path_buf();
        if self.picker.apply(listing) != PickerOutcome::Changed {
            return;
        }
        self.listings += 1;
        if self.picker.dir() != before {
            self.scroll = UNSCROLLED;
        }
        let row = self.picker.highlighted();
        self.selection = Selection::new(row.map(|i| i..i + 1), row, row);
    }

    /// Carry out `event`.
    pub fn update(&mut self, event: Event) -> Outcome {
        match event {
            Event::Panel(event) => self.panel(event),
            Event::List(event) => self.list(event),
            Event::Return => {
                let folder = (self.selected())
                    .filter(|&row| self.entries().get(row).is_some_and(|e| e.is_dir));
                match folder {
                    // a folder chooser chooses the selected folder; the others open it
                    Some(row) if self.picker.mode() != Mode::Folder => self.open(row),
                    _ => self.accept(),
                }
            }
            Event::Cancel => Outcome::Cancel,
        }
    }

    /// The selected row.
    fn selected(&self) -> Option<usize> {
        (self.selection.cursor()).filter(|&row| self.selection.contains(row))
    }

    fn accept(&self) -> Outcome {
        self.choice().map_or(Outcome::Stay, Outcome::Chosen)
    }

    /// A double click or Return on `row`: a folder opens, a file is chosen.
    fn open(&mut self, row: usize) -> Outcome {
        match self.picker.activate(row) {
            PickerOutcome::Choose(path) => Outcome::Chosen(path),
            PickerOutcome::Changed | PickerOutcome::Ignore | PickerOutcome::Cancel => Outcome::Stay,
        }
    }

    fn panel(&mut self, event: PanelEvent) -> Outcome {
        match event {
            PanelEvent::Place(index) => {
                if let Some(place) = self.places.get(index) {
                    let path = place.path.clone();
                    self.picker.go(&path);
                }
            }
            PanelEvent::PlacesScroll(scroll) => self.places_scroll = scroll,
            PanelEvent::Parent => {
                self.picker.parent();
            }
            PanelEvent::Split(split) => self.split = split,
            PanelEvent::Accept => return self.accept(),
            PanelEvent::Cancel => return Outcome::Cancel,
            // an open panel has no name to save as, no filters, no choices and no file to replace
            PanelEvent::Name(_)
            | PanelEvent::Filter(_)
            | PanelEvent::Choice(..)
            | PanelEvent::Replace
            | PanelEvent::KeepEditing => {}
        }
        Outcome::Stay
    }

    fn list(&mut self, event: ViewEvent) -> Outcome {
        match event {
            ViewEvent::Select(rows) => self.selection = rows,
            ViewEvent::Open(row) => return self.open(row),
            ViewEvent::Scroll(scroll) => self.scroll = scroll,
            // one row is chosen at a time; nothing is renamed, dragged, sorted or opened in place
            _ => {}
        }
        Outcome::Stay
    }
}
