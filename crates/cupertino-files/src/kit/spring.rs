//! Spring-loaded folders in a window ([`crate::spring`]): a drag of files held over a folder of the list or the grid,
//! a place of the sidebar or a folder of the path bar opens that folder in the window after the delay, the folder
//! flashing just before. The window remembers its folder, history, expanded folders, search, selection, view and
//! scroll position from before the first folder opened; the drag leaving the window or being cancelled takes it back there, and a drop keeps the
//! folder the drag opened.

use std::path::PathBuf;

use cupertino_kit::layout::ScrollState;
use cupertino_kit::prelude::Command;

use super::{BrowserWindow, Event, Message, ViewMode};
use crate::browser::{Asked, Visit};
use crate::spring::{SpringLoad, Sprung, Step, Timer};

/// The spring-loading of one window.
#[derive(Debug, Default)]
pub(super) struct Springing {
    dwell: SpringLoad<PathBuf>,
    /// What the window showed when a drag first opened a folder in it.
    origin: Option<Origin>,
    /// The folder a dwell asked the window to open, until it opens, and what the window showed then.
    asked: Option<(PathBuf, Origin)>,
    /// What comes back with the way back the drag's end asked for, until that folder opens.
    returning: Option<Back>,
    /// The scroll position to give back once the listing of the folder gone back to is complete.
    scroll_back: Option<ScrollState>,
    /// Counts the folders a drag opened: the folder's drop target is keyed by it, so the drag is asked anew where it
    /// rests over the folder that opened.
    opened: u64,
}

#[derive(Debug)]
struct Origin {
    visit: Visit,
    back: Back,
}

/// What comes back with the way back besides the folder.
#[derive(Debug)]
struct Back {
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

    /// The timer of a dwell in window `id` is due: the folder flashes, or is asked for, to open once it does.
    pub(super) fn spring_due(
        &mut self,
        id: u64,
        generation: u64,
        step: Step,
    ) -> Vec<Command<Message>> {
        match self.springing.dwell.due(generation, step) {
            Some(Sprung::Flash(_, timer)) => vec![after(id, timer)],
            Some(Sprung::Open(folder)) => {
                let origin = Origin {
                    visit: self.browser.visit(),
                    back: Back {
                        view: self.view,
                        scroll: self.scroll,
                        search: self.search.clone(),
                    },
                };
                self.springing.asked = Some((folder.clone(), origin));
                self.go(folder);
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// The drag left the window or was cancelled: back to what the window showed before a drag opened a folder.
    pub(super) fn spring_back(&mut self) {
        self.springing.dwell.cancel();
        self.springing.asked = None;
        let Some(Origin { visit, back }) = self.springing.origin.take() else {
            return;
        };
        if self.browser.return_to(visit) {
            self.entered();
            self.returned(back);
        } else {
            self.springing.returning = Some(back);
        }
    }

    /// The window shows the folder of a request, which it entered: the folder a dwell opened, or the one the drag's
    /// end went back to.
    pub(super) fn spring_entered(&mut self, asked: Asked) {
        self.entered();
        if asked == Asked::Return {
            if let Some(back) = self.springing.returning.take() {
                self.returned(back);
            }
            return;
        }
        let opened = self.springing.asked.take();
        if let Some((_, origin)) = opened.filter(|(folder, _)| folder == self.browser.dir()) {
            self.springing.origin.get_or_insert(origin);
            self.springing.opened += 1;
        }
    }

    /// The folder of a request did not open: a dwell opened nothing, and the way back stays where it is.
    pub(super) fn spring_unopened(&mut self) {
        self.springing.asked = None;
        self.springing.returning = None;
    }

    /// The window is back where the drag found it: its search, its view and its scroll position come back.
    fn returned(&mut self, back: Back) {
        if !back.search.is_empty() {
            // the search comes back with its text in the field
            self.browser.set_filter(&back.search);
            self.search = back.search;
            self.search_revision += 1;
            self.follow_search();
        }
        self.view = back.view;
        self.springing.scroll_back = Some(back.scroll);
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
