//! The browser window on CupertinoKit: the sidebar's places, the folder as a list or a grid of icons, the path and
//! status bars and the menus of the desktop's menu bar, over the toolkit-free [`Browser`]. It takes the place of the
//! window in `main.rs` once it does all that window does.

mod grid;
mod view;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cupertino_apps::{launch, MimeApps, MimeDatabase};
use cupertino_kit::controls::{Navigation, Selection, SortOrder, SplitLayout, TextChange};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::WindowRequest;
use cupertino_kit::prelude::*;
use cupertino_menu_model::ActionName;
use zbus::blocking::Connection;

use crate::browser::{Activation, Browser};
use crate::menu_bar::{self, MenuAction, MenuState};
use crate::name_edit::NameEdit;
use crate::notice::{self, Action, Notices};
use crate::open::{open_command, OpenError};
use crate::operations;
use crate::places::{removable_holding, Place};
use crate::settings::ColumnId;
use crate::trash::Trash;
use crate::udisks::{Media, Volumes};
use crate::window::ViewMode;
use crate::SortSpec;

/// Identifier of the window for the server, as the desktop entry names it.
pub const APP_ID: &str = "cupertino-files";

/// What the first window opens with; [`Start::open`] lists the folder before the toolkit runs.
pub struct Start {
    browser: Browser,
    woken: async_channel::Receiver<()>,
    places: Vec<Place>,
    notices: Notices,
    /// What opens a file: the session's associations, read at each open so changed defaults apply at once.
    applications: Box<dyn Fn() -> io::Result<MimeApps>>,
    terminal: Option<Vec<String>>,
    types: Option<Arc<MimeDatabase>>,
    recents: PathBuf,
    volumes: Option<Volumes>,
    media_woken: async_channel::Receiver<()>,
    trash: Trash,
}

impl Start {
    /// The window on `folder` with the sidebar's `places`, its items typed with `types`, the Recents folder read
    /// from `recents` and the removable media of the `system` bus.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        folder: PathBuf,
        places: Vec<Place>,
        types: Option<Arc<MimeDatabase>>,
        recents: PathBuf,
        notices: Notices,
        applications: Box<dyn Fn() -> io::Result<MimeApps>>,
        terminal: Option<Vec<String>>,
        system: Option<Connection>,
    ) -> io::Result<Start> {
        let (media_wake, media_woken) = wake_channel();
        let volumes = system.and_then(|system| {
            Volumes::watch(system, notices.clone(), media_wake)
                .map_err(|e| eprintln!("cupertino-files: no removable media: {e}"))
                .ok()
        });
        let (browser, woken) = open_browser(folder, types.clone(), recents.clone())?;
        Ok(Start {
            browser,
            woken,
            places,
            notices,
            applications,
            terminal,
            types,
            recents,
            volumes,
            media_woken,
            trash: Trash::new(),
        })
    }

    /// The windows moving items to `trash` and putting them back from it, in place of the session's trash.
    pub fn with_trash(self, trash: Trash) -> Start {
        Start { trash, ..self }
    }
}

/// What a model calls when it has changes waiting, and what wakes then.
fn wake_channel() -> (Arc<dyn Fn() + Send + Sync>, async_channel::Receiver<()>) {
    let (wake, woken) = async_channel::bounded(1);
    let wake = Arc::new(move || {
        // a wake already waiting covers this change too
        let _ = wake.try_send(());
    });
    (wake, woken)
}

/// The browser of `folder`, and what wakes when its model has changes waiting.
fn open_browser(
    folder: PathBuf,
    types: Option<Arc<MimeDatabase>>,
    recents: PathBuf,
) -> io::Result<(Browser, async_channel::Receiver<()>)> {
    let (wake, woken) = wake_channel();
    let browser = Browser::open(folder, SortSpec::default(), recents, types, wake)?;
    Ok((browser, woken))
}

thread_local! {
    static START: RefCell<Option<Start>> = const { RefCell::new(None) };
}

/// Hand `start` to the next [`Files`] that starts on this thread.
pub fn prepare(start: Start) {
    START.with(|s| *s.borrow_mut() = Some(start));
}

/// The application of the browser windows. [`prepare`] gives the first its folder.
pub struct Files;

