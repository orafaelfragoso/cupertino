//! What the calls of later launches and of the desktop do ([`crate::instance`]): `Activate` brings the most recently
//! opened browser window forward (or opens one on the home folder), `Open` opens a window on each folder, and the
//! actions show the progress window or open a new window. The window a call opens or brings forward is raised with
//! the call's activation token once it is shown.

use std::path::PathBuf;

use cupertino_kit::prelude::*;

use super::{progress, window_key, woken, Message, Model};
use crate::instance::{Action, Call, Request, Target};
use crate::notice::Action as Failed;

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
        for call in calls {
            commands.extend(self.remote_call(&call));
            call.done();
        }
        commands
    }

    fn remote_call(&mut self, call: &Call) -> Vec<Command<Message>> {
        let token = call.token.clone();
        match &call.request {
            Request::Activate => match self.windows.keys().next_back() {
                Some(&id) => vec![raise(window_key(id), token)],
                None => self.open_home(token),
            },
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
            Request::Action(Action::NewWindow) => self.open_home(token),
            Request::Action(Action::ShowProgress) => {
                let jobs = &mut self.session.jobs;
                jobs.show();
                if jobs.visible() {
                    vec![raise(WindowKey(Key::from(progress::KEY)), token)]
                } else {
                    // nothing to list: the progress window stays closed, as Window › Show Progress is disabled then
                    Vec::new()
                }
            }
        }
    }

    /// A new window on the home folder, raised with `token`.
    fn open_home(&mut self, token: Option<String>) -> Vec<Command<Message>> {
        let home = self.session.home.clone();
        match self.open_window(home) {
            Some((id, wake)) => vec![wake, raise(window_key(id), token)],
            None => Vec::new(),
        }
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
