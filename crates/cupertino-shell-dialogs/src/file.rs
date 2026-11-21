//! One file chooser: its [`Chooser`] and what its window keeps (the rows' selection, scroll and presses, the band of
//! a chooser of several files, the sidebar's scroll and width, the Save As field's revision, the question whether to
//! replace a file), and what each event of its panel and its list does to them.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use cupertino_kit::controls::{Selection, SplitLayout};
use cupertino_kit::core::{Command, Key, WindowKey};
use cupertino_kit::filepicker::{Listing, Request};
use cupertino_kit::files::entry::Entry;
use cupertino_kit::files::list::Presses;
use cupertino_kit::files::panel::{ChoiceValue, PanelEvent, SIDEBAR_WIDTH};
use cupertino_kit::files::places::Place;
use cupertino_kit::files::ViewEvent;
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::ScrollState;

use crate::ask::{FileAnswer, FileAsk, Kind};
use crate::chooser::{Accept, Chooser};
use crate::component::Message;

const NO_SCROLL: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

/// What happens in a file chooser's window.
#[derive(Clone, Debug)]
pub enum FileEvent {
    Panel(PanelEvent),
    List(ViewEvent),
    /// A folder the chooser asked for was read.
    Listing(Listing),
    /// Return: the default button, or opening the selected folder.
    Return,
    /// Escape, or the server closed the window.
    Cancel,
}

/// One open file chooser.
pub struct FileDialog {
    pub id: u64,
    handle: String,
    reply: async_channel::Sender<FileAnswer>,
    chooser: Chooser,
    /// The rows shown, as the list draws them.
    items: Rc<[Entry]>,
    selection: Selection,
    scroll: ScrollState,
    presses: Presses,
    band: Option<Rect>,
    places_scroll: ScrollState,
    split: SplitLayout,
    name_revision: u64,
    /// The file a save would replace, while the person is asked.
    replace: Option<PathBuf>,
    /// The answer, once the dialog is done.
    answer: Option<FileAnswer>,
}

impl FileDialog {
    /// The dialog for `ask`, from the portal's request `handle`, which answers on `reply`; its first folder is
    /// its first [`FileDialog::take_request`].
    pub fn new(
        id: u64,
        handle: String,
        ask: FileAsk,
        reply: async_channel::Sender<FileAnswer>,
        home: &Path,
    ) -> FileDialog {
        FileDialog {
            id,
            handle,
            reply,
            chooser: Chooser::new(ask, home),
            items: Rc::from([]),
            selection: Selection::default(),
            scroll: NO_SCROLL,
            presses: Presses::default(),
            band: None,
            places_scroll: NO_SCROLL,
            split: SplitLayout {
                position: SIDEBAR_WIDTH,
                collapsed: None,
            },
            name_revision: 0,
            replace: None,
            answer: None,
        }
    }

    pub fn handle(&self) -> &str {
        &self.handle
    }

    pub fn window_key(&self) -> WindowKey {
        WindowKey(Key::from(format!("chooser-{}", self.id)))
    }

    pub fn chooser(&self) -> &Chooser {
        &self.chooser
    }