pub struct Model {
    /// The open windows, by the id their messages carry.
    windows: BTreeMap<u64, BrowserWindow>,
    next_window: u64,
    session: Session,
}

/// What every window shares.
struct Session {
    places: Vec<Place>,
    /// The places before the removable media's.
    standard_places: usize,
    notices: Notices,
    applications: Box<dyn Fn() -> io::Result<MimeApps>>,
    terminal: Option<Vec<String>>,
    types: Option<Arc<MimeDatabase>>,
    recents: PathBuf,
    volumes: Option<Volumes>,
    media_woken: async_channel::Receiver<()>,
    media: Media,
    /// Where a window on a file system that goes away goes.
    home: PathBuf,
    trash: Trash,
}

/// One browser window.
struct BrowserWindow {
    browser: Browser,
    woken: async_channel::Receiver<()>,
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
    /// What the window last asked the compositor for.
    full_screen: bool,
    /// How the folder is shown.
    view: ViewMode,
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
    /// The removable media have reports waiting.
    Media,
    /// An event of the window with this id.
    Window(u64, Event),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
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
    Close,
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

fn woken(receiver: &async_channel::Receiver<()>, message: Message) -> Command<Message> {
    let receiver = receiver.clone();
    Command::Perform(Box::pin(async move {
        let _ = receiver.recv().await;
        message
    }))
}

fn window_key(window: u64) -> WindowKey {
    WindowKey(Key::from(format!("browser/{window}")))
}

impl Application for Files {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let start = START
            .with(|s| s.borrow_mut().take())
            .expect("kit::prepare runs before Files starts");
        let mut model = Model {
            windows: BTreeMap::new(),
            next_window: 0,
            session: Session {
                standard_places: start.places.len(),
                places: start.places,
                notices: start.notices,
                applications: start.applications,
                terminal: start.terminal,
                types: start.types,
                recents: start.recents,
                volumes: start.volumes,
                media_woken: start.media_woken,
                media: Media::default(),
                home: std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
                trash: start.trash,
            },
        };
        let mut commands = vec![model.add_window(start.browser, start.woken)];
        if model.session.volumes.is_some() {
            commands.push(woken(&model.session.media_woken, Message::Media));
        }
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Media => model.follow_media(),
            Message::Window(id, event) => model.window_event(id, event),
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .windows
            .iter()
            .map(|(&id, window)| view::window(&model.session, id, window))
            .collect()
    }
}

enum Step {
    Back,
    Forward,
    Parent,
}

impl Model {
    /// Show `browser` in a window of its own.
    fn add_window(
        &mut self,
        browser: Browser,
        woken: async_channel::Receiver<()>,
    ) -> Command<Message> {
        let id = self.next_window;
        self.next_window += 1;
        let wake = self::woken(&woken, Message::Window(id, Event::Woken));
        let mut window = BrowserWindow {
            browser,
            woken,
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
            full_screen: false,
            view: ViewMode::List,
        };
        window.entered();
        self.windows.insert(id, window);
        wake
    }

