//! What the calls of later launches and of the desktop do ([`crate::instance`]): `Activate` brings the most recently
//! opened browser window forward (or, with none open, the hidden progress window while it has something to list, else
//! a new window on the home folder), `Open` opens a window on each folder, `ShowItems` selects each item in a window on
//! its folder (one window a folder, a window already showing it when there is one) once a thread of its own has found
//! which of them exist, so an item on a mount that does not answer never holds the windows still, and the actions show the progress
//! window or open a new window. A click on the notice of a file operation's end shows the progress window with it, as
//! `Activate` does when nothing is left to list. The window a call opens or brings forward is raised with the call's
//! activation token once it is shown.

use std::io;
use std::path::{Path, PathBuf};

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
            commands.extend(self.remote_call(call));
        }
        commands
    }

    /// Do what `call` asks and answer it; `ShowItems` is answered once its items are shown ([`Model::look_up`]).
    fn remote_call(&mut self, call: Call) -> Vec<Command<Message>> {
        if matches!(call.request, Request::ShowItems(_)) {
            return self.look_up(call);
        }
        let commands = self.take_call(&call);
        call.done();
        commands
    }

    fn take_call(&mut self, call: &Call) -> Vec<Command<Message>> {
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
            Request::ShowItems(_) => Vec::new(),
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

    /// Find out on a thread of its own which items of the `ShowItems` call `call` exist, so an item on a mount that
    /// does not answer holds no window still; the items are shown, and the call answered, once it is known
    /// ([`Model::looked_up`]). Past [`MAX_LOOK_UPS`] such threads at once, the call is refused.
    fn look_up(&mut self, call: Call) -> Vec<Command<Message>> {
        let Request::ShowItems(targets) = &call.request else {
            return Vec::new();
        };
        if self.look_ups.len() >= MAX_LOOK_UPS {
            call.refuse("too many items are being looked up");
            return Vec::new();
        }
        let paths: Vec<PathBuf> = targets
            .iter()
            .filter_map(|t| match t {
                Target::Folder(path) => Some(path.clone()),
                Target::Trash => None,
            })
            .collect();
        let Some(found) = look_up_items(paths, |path| std::fs::symlink_metadata(path).map(drop))
        else {
            call.refuse("no thread could look the items up");
            return Vec::new();
        };
        let id = self.next_look_up;
        self.next_look_up += 1;
        self.look_ups.insert(id, call);
        vec![Command::Perform(Box::pin(async move {
            Message::LookedUp(id, found.recv().await.unwrap_or_default())
        }))]
    }

    /// The items of the `ShowItems` call `id` were looked up (`found`, one a path it names, in order): show them, and
    /// answer the call.
    pub(super) fn looked_up(&mut self, id: u64, found: Vec<Found>) -> Vec<Command<Message>> {
        let Some(call) = self.look_ups.remove(&id) else {
            return Vec::new();
        };
        let commands = match &call.request {
            Request::ShowItems(targets) => self.show_items(targets, &found, call.token.clone()),
            _ => Vec::new(),
        };
        call.done();
        commands
    }

    /// Show each item of `targets` selected in a window on its folder, raised with `token`: the items of one folder in
    /// one window, the newest window that shows the folder (without a search) when there is one, else a new one; the
    /// trash is opened itself, and the root shows itself. An item that does not exist (`found`, one a path in order;
    /// a path past its end was not looked up) is a notice.
    fn show_items(
        &mut self,
        targets: &[Target],
        found: &[Found],
        token: Option<String>,
    ) -> Vec<Command<Message>> {
        // each folder with its items, in the order the call names them
        let mut folders: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
        let mut found = found.iter();
        for target in targets {
            let (folder, item) = match target {
                Target::Trash => match self.folder_of(target) {
                    Some(folder) => (folder, None),
                    None => continue,
                },
                Target::Folder(item) => {
                    let missing = match found.next() {
                        Some(Found::Here) => None,
                        Some(Found::Missing(e)) => Some(e.error()),
                        None => Some(io::Error::new(io::ErrorKind::TimedOut, "not looked up")),
                    };
                    if let Some(e) = missing {
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

/// The most `ShowItems` calls whose items are looked up at once: each on a thread that a mount which does not answer
/// can hold.
pub(super) const MAX_LOOK_UPS: usize = 8;

/// Whether an item a `ShowItems` call names exists, as its thread found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    Here,
    Missing(LookUpError),
}

/// Why an item could not be looked up: what an [`io::Error`] says, kept so the message carrying it can be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LookUpError {
    kind: io::ErrorKind,
    code: Option<i32>,
}

impl LookUpError {
    fn of(e: &io::Error) -> LookUpError {
        LookUpError {
            kind: e.kind(),
            code: e.raw_os_error(),
        }
    }

    fn error(self) -> io::Error {
        match self.code {
            Some(code) => io::Error::from_raw_os_error(code),
            None => self.kind.into(),
        }
    }
}

/// Look up each of `paths` with `look` on a thread of its own: what was found arrives on the channel once every path
/// has been looked up, in order. `None` when no thread could be started.
fn look_up_items(
    paths: Vec<PathBuf>,
    look: impl Fn(&Path) -> io::Result<()> + Send + 'static,
) -> Option<async_channel::Receiver<Vec<Found>>> {
    let (sent, found) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("files-look-up".into())
        .spawn(move || {
            let looked = paths
                .iter()
                .map(|path| match look(path) {
                    Ok(()) => Found::Here,
                    Err(e) => Found::Missing(LookUpError::of(&e)),
                })
                .collect();
            let _ = sent.try_send(looked);
        })
        .ok()?;
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn items_are_looked_up_on_a_thread_of_their_own() {
        let (release, held) = std::sync::mpsc::channel::<()>();
        let held = std::sync::Mutex::new(held);
        // the first look-up does not answer until released, as a folder on a mount that hangs
        let found = look_up_items(vec!["/here".into(), "/gone".into()], move |path| {
            if path == Path::new("/here") {
                let _ = held.lock().unwrap().recv_timeout(Duration::from_secs(60));
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(libc::ENOENT))
            }
        })
        .expect("a thread");
        // the caller is back at once, with nothing found yet
        assert!(found.try_recv().is_err());
        release.send(()).unwrap();
        let looked = zbus::block_on(found.recv()).expect("what was found");
        assert_eq!(looked[0], Found::Here);
        let Found::Missing(e) = looked[1] else {
            panic!("{looked:?}");
        };
        assert_eq!(e.error().raw_os_error(), Some(libc::ENOENT));
        assert_eq!(e.error().kind(), io::ErrorKind::NotFound);
    }
}
