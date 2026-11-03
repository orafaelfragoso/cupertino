//! The browser window on CupertinoKit: the sidebar's places, the folder as a list, the path and status bars and the
//! menus of the desktop's menu bar, over the toolkit-free [`Browser`]. It takes the place of the window in
//! `main.rs` once it does all that window does.

mod view;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cupertino_apps::{launch, MimeApps, MimeDatabase};
use cupertino_kit::controls::{Navigation, Selection, SortOrder, SplitLayout, TextChange};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::prelude::*;
use cupertino_menu_model::ActionName;

use crate::browser::{Activation, Browser};
use crate::menu_bar::{self, MenuAction, MenuState};
use crate::name_edit::NameEdit;
use crate::notice::{self, Action, Notices};
use crate::open::{open_command, OpenError};
use crate::operations;
use crate::places::Place;
use crate::settings::ColumnId;
use crate::trash::Trash;
use crate::window::ViewMode;
use crate::SortSpec;

/// Identifier of the window for the server, as the desktop entry names it.
pub const APP_ID: &str = "cupertino-files";

/// What a window opens with; [`Start::open`] lists the folder before the toolkit runs.
pub struct Start {
    browser: Browser,
    woken: async_channel::Receiver<()>,
    places: Vec<Place>,
    notices: Notices,
    /// What opens a file: the session's associations, read at each open so changed defaults apply at once.
    applications: Box<dyn Fn() -> io::Result<MimeApps>>,
    terminal: Option<Vec<String>>,
    trash: Trash,
}

impl Start {
    /// The window on `folder` with the sidebar's `places`, its items typed with `types` and the Recents folder read
    /// from `recents`.
    pub fn open(
        folder: PathBuf,
        places: Vec<Place>,
        types: Option<Arc<MimeDatabase>>,
        recents: PathBuf,
        notices: Notices,
        applications: Box<dyn Fn() -> io::Result<MimeApps>>,
        terminal: Option<Vec<String>>,
    ) -> io::Result<Start> {
        let (wake, woken) = async_channel::bounded(1);
        let browser = Browser::open(
            folder,
            SortSpec::default(),
            recents,
            types,
            Arc::new(move || {
                // a wake already waiting covers this change too
                let _ = wake.try_send(());
            }),
        )?;
        Ok(Start {
            browser,
            woken,
            places,
            notices,
            applications,
            terminal,
            trash: Trash::new(),
        })
    }

    /// The window moving items to `trash` and putting them back from it, in place of the session's trash.
    pub fn with_trash(self, trash: Trash) -> Start {
        Start { trash, ..self }
    }
}

thread_local! {
    static START: RefCell<Option<Start>> = const { RefCell::new(None) };
}

/// Hand `start` to the next [`Files`] that starts on this thread.
pub fn prepare(start: Start) {
    START.with(|s| *s.borrow_mut() = Some(start));
}

/// The application of one browser window. [`prepare`] gives it its folder first.
pub struct Files;

pub struct Model {
    browser: Browser,
    woken: async_channel::Receiver<()>,
    places: Vec<Place>,
    notices: Notices,
    applications: Box<dyn Fn() -> io::Result<MimeApps>>,
    terminal: Option<Vec<String>>,
    trash: Trash,
    path_bar: bool,
    status_bar: bool,
    /// The free space of the folder's file system, for the status bar.
    available: Option<u64>,
    search: String,
    search_revision: u64,
    scroll: ScrollState,
    sidebar_scroll: ScrollState,
    split: SplitLayout,
    /// When the dates of the list were last worded.
    now: SystemTime,
    /// The name being edited in place.
    rename: Option<Rename>,
    /// A folder just made, whose name is edited once the list shows it.
    rename_when_listed: Option<PathBuf>,
}

/// An item's name edited in its row of the list.
struct Rename {
    edit: NameEdit,
    path: PathBuf,
    revision: u64,
    /// What the field selects when it opens: the name without its extension, in bytes.
    selection: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// The folder's model has changes waiting.
    Woken,
    Place(usize),
    Navigate(Navigation),
    Select(Selection),
    Open(usize),
    Sort((usize, SortOrder)),
    Search(TextChange),
    Menu(ActionName),
    /// The text of the name being edited changed.
    NameChanged(TextChange),
    /// Return in the name being edited: rename the item.
    NameSubmitted,
    /// Escape in the name being edited: keep the old name.
    NameCancelled,
    /// The name field is in the window: give it the keyboard.
    FocusName,
    Scroll(ScrollState),
    SidebarScroll(ScrollState),
    Split(SplitLayout),
}

/// The list's columns, in order.
const COLUMNS: [ColumnId; 4] = [
    ColumnId::Name,
    ColumnId::DateModified,
    ColumnId::Size,
    ColumnId::Kind,
];

