//! What the calls of later launches and of the desktop do ([`crate::instance`]): `Activate` brings the most recently
//! opened browser window forward (or, with none open, the hidden progress window while it has something to list, else
//! a new window on the home folder), `Open` opens a window on each folder, `ShowItems` selects each item in a window on
//! its folder (one window a folder, a window already showing it when there is one), and the actions show the progress
//! window or open a new window. A click on the notice of a file operation's end shows the progress window with it, as
//! `Activate` does when nothing is left to list. The window a call opens or brings forward is raised with the call's
//! activation token once it is shown.

use std::path::PathBuf;

use cupertino_kit::prelude::*;

use super::{progress, window_key, woken, Message, Model};
use crate::instance::{Action, Call, Request, Target};
use crate::notice::{Action as Failed, Click};

/// Raise the window `key` with `token` once it has been shown: at once when it already has, else after its first frame,
/// so the compositor finds a mapped window for the token.
pub(super) fn raise(key: WindowKey, token: Option<String>) -> Command<Message> {
    Command::OnPresented {
        window: key.clone(),
        message: Message::Raise(key, token),
    }
}

impl Model {
    /// Do what each waiting call asks, answer it, and wait for the next.
    pub(super) fn remote_calls(&mut self) -> Vec<Command<Message>> {
        let Some(instance) = &self.session.instance else {
            return Vec::new();
        };
        let calls = instance.take();
        let mut commands = vec![woken(instance.woken(), Message::Remote)];
        if !calls.is_empty() {
            // the call a Files started without a window waits for came: it exits once nothing is left open
            self.awaiting_call = false;
        }
        for call in calls {
            commands.extend(self.remote_call(&call));
            call.done();
        }
        commands
    }

    fn remote_call(&mut self, call: &Call) -> Vec<Command<Message>> {
        let token = call.token.clone();
        match &call.request {
            Request::Activate => self.activate(token),
            Request::Open(targets) => {
                let mut commands = Vec::new();
                let mut last = None;
                for target in targets {
                    let Some(folder) = self.folder_of(target) else {
                        continue;
                    };
                    if let Some((id, wake)) = self.open_window(folder) {
                        commands.push(wake);
                        last = Some(id);
                    }
                }
                // a token raises one window: the last one opened, which the others open beneath
                commands.extend(last.map(|id| raise(window_key(id), token)));
                commands
            }
            Request::ShowItems(targets) => self.show_items(targets, token),
            Request::Action(Action::NewWindow) => self.open_home(token),
            Request::Action(Action::ShowProgress) => {
                // nothing to list: the progress window stays closed, as Window › Show Progress is disabled then
                self.show_progress(token).unwrap_or_default()
            }
        }
    }

    /// Bring Files forward: its most recently opened browser window; with none open, the progress window when it is
    /// hidden with something to list (a file operation runs on); else a new window on the home folder.
    fn activate(&mut self, token: Option<String>) -> Vec<Command<Message>> {
        if let Some(&id) = self.windows.keys().next_back() {
            return vec![raise(window_key(id), token)];
        }
        let jobs = &self.session.jobs;
        if !jobs.visible() && !jobs.is_empty() {
            if let Some(commands) = self.show_progress(token.clone()) {
                return commands;
            }
        }
        self.open_home(token)
    }

    /// Show the progress window raised with `token`, when it has something to list.
    fn show_progress(&mut self, token: Option<String>) -> Option<Vec<Command<Message>>> {
        let jobs = &mut self.session.jobs;
        jobs.show();
        jobs.visible()
            .then(|| vec![raise(WindowKey(Key::from(progress::KEY)), token)])
    }

    /// A notice of a file operation's end was clicked: the progress window shows its failure again, or what still
    /// runs; with neither, Files comes forward as a launch brings it. Then the next click is waited for.
    pub(super) fn noticed(&mut self, click: Option<Click>) -> Vec<Command<Message>> {
        let Some(Click { tag, token }) = click else {
            return Vec::new();
        };
        let session = &mut self.session;
        let failure = session
            .noticed
            .iter()
            .find(|(id, _)| *id == tag)
            .and_then(|(_, failure)| failure.clone());
        if let Some(failure) = failure {
            session.jobs.failed(failure);
        }
        let mut commands = match self.show_progress(token.clone()) {
            Some(commands) => commands,
            None => self.activate(token),
        };
        commands.extend(self.session.notice_clicks());
        commands
    }

    /// A new window on the home folder, raised with `token`.
    fn open_home(&mut self, token: Option<String>) -> Vec<Command<Message>> {
        let home = self.session.home.clone();
        match self.open_window(home) {
            Some((id, wake)) => vec![wake, raise(window_key(id), token)],
            None => Vec::new(),
        }
    }

    /// Show each item of `targets` selected in a window on its folder, raised with `token`: the items of one folder in
    /// one window, the newest window that shows the folder (without a search) when there is one, else a new one; the
    /// trash is opened itself, and the root shows itself. An item that does not exist is a notice.
    fn show_items(&mut self, targets: &[Target], token: Option<String>) -> Vec<Command<Message>> {
        // each folder with its items, in the order the call names them
        let mut folders: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
        for target in targets {
            let (folder, item) = match target {
                Target::Trash => match self.folder_of(target) {
                    Some(folder) => (folder, None),
                    None => continue,
                },
                Target::Folder(item) => {
                    if let Err(e) = std::fs::symlink_metadata(item) {
                        self.session.failed(Failed::Open, &[item], &e);
                        continue;
                    }
                    match item.parent() {
                        Some(folder) => (folder.to_path_buf(), Some(item.clone())),
                        None => (item.clone(), None),
                    }
                }
            };
            match folders.iter_mut().find(|(f, _)| *f == folder) {
                Some((_, items)) => items.extend(item),
                None => folders.push((folder, item.into_iter().collect())),
            }
        }
        let mut commands = Vec::new();
        let mut last = None;
        for (folder, items) in folders {
            let shown = self
                .windows
                .iter()
                .rev()
                .find(|(_, w)| w.browser.dir() == folder && w.search.is_empty())
                .map(|(&id, _)| id);
            let id = match shown {
                Some(id) => id,
                None => match self.open_window(folder) {
                    Some((id, wake)) => {
                        commands.push(wake);
                        id
                    }
                    None => continue,
                },
            };
            last = Some(id);
            if items.is_empty() {
                continue;
            }
            let window = self.windows.get_mut(&id).expect("the window is open");
            window.browser.reveal(items);
            // a folder already read is selected at once; a new window's once its listing is read
            if let Some(path) = window.browser.take_revealed() {
                commands.extend(window.focus_item(id, &path));
            }
        }
        // a token raises one window: the last one shown, which the others open beneath
        commands.extend(last.map(|id| raise(window_key(id), token)));
        commands
    }

    /// The folder a window shows for `target`; a trash that cannot be made is a notice.
    fn folder_of(&self, target: &Target) -> Option<PathBuf> {
        let session = &self.session;
        target
            .folder(&session.trash)
            .map_err(|e| session.failed(Failed::Open, &[session.trash.home_trash()], &e))
            .ok()
    }
}