    /// Open a window on each of `folders`.
    fn open_windows(&mut self, folders: Vec<PathBuf>) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        for folder in folders {
            let session = &self.session;
            match open_browser(
                folder.clone(),
                session.types.clone(),
                session.recents.clone(),
            ) {
                Ok((browser, woken)) => commands.push(self.add_window(browser, woken)),
                Err(e) => session.failed(Action::Open, &[&folder], &e),
            }
        }
        commands
    }

    fn window_event(&mut self, id: u64, event: Event) -> Vec<Command<Message>> {
        let session = &self.session;
        // a window's last wake can come after it closed
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        match event {
            Event::Woken => {
                let polled = window.browser.poll();
                for error in polled.errors {
                    eprintln!("cupertino-files: {error}");
                }
                if polled.changed {
                    window.now = SystemTime::now();
                }
                let mut commands = window.listed(id);
                commands.push(woken(&window.woken, Message::Window(id, Event::Woken)));
                return commands;
            }
            Event::Place(index) => {
                if let Some(folder) = session.places.get(index).and_then(Place::folder) {
                    window.go(folder.to_path_buf(), session);
                }
            }
            Event::Navigate(Navigation::Back) => window.step(Step::Back, session),
            Event::Navigate(Navigation::Forward) => window.step(Step::Forward, session),
            Event::Select(selection) => {
                let indices = selection.ranges().iter().flat_map(Clone::clone);
                window
                    .browser
                    .select(indices, selection.anchor(), selection.cursor());
                let renamed = window.rename_index();
                if renamed.is_some() && window.browser.selection().indices().ne(renamed) {
                    window.end_rename(session);
                }
            }
            Event::Open(index) => {
                if !window.browser.selection().contains(index) {
                    window.browser.select([index], Some(index), Some(index));
                }
                window.open_selection(session);
            }
            Event::Sort((column, order)) => {
                if let Some(column) = COLUMNS.get(column) {
                    window
                        .browser
                        .set_sort(column.sort_key(), order == SortOrder::Ascending);
                }
            }
            Event::Search(change) => {
                window.browser.set_filter(&change.text);
                window.search = change.text;
                window.search_revision = change.revision;
            }
            Event::Menu(name) => {
                if let Some(action) = MenuAction::from_name(&name) {
                    return self.menu(id, action);
                }
            }
            Event::NameChanged(change) => {
                if let Some(rename) = window.rename.as_mut() {
                    rename.edit.field_mut().set_text(&change.text);
                    rename.revision = change.revision;
                }
            }
            Event::NameSubmitted => window.end_rename(session),
            Event::NameCancelled => window.rename = None,
            Event::FocusName => {
                if window.rename.is_some() {
                    return vec![Command::Focus(view::name_field_key(id))];
                }
            }
            Event::Scroll(scroll) => window.scroll = scroll,
            Event::SidebarScroll(scroll) => window.sidebar_scroll = scroll,
            Event::Split(split) => window.split = split,
            Event::Close => {
                self.windows.remove(&id);
            }
        }
        Vec::new()
    }

    /// Do `action` of the menus of the window `id`.
    fn menu(&mut self, id: u64, action: MenuAction) -> Vec<Command<Message>> {
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        let session = &self.session;
        match action {
            MenuAction::NewWindow => {
                let folder = window.browser.dir().to_path_buf();
                return self.open_windows(vec![folder]);
            }
            MenuAction::OpenInNewWindow => {
                let browser = &window.browser;
                let folders = browser
                    .selection()
                    .indices()
                    .filter_map(|i| browser.entries().get(i))
                    .filter(|e| e.is_dir)
                    .map(|e| e.path.clone())
                    .collect();
                return self.open_windows(folders);
            }
            MenuAction::CloseWindow => {
                self.windows.remove(&id);
            }
            MenuAction::FullScreen => {
                window.full_screen = !window.full_screen;
                return vec![Command::Window(
                    window_key(id),
                    WindowRequest::Fullscreen(window.full_screen),
                )];
            }
            MenuAction::Eject => {
                if let Some(place) = removable_holding(window.browser.dir(), &session.places) {
                    self.eject(place);
                }
            }
            MenuAction::Open => window.open_selection(session),
            MenuAction::SelectAll => window.browser.select_all(),
            MenuAction::SortBy(key, ascending) => window.browser.set_sort(key, ascending),
            MenuAction::TogglePathBar => window.path_bar = !window.path_bar,
            MenuAction::ToggleStatusBar => window.status_bar = !window.status_bar,
            MenuAction::Back => window.step(Step::Back, session),
            MenuAction::Forward => window.step(Step::Forward, session),
            MenuAction::EnclosingFolder => window.step(Step::Parent, session),
            MenuAction::MoveToTrash => window.trash_selection(session),
            MenuAction::PutBack => window.put_back(session),
            MenuAction::NewFolder => match operations::new_folder(window.browser.dir()) {
                Ok(path) => window.rename_new_folder(path),
                Err(e) => session.failed(Action::NewFolder, &[], &e),
            },
            MenuAction::NewFolderWithSelection => {
                let paths = window.selected_paths();
                match operations::new_folder_with(window.browser.dir(), &paths) {
                    Ok(path) => window.rename_new_folder(path),
                    Err(e) => {
                        let items: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
                        session.failed(Action::MoveIntoNewFolder, &items, &e);
                    }
                }
            }
            MenuAction::Rename => {
                let first = window.browser.selection().indices().next();
                if let Some(index) = first {
                    return window.begin_rename(id, index);
                }
            }
            MenuAction::Duplicate => {
                let paths = window.selected_paths();
                let duplicated = operations::duplicate(&paths);
                session.failed_some(Action::Duplicate, paths.iter().zip(duplicated));
            }
            MenuAction::MakeAlias => {
                let paths = window.selected_paths();
                let made = paths.iter().map(|p| (p, operations::make_alias(p)));
                session.failed_some(Action::MakeAlias, made);
            }
            MenuAction::ShowOriginal => window.show_original(session),
            MenuAction::View(view) if view != window.view => {
                window.view = view;
                window.scroll.offset = cupertino_kit::geometry::Point::ZERO;
            }
            MenuAction::View(_) => {}
        }
        Vec::new()
    }

    /// Take what the removable media reported: the rows follow the devices, a window on a file system that went away
    /// goes home, and a finished mount goes to its folder in the first window.
    fn follow_media(&mut self) -> Vec<Command<Message>> {
        let session = &mut self.session;
        let Some(volumes) = &session.volumes else {
            return Vec::new();
        };
        let reports = volumes.poll();
        if !reports.is_empty() {
            let held: Vec<bool> = self
                .windows
                .values()
                .map(|w| removable_holding(w.browser.dir(), &session.places).is_some())
                .collect();
            let mut mounted = reports
                .into_iter()
                .filter_map(|r| session.media.take(r))
                .last();
            session.show_media();
            for (window, held) in self.windows.values_mut().zip(held) {
                if let Some(at) = mounted.take() {
                    window.go(at, session);
                } else if held && !session.media.holds(window.browser.dir()) {
                    window.go(session.home.clone(), session);
                }
            }
        }
        vec![woken(&session.media_woken, Message::Media)]
    }

    /// Eject the drive of the removable file system of place `index`: the windows on its file systems go home first,
    /// so they hold none of them.
    fn eject(&mut self, index: usize) {
        let session = &mut self.session;
        let Some((device, filesystems)) = index
            .checked_sub(session.standard_places)
            .and_then(|i| session.media.eject(i))
        else {
            return;
        };
        let mounts: Vec<&Path> = filesystems
            .iter()
            .filter_map(|d| d.mount.as_deref())
            .collect();
        for window in self.windows.values_mut() {
            if mounts.iter().any(|m| window.browser.dir().starts_with(m)) {
                window.go(session.home.clone(), session);
            }
        }
        if let Some(volumes) = &session.volumes {
            volumes.eject(&device, filesystems);
        }
        session.show_media();
    }
}