    pub fn items(&self) -> &Rc<[Entry]> {
        &self.items
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn scroll(&self) -> ScrollState {
        self.scroll
    }

    pub fn presses(&self) -> &Presses {
        &self.presses
    }

    pub fn band(&self) -> Option<Rect> {
        self.band
    }

    pub fn places_scroll(&self) -> ScrollState {
        self.places_scroll
    }

    pub fn split(&self) -> SplitLayout {
        self.split
    }

    pub fn name_revision(&self) -> u64 {
        self.name_revision
    }

    pub fn replacing(&self) -> Option<&Path> {
        self.replace.as_deref()
    }

    /// Take the listing the dialog's read made, as its task hands it back (the capture example reads it in place).
    pub fn apply(&mut self, listing: Listing) {
        self.chooser.apply(listing);
        self.refresh();
    }

    /// The rows and their selection after the chooser changed: the selected paths where they are shown now.
    fn refresh(&mut self) {
        self.items = self.chooser.entries().into_iter().cloned().collect();
        let rows: Vec<usize> = (self.items.iter().enumerate())
            .filter(|(_, e)| self.chooser.selection().contains(&e.path))
            .map(|(i, _)| i)
            .collect();
        let cursor = rows.last().copied();
        self.selection = Selection::new(rows.into_iter().map(|i| i..i + 1), cursor, cursor);
    }

    /// The folder to read next, after the dialog opened or an event moved it; the listing comes back as
    /// [`FileEvent::Listing`].
    pub fn take_request(&mut self) -> Option<Request> {
        self.chooser.take_request()
    }

    /// What the accept button, Return or a row opened did: the dialog is done, or asks whether to replace a file.
    fn accepted(&mut self, accept: Accept) -> (bool, Vec<Command<Message>>) {
        match accept {
            Accept::Answer(answer) => {
                self.answer = Some(answer);
                (true, Vec::new())
            }
            Accept::Replace(path) => {
                self.replace = Some(path);
                (false, vec![Command::Focus(Key::from("replace"))])
            }
            Accept::Nothing => (false, Vec::new()),
        }
    }

    /// Carry out `event`; true when the dialog is done. A folder it moves to is the next [`FileDialog::take_request`].
    pub fn update(&mut self, event: FileEvent, places: &[Place]) -> (bool, Vec<Command<Message>>) {
        match event {
            FileEvent::Panel(event) => self.panel(event, places),
            FileEvent::List(event) => self.list(event),
            FileEvent::Listing(listing) => {
                self.apply(listing);
                (false, Vec::new())
            }
            FileEvent::Return => {
                // a selected folder opens, as its row's Return does; else the default button
                let folder = (self.selection.cursor())
                    .filter(|&row| self.items.get(row).is_some_and(|e| e.is_dir));
                match folder {
                    // the question whether to replace a file holds the default button while it is asked
                    _ if self.replace.is_some() => self.panel(PanelEvent::Replace, places),
                    Some(row) if !self.chooser.ask().directory => self.list(ViewEvent::Open(row)),
                    _ => {
                        let accept = self.chooser.accept();
                        self.accepted(accept)
                    }
                }
            }
            FileEvent::Cancel => {
                self.answer = Some(FileAnswer::Cancelled);
                (true, Vec::new())
            }
        }
    }

    fn panel(&mut self, event: PanelEvent, places: &[Place]) -> (bool, Vec<Command<Message>>) {
        match event {
            PanelEvent::Place(index) => {
                if let Some(place) = places.get(index) {
                    self.chooser.go(&place.path);
                }
            }
            PanelEvent::PlacesScroll(scroll) => self.places_scroll = scroll,
            PanelEvent::Parent => self.chooser.parent(),
            PanelEvent::Split(split) => self.split = split,
            PanelEvent::Name(change) => {
                self.chooser.set_name(change.text);
                self.name_revision = change.revision;
                self.replace = None;
            }
            PanelEvent::Filter(index) => {
                self.chooser.set_filter(index);
                self.refresh();
            }
            PanelEvent::Choice(index, value) => {
                let value = match value {
                    ChoiceValue::Checked(on) => Some(on.to_string()),
                    ChoiceValue::Option(option) => (self.chooser.ask().choices.get(index))
                        .and_then(|c| c.options.get(option))
                        .map(|(id, _)| id.clone()),
                };
                if let Some(value) = value {
                    self.chooser.set_choice(index, &value);
                }
            }
            PanelEvent::Accept => {
                let accept = self.chooser.accept();
                return self.accepted(accept);
            }
            PanelEvent::Cancel => {
                self.answer = Some(FileAnswer::Cancelled);
                return (true, Vec::new());
            }
            PanelEvent::Replace => {
                if let Some(path) = self.replace.take() {
                    self.answer = Some(self.chooser.replace(path));
                    return (true, Vec::new());
                }
            }
            PanelEvent::KeepEditing => self.replace = None,
        }
        (false, Vec::new())
    }

    fn list(&mut self, event: ViewEvent) -> (bool, Vec<Command<Message>>) {
        match event {
            ViewEvent::Select(rows) => {
                self.chooser
                    .select(rows.ranges().iter().flat_map(Clone::clone));
                self.selection = rows;
                if self.chooser.ask().kind == Kind::Save {
                    // a file chosen in a save names the file to write
                    let chosen = (self.items.iter())
                        .find(|e| !e.is_dir && self.chooser.selection().contains(&e.path))
                        .map(|e| e.name.clone());
                    if let Some(name) = chosen {
                        self.chooser.set_name(name);
                        self.name_revision += 1;
                    }
                }
            }
            ViewEvent::Open(row) => {
                let accept = self.chooser.open(row);
                return self.accepted(accept);
            }
            ViewEvent::Scroll(scroll) => self.scroll = scroll,
            ViewEvent::Band(band) => self.band = band,
            _ => {}
        }
        (false, Vec::new())
    }

    /// Send the dialog's answer to its call; a dialog dropped without one ends its call.
    pub fn finish(self) {
        if let Some(answer) = self.answer {
            let _ = self.reply.try_send(answer);
        }
    }
}
