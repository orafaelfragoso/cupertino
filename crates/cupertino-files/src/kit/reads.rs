//! What a window reads off the UI thread, as I/O tasks the program starts and answers that come back as messages: the
//! free space of its folder (the window asks with [`BrowserWindow::read_available`]; the answer is an [`Event::Space`]),
//! the nearest existing folder above one that is gone, and the telling of a folder that could not be read at all.

use std::path::Path;
use std::time::Duration;

use cupertino_kit::prelude::Command;

use super::{Action, BrowserWindow, Event, Message, Session};
use crate::browser::{AncestorLookup, LOOKUP_DEADLINE};
use crate::space;

/// How long reading a folder's free space should take: one `statvfs` of a mounted file system, which only a mount
/// that does not answer holds up.
const SPACE_DEADLINE: Duration = Duration::from_secs(5);

/// The task that looks for the nearest existing folder above a folder that is gone; the browser hears it through its
/// own wake.
pub(super) fn look_up(lookup: AncestorLookup) -> Command<Message> {
    cupertino_kit::task::io(LOOKUP_DEADLINE, move || lookup.run(), |()| Message::Worked)
}

impl BrowserWindow {
    /// The window's first folder could not be read (`error`): the notice, and whether the window has nothing to show
    /// and closes. A tab among others stays on the folder it had.
    pub(super) fn not_opened(&self, error: Option<String>, session: &Session) -> bool {
        let Some(error) = error else {
            return false;
        };
        session.failed(Action::Open, &[self.browser.dir()], &error);
        self.tabs.len() == 1
    }

    /// The task that reads the free space of the folder shown, when it is due.
    pub(super) fn space_task(&mut self, id: u64) -> Option<Command<Message>> {
        if !std::mem::take(&mut self.space_due) {
            return None;
        }
        let dir = self.browser.dir().to_path_buf();
        let read = dir.clone();
        Some(cupertino_kit::task::io(
            SPACE_DEADLINE,
            move || space::read(&read),
            move |read| Message::Window(id, Event::Space(dir, read)),
        ))
    }

    /// The file system of `dir` was read: the status bar of the tab that shows it takes the answer. A folder whose file
    /// system cannot be read takes the changes, and their failures say why.
    pub(super) fn space_read(&mut self, dir: &Path, read: Result<space::Space, String>) {
        let (available, read_only) = match read {
            Ok(space) => (space.available, space.read_only),
            Err(e) => {
                eprintln!(
                    "cupertino-files: cannot read the free space of {}: {e}",
                    dir.display()
                );
                (None, false)
            }
        };
        if self.browser.dir() == dir {
            self.available = available;
            self.mounted_read_only = read_only;
        } else if let Some(tab) = self.parked.values_mut().find(|t| t.browser.dir() == dir) {
            tab.available = available;
            tab.mounted_read_only = read_only;
        }
    }
}
