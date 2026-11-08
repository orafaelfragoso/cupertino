//! Spring-loaded folders in a window ([`crate::spring`]): a drag of files held over a folder of the list or the grid,
//! a place of the sidebar or a folder of the path bar opens that folder in the window after the delay, the folder
//! flashing just before. The window remembers its folder, history, expanded folders, search, selection, view and
//! scroll position from before the first folder opened; the drag leaving the window or being cancelled takes it back there, and a drop keeps the
//! folder the drag opened.

use std::path::PathBuf;

use cupertino_kit::layout::ScrollState;
use cupertino_kit::prelude::Command;

use super::{BrowserWindow, Event, Message, Session, ViewMode};
use crate::browser::Visit;
use crate::notice;
use crate::spring::{SpringLoad, Sprung, Step, Timer};

/// The spring-loading of one window.
#[derive(Debug, Default)]
pub(super) struct Springing {
    dwell: SpringLoad<PathBuf>,
    /// What the window showed when a drag first opened a folder in it.
    origin: Option<Origin>,
    /// The scroll position to give back once the listing of the folder gone back to is complete.
    scroll_back: Option<ScrollState>,
    /// Counts the folders a drag opened: the folder's drop target is keyed by it, so the drag is asked anew where it
    /// rests over the folder that opened.
    opened: u64,
}

#[derive(Debug)]
struct Origin {
    visit: Visit,
    view: ViewMode,
    scroll: ScrollState,
    /// The search field's text.
    search: String,
}

impl Springing {
    /// The folder flashing just before it opens.
    pub fn flashing(&self) -> Option<&PathBuf> {
        self.dwell.flashing()
    }

    /// Counts the folders a drag opened in the window.
    pub fn opened(&self) -> u64 {
        self.opened
    }
}

fn after(id: u64, timer: Timer) -> Command<Message> {
    Command::After {
        delay: timer.after,
        message: Message::Window(id, Event::SpringDue(timer.generation, timer.step)),
    }
}

impl BrowserWindow {
    /// A drag is over `folder`, or over no folder, in window `id`. The folder shown opens nothing.
    pub(super) fn spring_over(
        &mut self,
        id: u64,
        folder: Option<PathBuf>,
    ) -> Vec<Command<Message>> {
        let folder = folder.filter(|f| f != self.browser.dir());
        self.springing
            .dwell
            .hover(folder)
            .map(|timer| after(id, timer))
            .into_iter()
            .collect()
    }

    /// The timer of a dwell in window `id` is due: the folder flashes, or opens.
    pub(super) fn spring_due(
        &mut self,
        id: u64,
        generation: u64,
        step: Step,
        session: &Session,
    ) -> Vec<Command<Message>> {
        match self.springing.dwell.due(generation, step) {
            Some(Sprung::Flash(_, timer)) => vec![after(id, timer)],
            Some(Sprung::Open(folder)) => {
                let origin = Origin {
                    visit: self.browser.visit(),
                    view: self.view,
                    scroll: self.scroll,
                    search: self.search.clone(),
                };
                let before = self.browser.dir().to_path_buf();
                self.go(folder, session);
                if self.browser.dir() != before {
                    self.springing.origin.get_or_insert(origin);
                    self.springing.opened += 1;
                }
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// The drag left the window or was cancelled: back to what the window showed before a drag opened a folder.
    pub(super) fn spring_back(&mut self, session: &Session) {
        self.springing.dwell.cancel();
        let Some(origin) = self.springing.origin.take() else {
            return;
        };
        let dir = origin.visit.dir().to_path_buf();
        if let Err(e) = self.browser.return_to(origin.visit) {
            let name = crate::places::window_title(&dir, &session.places);
            session.notices.post(notice::unopened(&name, &e));
            return;
        }
        self.entered();
        if !origin.search.is_empty() {
            // the search comes back with its text in the field
            self.browser.set_filter(&origin.search);
            self.search = origin.search;
            self.search_revision += 1;
            self.follow_search();
        }
        self.view = origin.view;
        self.springing.scroll_back = Some(origin.scroll);
        self.spring_settle();
    }

    /// The drag dropped on the window: it stays in the folder the drag opened.
    pub(super) fn spring_kept(&mut self) {
        self.springing.dwell.cancel();
        self.springing.origin = None;
    }

    /// Once the listing of the folder gone back to is complete, its scroll position comes back.
    pub(super) fn spring_settle(&mut self) {
        if self.browser.is_loading() {
            return;
        }
        if let Some(scroll) = self.springing.scroll_back.take() {
            self.scroll.offset = scroll.offset;
        }
    }
}