const NO_SCROLL: ScrollState = ScrollState {
    offset: cupertino_kit::geometry::Point::ZERO,
    content: cupertino_kit::geometry::Size::ZERO,
    viewport: cupertino_kit::geometry::Size::ZERO,
};

/// Menu items the window does not do yet; they are shown disabled.
const NOT_YET: [MenuAction; 7] = [
    MenuAction::View(ViewMode::Icons),
    MenuAction::NewWindow,
    MenuAction::OpenInNewWindow,
    MenuAction::MakeAlias,
    MenuAction::ShowOriginal,
    MenuAction::Eject,
    MenuAction::FullScreen,
];

fn woken(receiver: &async_channel::Receiver<()>) -> Command<Message> {
    let receiver = receiver.clone();
    Command::Perform(Box::pin(async move {
        let _ = receiver.recv().await;
        Message::Woken
    }))
}

impl Application for Files {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let start = START
            .with(|s| s.borrow_mut().take())
            .expect("kit::prepare runs before Files starts");
        let mut model = Model {
            browser: start.browser,
            woken: start.woken,
            places: start.places,
            notices: start.notices,
            applications: start.applications,
            terminal: start.terminal,
            trash: start.trash,
            path_bar: false,
            status_bar: false,
            available: None,
            search: String::new(),
            search_revision: 0,
            scroll: NO_SCROLL,
            sidebar_scroll: NO_SCROLL,
            split: SplitLayout {
                position: view::SIDEBAR_WIDTH,
                collapsed: None,
            },
            now: SystemTime::now(),
            rename: None,
            rename_when_listed: None,
        };
        model.entered();
        let commands = vec![woken(&model.woken)];
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Woken => {
                let polled = model.browser.poll();
                for error in polled.errors {
                    eprintln!("cupertino-files: {error}");
                }
                if polled.changed {
                    model.now = SystemTime::now();
                }
                let mut commands = model.listed();
                commands.push(woken(&model.woken));
                return commands;
            }
            Message::Place(index) => {
                if let Some(folder) = model.places.get(index).and_then(Place::folder) {
                    model.go(folder.to_path_buf());
                }
            }
            Message::Navigate(Navigation::Back) => model.step(Step::Back),
            Message::Navigate(Navigation::Forward) => model.step(Step::Forward),
            Message::Select(selection) => {
                let indices = selection.ranges().iter().flat_map(Clone::clone);
                model
                    .browser
                    .select(indices, selection.anchor(), selection.cursor());
                let renamed = model.rename_index();
                if renamed.is_some() && model.browser.selection().indices().ne(renamed) {
                    model.end_rename();
                }
            }
            Message::Open(index) => {
                if !model.browser.selection().contains(index) {
                    model.browser.select([index], Some(index), Some(index));
                }
                model.open_selection();
            }
            Message::Sort((column, order)) => {
                if let Some(id) = COLUMNS.get(column) {
                    model
                        .browser
                        .set_sort(id.sort_key(), order == SortOrder::Ascending);
                }
            }
            Message::Search(change) => {
                model.browser.set_filter(&change.text);
                model.search = change.text;
                model.search_revision = change.revision;
            }
            Message::Menu(name) => {
                if let Some(action) = MenuAction::from_name(&name) {
                    return model.menu(action);
                }
            }
            Message::NameChanged(change) => {
                if let Some(rename) = model.rename.as_mut() {
                    rename.edit.field_mut().set_text(&change.text);
                    rename.revision = change.revision;
                }
            }
            Message::NameSubmitted => model.end_rename(),
            Message::NameCancelled => model.rename = None,
            Message::FocusName => {
                if model.rename.is_some() {
                    return vec![Command::Focus(Key::from(view::NAME_FIELD))];
                }
            }
            Message::Scroll(scroll) => model.scroll = scroll,
            Message::SidebarScroll(scroll) => model.sidebar_scroll = scroll,
            Message::Split(split) => model.split = split,
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        vec![view::window(model)]
    }
}

enum Step {
    Back,
    Forward,
    Parent,
}

impl Model {
    fn go(&mut self, to: PathBuf) {
        match self.browser.go(to.clone()) {
            Ok(()) => self.entered(),
            Err(e) => self.failed(Action::Open, &[&to], &e),
        }
    }

    fn step(&mut self, step: Step) {
        let target = match step {
            Step::Back => self.browser.history().back_target(),
            Step::Forward => self.browser.history().forward_target(),
            Step::Parent => self.browser.dir().parent(),
        }
        .map(|p| crate::places::window_title(p, &self.places));
        let outcome = match step {
            Step::Back => self.browser.back(),
            Step::Forward => self.browser.forward(),
            Step::Parent => self.browser.parent(),
        };
        match outcome {
            Ok(true) => self.entered(),
            Ok(false) => {}
            Err(e) => self
                .notices
                .post(notice::unopened(target.as_deref().unwrap_or_default(), &e)),
        }
    }