impl Session {
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

    fn show_media(&mut self) {
        self.places.truncate(self.standard_places);
        self.places.extend(self.media.places());
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
}

impl BrowserWindow {
    fn go(&mut self, to: PathBuf, session: &Session) {
        match self.browser.go(to.clone()) {
            Ok(()) => self.entered(),
            Err(e) => session.failed(Action::Open, &[&to], &e),
        }
    }

    fn step(&mut self, step: Step, session: &Session) {
        let target = match step {
            Step::Back => self.browser.history().back_target(),
            Step::Forward => self.browser.history().forward_target(),
            Step::Parent => self.browser.dir().parent(),
        }
        .map(|p| crate::places::window_title(p, &session.places));
        let outcome = match step {
            Step::Back => self.browser.back(),
            Step::Forward => self.browser.forward(),
            Step::Parent => self.browser.parent(),
        };
        match outcome {
            Ok(true) => self.entered(),
            Ok(false) => {}
            Err(e) => session
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

    fn selected_paths(&self) -> Vec<PathBuf> {
        let entries = self.browser.entries();
        self.browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
            .map(|e| e.path.clone())
            .collect()
    }

    /// Go to the folder of the selected link's target and select the target.
    fn show_original(&mut self, session: &Session) {
        let Some(link) = self.selected_paths().into_iter().next() else {
            return;
        };
        let target = match std::fs::canonicalize(&link) {
            Ok(target) => target,
            Err(e) => return session.failed(Action::ShowOriginal, &[&link], &e),
        };
        let Some(parent) = target.parent().map(Path::to_path_buf) else {
            return;
        };
        self.go(parent.clone(), session);
        if self.browser.dir() == parent {
            self.browser.select_when_appears(target);
        }
    }

    /// Move the selected items to the trash; the listing drops them when the watcher reports it.
    fn trash_selection(&self, session: &Session) {
        let paths = self.selected_paths();
        session.failed_some(
            Action::Trash,
            paths.iter().map(|path| (path, session.trash.trash(path))),
        );
    }

    /// Put the selected items of the trash back where they came from.
    fn put_back(&self, session: &Session) {
        let paths = self.selected_paths();
        let listed = match session.trash.list() {
            Ok(listed) => listed,
            Err(e) => {
                let items: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
                return session.failed(Action::PutBack, &items, &e);
            }
        };
        let restored = paths.iter().map(|path| {
            let restored = match listed.iter().find(|item| item.file_path() == *path) {
                Some(item) => session.trash.restore(item).map_err(|e| e.to_string()),
                None => Err("The Trash has no record of where it came from".to_string()),
            };
            (path, restored)
        });
        session.failed_some(Action::PutBack, restored);
    }

    /// Go into the selected folder, or open the selected files with their default applications.
    fn open_selection(&mut self, session: &Session) {
        let Activation { folder, files } = self.browser.activation();
        if !files.is_empty() {
            match (session.applications)() {
                Ok(apps) => {
                    for path in files {
                        session.launch(&apps, &path);
                    }
                }
                Err(e) => {
                    let items: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
                    session.failed(Action::Open, &items, &e);
                }
            }
        }
        if let Some(folder) = folder {
            self.go(folder, session);
        }
    }

    /// A folder was made: the list selects it when it shows it, and its name is edited then.
    fn rename_new_folder(&mut self, path: PathBuf) {
        self.browser.select_when_appears(path.clone());
        self.rename_when_listed = Some(path);
    }

    /// Edit the name of item `index` in its row, if it is the only selected item. The field of the window `id` is
    /// focused once the window shows it.
    fn begin_rename(&mut self, id: u64, index: usize) -> Vec<Command<Message>> {
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
        vec![Command::Perform(Box::pin(async move {
            Message::Window(id, Event::FocusName)
        }))]
    }

    /// The row of the name being edited.
    fn rename_index(&self) -> Option<usize> {
        self.rename.as_ref().map(|r| r.edit.index())
    }

    /// Commit the edited name. A name that is not valid is reported and the field stays open. A rename that changes
    /// the extension is not confirmed (there is no dialog to ask in).
    fn end_rename(&mut self, session: &Session) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        match rename.edit.commit() {
            Ok(None) => {}
            Ok(Some(name)) => match operations::rename_item(&rename.path, &name) {
                Ok(path) => self.browser.select_when_appears(path),
                Err(e) => session.failed(Action::Rename, &[&rename.path], &e),
            },
            Err(e) => {
                session
                    .notices
                    .post(notice::bad_name(rename.edit.text(), &e));
                self.rename = Some(rename);
            }
        }
    }

    /// The list of the window `id` changed: the name being edited follows its item, and a folder just made starts
    /// its rename.
    fn listed(&mut self, id: u64) -> Vec<Command<Message>> {
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
                self.begin_rename(id, index)
            }
            None => Vec::new(),
        }
    }

    fn menu_state(&self, session: &Session) -> MenuState {
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
            in_trash: self.browser.dir() == session.trash.home_trash().join("files"),
            path_bar: self.path_bar,
            status_bar: self.status_bar,
            full_screen: self.full_screen,
            can_go_back: history.back_target().is_some(),
            can_go_forward: history.forward_target().is_some(),
            has_parent: !self.browser.is_virtual() && self.browser.dir().parent().is_some(),
            eject: removable_holding(self.browser.dir(), &session.places)
                .filter(|&i| session.places[i].ejectable())
                .map(|i| session.places[i].name.clone()),
        }
    }

    /// The actions the menus show but the window cannot do now.
    fn disabled(state: &MenuState) -> BTreeSet<ActionName> {
        menu_bar::enabled(state)
            .into_iter()
            .filter(|(_, enabled)| !enabled)
            .map(|(action, _)| action.name())
            .collect()
    }
}

/// Open the window on `start` and serve it until the last window closes.
pub fn run(start: Start) -> Result<(), cupertino_kit::app::AppError> {
    prepare(start);
    cupertino_kit::app::run::<Files>()
}
