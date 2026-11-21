//! Opening items and showing a link's original: a launch waits for the activation token the window asks for, and the
//! file system reads it needs (where a link leads) run as I/O tasks whose answers come back as messages, so the UI
//! thread touches no file. Whether an item is a folder comes from its listing's entry.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cupertino_kit::files::notice::Action;
use cupertino_kit::files::open::{
    command_with, default_app_id, open_command, open_with, OpenError,
};
use cupertino_kit::prelude::Command;

use super::{window_key, BrowserWindow, Event, Message, Session, APP_ID};

/// How long resolving a link's target should take: one `realpath`, which only a hung network mount holds up.
const ORIGINAL_DEADLINE: Duration = Duration::from_secs(5);

/// An item to open and whether it is a folder, as its listing's entry tells.
pub(super) struct Item {
    pub path: PathBuf,
    pub is_dir: bool,
}

impl Session {
    /// Open `path` (a folder when `is_dir`) with the application of desktop entry `with`, else its default one,
    /// handing it `token`. The associations are read at each open, so changed defaults and new applications apply at
    /// once.
    pub(super) fn launch(
        &self,
        path: &Path,
        is_dir: bool,
        with: Option<&str>,
        token: Option<&str>,
    ) {
        let terminal = self.terminal.as_deref();
        let spec = (self.applications)()
            .map_err(|e| e.to_string())
            .and_then(|apps| match with {
                None => open_command(&apps, path, is_dir, terminal).map_err(|e| match e {
                    OpenError::NoApplication { .. } => "No application is set to open it".into(),
                    e => e.to_string(),
                }),
                Some(id) => {
                    let item = open_with(&apps, path, is_dir)
                        .into_iter()
                        .find(|item| item.entry.id == id)
                        .ok_or_else(|| format!("{id} no longer opens it"))?;
                    command_with(item.entry, path, terminal).map_err(|e| e.to_string())
                }
            });
        match spec {
            Ok(spec) => {
                let spec = match token {
                    Some(token) => spec.with_activation_token(token),
                    None => spec,
                };
                cupertino_kit::spawn(APP_ID, spec.to_command());
            }
            Err(e) => self.failed(Action::Open, &[path], &e),
        }
    }

    /// Ask the window `id` for an activation token for each of `items`, naming the application that will open it, so
    /// the dock shows it starting; each opens once its token comes.
    pub(super) fn open_paths(
        &self,
        id: u64,
        items: Vec<Item>,
        with: Option<String>,
    ) -> Vec<Command<Message>> {
        if items.is_empty() {
            return Vec::new();
        }
        let apps = match (self.applications)() {
            Ok(apps) => apps,
            Err(e) => {
                let paths: Vec<&Path> = items.iter().map(|item| item.path.as_path()).collect();
                self.failed(Action::Open, &paths, &e);
                return Vec::new();
            }
        };
        items
            .into_iter()
            .map(|Item { path, is_dir }| {
                let app_id = with
                    .clone()
                    .or_else(|| default_app_id(&apps, &path, is_dir));
                let with = with.clone();
                Command::ActivationToken {
                    window: window_key(id),
                    app_id,
                    on_token: Box::new(move |token| {
                        Message::Window(
                            id,
                            Event::Launch {
                                path,
                                is_dir,
                                with,
                                token,
                            },
                        )
                    }),
                }
            })
            .collect()
    }
}

impl BrowserWindow {
    /// The selected items, each with whether it is a folder, as the listing tells.
    pub(super) fn selected_items(&self) -> Vec<Item> {
        let entries = self.browser.entries();
        self.browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
            .map(|e| Item {
                path: e.path.clone(),
                is_dir: e.is_dir,
            })
            .collect()
    }

    /// Whether `path` is a folder, as the listing's entry for it tells; a path not listed is none.
    pub(super) fn is_dir(&self, path: &Path) -> bool {
        self.browser
            .entries()
            .iter()
            .find(|e| e.path == path)
            .is_some_and(|e| e.is_dir)
    }

    /// Look up where the selected link leads, on an I/O task; the answer is an [`Event::Original`].
    pub(super) fn show_original(&self, id: u64) -> Vec<Command<Message>> {
        let Some(link) = self.selected_paths().into_iter().next() else {
            return Vec::new();
        };
        let resolve = link.clone();
        vec![cupertino_kit::task::io(
            ORIGINAL_DEADLINE,
            move || original(&resolve),
            move |target| Message::Window(id, Event::Original(link, target)),
        )]
    }

    /// The target of `link` was looked up: go to its folder and select it, or tell why not.
    pub(super) fn original_found(
        &mut self,
        link: &Path,
        target: Result<PathBuf, String>,
        session: &Session,
    ) {
        let target = match target {
            Ok(target) => target,
            Err(e) => return session.failed(Action::ShowOriginal, &[link], &e),
        };
        let Some(parent) = target.parent().map(Path::to_path_buf) else {
            return;
        };
        self.go_selecting(parent, Some(target));
    }
}

/// Where `link` leads, every link on the way followed, or why it cannot be found.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn original(link: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(link).map_err(|e| e.to_string())
}