    /// A new folder starts at the top, its search cleared and its free space read.
    fn entered(&mut self) {
        self.scroll = NO_SCROLL;
        self.rename = None;
        self.rename_when_listed = None;
        if !self.search.is_empty() {
            self.search.clear();
            self.search_revision += 1;
        }
        self.now = SystemTime::now();
        self.available = match self.browser.available_space() {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!(
                    "cupertino-files: cannot read the free space of {}: {e}",
                    self.browser.dir().display()
                );
                None
            }
        };
    }

    fn failed(&self, action: Action, items: &[&Path], error: &dyn std::fmt::Display) {
        self.notices.post(notice::failure(action, items, error));
    }

    /// Report the items of `results` that failed as one notice, worded by the first failure.
    fn failed_some<'a, T, E: std::fmt::Display>(
        &self,
        action: Action,
        results: impl IntoIterator<Item = (&'a PathBuf, Result<T, E>)>,
    ) {
        let mut failed = Vec::new();
        let mut first = None;
        for (path, result) in results {
            if let Err(e) = result {
                failed.push(path.as_path());
                first.get_or_insert(e);
            }
        }
        if let Some(e) = first {
            self.failed(action, &failed, &e);
        }
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        let entries = self.browser.entries();
        self.browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
            .map(|e| e.path.clone())
            .collect()
    }

    /// Move the selected items to the trash; the listing drops them when the watcher reports it.
    fn trash_selection(&self) {
        let paths = self.selected_paths();
        self.failed_some(
            Action::Trash,
            paths.iter().map(|path| (path, self.trash.trash(path))),
        );
    }

    /// Put the selected items of the trash back where they came from.
    fn put_back(&self) {
        let paths = self.selected_paths();
        let listed = match self.trash.list() {
            Ok(listed) => listed,
            Err(e) => {
                let items: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
                return self.failed(Action::PutBack, &items, &e);
            }
        };
        let restored = paths.iter().map(|path| {
            let restored = match listed.iter().find(|item| item.file_path() == *path) {
                Some(item) => self.trash.restore(item).map_err(|e| e.to_string()),
                None => Err("The Trash has no record of where it came from".to_string()),
            };
            (path, restored)
        });
        self.failed_some(Action::PutBack, restored);
    }

    /// Go into the selected folder, or open the selected files with their default applications.
    fn open_selection(&mut self) {
        let Activation { folder, files } = self.browser.activation();
        if !files.is_empty() {
            match (self.applications)() {
                Ok(apps) => {
                    for path in files {
                        self.launch(&apps, &path);
                    }
                }
                Err(e) => {
                    let items: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
                    self.failed(Action::Open, &items, &e);
                }
            }
        }
        if let Some(folder) = folder {
            self.go(folder);
        }
    }

    fn launch(&self, apps: &MimeApps, path: &Path) {
        match open_command(apps, path, false, self.terminal.as_deref()) {
            Ok(spec) => {
                launch::spawn(APP_ID, spec.to_command());
            }
            Err(OpenError::NoApplication { .. }) => {
                self.failed(Action::Open, &[path], &"No application is set to open it")
            }
            Err(e) => self.failed(Action::Open, &[path], &e),
        }
    }

    fn menu(&mut self, action: MenuAction) -> Vec<Command<Message>> {
        match action {
            MenuAction::Open => self.open_selection(),
            MenuAction::CloseWindow => return vec![Command::Exit],
            MenuAction::SelectAll => self.browser.select_all(),
            MenuAction::SortBy(key, ascending) => self.browser.set_sort(key, ascending),
            MenuAction::TogglePathBar => self.path_bar = !self.path_bar,
            MenuAction::ToggleStatusBar => self.status_bar = !self.status_bar,
            MenuAction::Back => self.step(Step::Back),
            MenuAction::Forward => self.step(Step::Forward),
            MenuAction::EnclosingFolder => self.step(Step::Parent),
            MenuAction::MoveToTrash => self.trash_selection(),
            MenuAction::PutBack => self.put_back(),
            MenuAction::NewFolder => match operations::new_folder(self.browser.dir()) {
                Ok(path) => self.rename_new_folder(path),
                Err(e) => self.failed(Action::NewFolder, &[], &e),
            },
            MenuAction::NewFolderWithSelection => {
                let paths = self.selected_paths();
                match operations::new_folder_with(self.browser.dir(), &paths) {
                    Ok(path) => self.rename_new_folder(path),
                    Err(e) => {
                        let items: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
                        self.failed(Action::MoveIntoNewFolder, &items, &e);
                    }
                }
            }
            MenuAction::Rename => {
                let first = self.browser.selection().indices().next();
                if let Some(index) = first {
                    return self.begin_rename(index);
                }
            }
            MenuAction::Duplicate => {
                let paths = self.selected_paths();
                let duplicated = operations::duplicate(&paths);
                self.failed_some(Action::Duplicate, paths.iter().zip(duplicated));
            }
            MenuAction::View(_)
            | MenuAction::NewWindow
            | MenuAction::OpenInNewWindow
            | MenuAction::MakeAlias
            | MenuAction::ShowOriginal
            | MenuAction::Eject
            | MenuAction::FullScreen => {}
        }
        Vec::new()
    }

    /// A folder was made: the list selects it when it shows it, and its name is edited then.
    fn rename_new_folder(&mut self, path: PathBuf) {
        self.browser.select_when_appears(path.clone());
        self.rename_when_listed = Some(path);
    }

    /// Edit the name of item `index` in its row, if it is the only selected item. The field is focused once the
    /// window shows it.
    fn begin_rename(&mut self, index: usize) -> Vec<Command<Message>> {
        let selection = self.browser.selection();
        if selection.len() != 1 || !selection.contains(index) {
            return Vec::new();
        }
        let Some(entry) = self.browser.entries().get(index) else {
            return Vec::new();
        };
        let edit = NameEdit::new(index, &entry.name, entry.is_dir);
        let revision = self.rename.as_ref().map_or(0, |r| r.revision + 1);
        self.rename = Some(Rename {
            selection: edit.field().selection(),
            edit,
            path: entry.path.clone(),
            revision,
        });
        vec![Command::Perform(Box::pin(async { Message::FocusName }))]
    }

    /// The row of the name being edited.
    fn rename_index(&self) -> Option<usize> {
        self.rename.as_ref().map(|r| r.edit.index())
    }

    /// Commit the edited name. A name that is not valid is reported and the field stays open. A rename that changes
    /// the extension is not confirmed (there is no dialog to ask in).
    fn end_rename(&mut self) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        match rename.edit.commit() {
            Ok(None) => {}
            Ok(Some(name)) => match operations::rename_item(&rename.path, &name) {
                Ok(path) => self.browser.select_when_appears(path),
                Err(e) => self.failed(Action::Rename, &[&rename.path], &e),
            },
            Err(e) => {
                self.notices.post(notice::bad_name(rename.edit.text(), &e));
                self.rename = Some(rename);
            }
        }
    }

    /// The list changed: the name being edited follows its item, and a folder just made starts its rename.
    fn listed(&mut self) -> Vec<Command<Message>> {
        let entries = self.browser.entries();
        if let Some(rename) = self.rename.as_mut() {
            match entries.iter().position(|e| e.path == rename.path) {
                Some(index) => rename.edit.set_index(index),
                None => self.rename = None,
            }
        }
        let made = self
            .rename_when_listed
            .as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path));
        match made {
            Some(index) => {
                self.rename_when_listed = None;
                self.begin_rename(index)
            }
            None => Vec::new(),
        }
    }

    fn menu_state(&self) -> MenuState {
        let selected: Vec<_> = self
            .browser
            .selection()
            .indices()
            .filter_map(|i| self.browser.entries().get(i))
            .collect();
        let history = self.browser.history();
        MenuState {
            count: selected.len(),
            all_folders: !selected.is_empty() && selected.iter().all(|e| e.is_dir),
            single_link: matches!(selected.as_slice(), [e] if e.kind == crate::Kind::Symlink),
            in_trash: self.browser.dir() == self.trash.home_trash().join("files"),
            path_bar: self.path_bar,
            status_bar: self.status_bar,
            full_screen: false,
            can_go_back: history.back_target().is_some(),
            can_go_forward: history.forward_target().is_some(),
            has_parent: !self.browser.is_virtual() && self.browser.dir().parent().is_some(),
            eject: None,
        }
    }

    /// The actions the menus show but the window cannot do now.
    fn disabled(&self, state: &MenuState) -> BTreeSet<ActionName> {
        menu_bar::enabled(state)
            .into_iter()
            .filter(|(_, enabled)| !enabled)
            .map(|(action, _)| action)
            .chain(NOT_YET)
            .map(|action| action.name())
            .collect()
    }
}

/// Open the window on `start` and serve it until it closes.
pub fn run(start: Start) -> Result<(), cupertino_kit::app::AppError> {
    prepare(start);
    cupertino_kit::app::run::<Files>()
}
