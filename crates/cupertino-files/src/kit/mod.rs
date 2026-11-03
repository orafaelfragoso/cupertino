//! The browser window on CupertinoKit: the sidebar's places, the folder as a list, the path and status bars and the
//! menus of the desktop's menu bar, over the toolkit-free [`Browser`]. It takes the place of the window in
//! `main.rs` once it does all that window does.

mod view;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cupertino_apps::{launch, MimeApps, MimeDatabase};
use cupertino_kit::controls::{Navigation, Selection, SortOrder, SplitLayout, TextChange};
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::WindowRequest;
use cupertino_kit::prelude::*;
use cupertino_menu_model::ActionName;

use crate::browser::{Activation, Browser};
use crate::menu_bar::{self, MenuAction, MenuState};
use crate::notice::{self, Action, Notices};
use crate::open::{open_command, OpenError};
use crate::places::Place;
use crate::settings::ColumnId;
use crate::trash::Trash;
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
        })
    }
}

/// The browser of `folder`, and what wakes when its model has changes waiting.
fn open_browser(
    folder: PathBuf,
    types: Option<Arc<MimeDatabase>>,
    recents: PathBuf,
) -> io::Result<(Browser, async_channel::Receiver<()>)> {
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
    notices: Notices,
    applications: Box<dyn Fn() -> io::Result<MimeApps>>,
    terminal: Option<Vec<String>>,
    types: Option<Arc<MimeDatabase>>,
    recents: PathBuf,
    trash_files: PathBuf,
}

/// One browser window.
pub struct BrowserWindow {
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
    /// What the window last asked the compositor for.
    full_screen: bool,
}

/// An [`Event`] of the window `window`.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    window: u64,
    event: Event,
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

/// Menu items the window does not do yet; they are shown disabled.
const NOT_YET: [MenuAction; 10] = [
    MenuAction::View(ViewMode::Icons),
    MenuAction::NewFolder,
    MenuAction::NewFolderWithSelection,
    MenuAction::MoveToTrash,
    MenuAction::PutBack,
    MenuAction::Rename,
    MenuAction::Duplicate,
    MenuAction::MakeAlias,
    MenuAction::ShowOriginal,
    MenuAction::Eject,
];

fn woken(window: u64, receiver: &async_channel::Receiver<()>) -> Command<Message> {
    let receiver = receiver.clone();
    Command::Perform(Box::pin(async move {
        let _ = receiver.recv().await;
        Message {
            window,
            event: Event::Woken,
        }
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
                places: start.places,
                notices: start.notices,
                applications: start.applications,
                terminal: start.terminal,
                types: start.types,
                recents: start.recents,
                trash_files: Trash::new().home_trash().join("files"),
            },
        };
        let commands = vec![model.add_window(start.browser, start.woken)];
        (model, commands)
    }

    fn update(model: &mut Model, Message { window: id, event }: Message) -> Vec<Command<Message>> {
        let session = &model.session;
        // a window's last wake can come after it closed
        let Some(window) = model.windows.get_mut(&id) else {
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
                return vec![woken(id, &window.woken)];
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
                    return model.menu(id, action);
                }
            }
            Event::Scroll(scroll) => window.scroll = scroll,
            Event::SidebarScroll(scroll) => window.sidebar_scroll = scroll,
            Event::Split(split) => window.split = split,
            Event::Close => {
                model.windows.remove(&id);
            }
        }
        Vec::new()
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
        let wake = self::woken(id, &woken);
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
            full_screen: false,
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
            MenuAction::Open => window.open_selection(session),
            MenuAction::SelectAll => window.browser.select_all(),
            MenuAction::SortBy(key, ascending) => window.browser.set_sort(key, ascending),
            MenuAction::TogglePathBar => window.path_bar = !window.path_bar,
            MenuAction::ToggleStatusBar => window.status_bar = !window.status_bar,
            MenuAction::Back => window.step(Step::Back, session),
            MenuAction::Forward => window.step(Step::Forward, session),
            MenuAction::EnclosingFolder => window.step(Step::Parent, session),
            MenuAction::View(_)
            | MenuAction::NewFolder
            | MenuAction::NewFolderWithSelection
            | MenuAction::MoveToTrash
            | MenuAction::PutBack
            | MenuAction::Rename
            | MenuAction::Duplicate
            | MenuAction::MakeAlias
            | MenuAction::ShowOriginal
            | MenuAction::Eject => {}
        }
        Vec::new()
    }
}

impl Session {
    fn failed(&self, action: Action, items: &[&Path], error: &dyn std::fmt::Display) {
        self.notices.post(notice::failure(action, items, error));
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
            in_trash: self.browser.dir() == session.trash_files,
            path_bar: self.path_bar,
            status_bar: self.status_bar,
            full_screen: self.full_screen,
            can_go_back: history.back_target().is_some(),
            can_go_forward: history.forward_target().is_some(),
            has_parent: !self.browser.is_virtual() && self.browser.dir().parent().is_some(),
            eject: None,
        }
    }

    /// The actions the menus show but the window cannot do now.
    fn disabled(state: &MenuState) -> BTreeSet<ActionName> {
        menu_bar::enabled(state)
            .into_iter()
            .filter(|(_, enabled)| !enabled)
            .map(|(action, _)| action)
            .chain(NOT_YET)
            .map(|action| action.name())
            .collect()
    }
}

/// Open the window on `start` and serve it until the last window closes.
pub fn run(start: Start) -> Result<(), cupertino_kit::app::AppError> {
    prepare(start);
    cupertino_kit::app::run::<Files>()
}
