//! The browser window on CupertinoKit: the sidebar's places, the folder as a list or a grid of icons, the path and
//! status bars and the menus of the desktop's menu bar, over the toolkit-free [`Browser`]. It is the window `main.rs`
//! runs.

mod announce;
mod band;
mod clicks;
mod context;
mod dragged;
mod drops;
mod grid;
mod item_icons;
mod list;
mod places;
mod search;
mod sidebar;
mod toolbar;
mod transfer;
mod view;

pub use dragged::Grab;
pub use drops::Destination;
pub use list::RowGrab;
pub use places::PlacesSource;
pub use transfer::Observer;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cupertino_apps::{launch, MimeApps, MimeDatabase};
use cupertino_kit::controls::{
    MenuAnchor, Navigation, Selection, SortOrder, SplitLayout, TextChange,
};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::{DragProgress, WindowRequest};
use cupertino_kit::prelude::*;
use cupertino_menu_model::ActionName;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

use crate::browser::{Activation, Browser};
use crate::context_menu::{common_apps, AppChoice, Command as MenuCommand};
use crate::menu_bar::{self, MenuAction, MenuState};
use crate::menu_spec::{self, Background, Subject};
use crate::name_edit::NameEdit;
use crate::notice::{self, Action, Notices};
use crate::open::{command_with, default_app_id, open_command, open_with, OpenError};
use crate::operations::{self, DropAction};
use crate::places::{removable_holding, Place};
use crate::search_field::{self, SearchWidth};
use crate::settings::ColumnId;
use crate::tags::{self, TagColor};
use crate::thumbnail::Thumbnailer;
use crate::trash::Trash;
use crate::udisks::{Media, Passphrase, Report, Volumes};
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
    view: ViewMode,
    home: Option<PathBuf>,
    observer: Option<Observer>,
    places_source: Option<PlacesSource>,
    /// Folders whose changes may change the places, besides the trash.
    places_folders: Vec<PathBuf>,
    file_icons: Option<FileIcons>,
    thumbnails: Option<Thumbnailer>,
    /// What the list words its dates relative to.
    clock: fn() -> SystemTime,
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
            view: ViewMode::Icons,
            home: None,
            observer: None,
            places_source: None,
            places_folders: Vec::new(),
            file_icons: None,
            thumbnails: None,
            clock: SystemTime::now,
        })
    }

    /// The windows moving items to `trash` and putting them back from it, in place of the session's trash.
    pub fn with_trash(self, trash: Trash) -> Start {
        Start { trash, ..self }
    }

    /// Windows on a file system that goes away going to `home` in place of `$HOME`.
    pub fn with_home(self, home: PathBuf) -> Start {
        Start {
            home: Some(home),
            ..self
        }
    }

    /// The windows opening in `view` in place of the icon view.
    pub fn with_view(self, view: ViewMode) -> Start {
        Start { view, ..self }
    }

    /// The sidebar's standard places read again from `source` whenever one of `folders` changes, besides the trash's
    /// place, which follows the trash.
    pub fn with_places_source(self, source: PlacesSource, folders: Vec<PathBuf>) -> Start {
        Start {
            places_source: Some(source),
            places_folders: folders,
            ..self
        }
    }

    /// Every paste and move calling `observer` with its progress on its own thread, before it looks for a stop.
    pub fn with_transfer_observer(self, observer: Observer) -> Start {
        Start {
            observer: Some(observer),
            ..self
        }
    }

    /// The items drawn with the icons of `icons` in place of the built-in placeholders.
    pub fn with_file_icons(self, icons: FileIcons) -> Start {
        Start {
            file_icons: Some(icons),
            ..self
        }
    }

    /// Pictures drawn with their thumbnails from `cache`, made there when it lacks them. Without it, every item shows
    /// its type's icon.
    pub fn with_thumbnails(self, cache: Thumbnailer) -> Start {
        Start {
            thumbnails: Some(cache),
            ..self
        }
    }

    /// The list wording its dates ("Today at 09:41") relative to the time `clock` gives, in place of the system's.
    pub fn with_clock(self, clock: fn() -> SystemTime) -> Start {
        Start { clock, ..self }
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
    /// The window that asked for each mount or unlock under way, by the device's block.
    mounting: HashMap<OwnedObjectPath, u64>,
    /// Where a window on a file system that goes away goes.
    home: PathBuf,
    trash: Trash,
    /// How a new window shows its folder.
    view: ViewMode,
    /// What Copy took last, for Paste Item and Move Item Here in any window.
    pasteboard: Vec<PathBuf>,
    /// What each transfer calls with its progress.
    observer: Option<Observer>,
    next_transfer: u64,
    places_source: Option<PlacesSource>,
    places_watch: places::PlacesWatch,
    /// The items' icons and thumbnails.
    icons: item_icons::ItemIcons,
    /// What wakes when a thumbnail is made.
    thumbnails_woken: async_channel::Receiver<()>,
    /// The items a drag out of a window carries, until it ends.
    dragged: Vec<PathBuf>,
    /// What the list words its dates relative to.
    clock: fn() -> SystemTime,
}

/// One browser window.
struct BrowserWindow {
    browser: Browser,
    woken: async_channel::Receiver<()>,
    path_bar: bool,
    status_bar: bool,
    /// The free space of the folder's file system, for the status bar.
    available: Option<u64>,
    /// Whether the folder's file system is mounted read-only, read with the free space.
    mounted_read_only: bool,
    search: String,
    search_revision: u64,
    /// The search field has the keyboard.
    search_focused: bool,
    search_width: SearchWidth,
    scroll: ScrollState,
    sidebar_scroll: ScrollState,
    split: SplitLayout,
    /// When the dates of the list were last worded.
    now: SystemTime,
    /// What gives `now`.
    clock: fn() -> SystemTime,
    /// The name being edited in place.
    rename: Option<Rename>,
    /// A folder just made, whose name is edited once the list shows it.
    rename_when_listed: Option<PathBuf>,
    /// What the window last asked the compositor for.
    full_screen: bool,
    /// The passphrase prompt of a locked container.
    unlock: Option<Unlock>,
    /// How the folder is shown.
    view: ViewMode,
    /// The context menu open in the window.
    context: Option<context::OpenMenu>,
    /// What the latest secondary press was on, until its context menu opens: an item, or empty space.
    pressed: Option<Option<usize>>,
    /// The pastes and moves into a folder of this window still running, oldest first.
    transfers: Vec<transfer::Transfer>,
    /// The keyboard is in the sidebar.
    sidebar_focused: bool,
    /// The key of the sidebar row a click or the arrows last chose since the window last went to another folder: the
    /// row the sidebar's keyboard is on, when it is not the selected one.
    sidebar_row: Option<Key>,
    /// The rubber band being drawn over the items, in the view's content.
    band: Option<cupertino_kit::geometry::Rect>,
    /// The presses on the list's rows, which its rows and its frame share.
    list_presses: list::SharedPresses,
    /// What the window's live region reads last ([`announce`]).
    announcement: announce::Announcement,
    /// The keyboard is on the folder's items.
    items_focused: bool,
    /// An item just renamed, whose row or icon takes the keyboard once the view shows it.
    focus_when_listed: Option<PathBuf>,
}

/// The sheet that asks for a locked container's passphrase.
struct Unlock {
    object: OwnedObjectPath,
    name: String,
    passphrase: Passphrase,
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
    /// A folder the sidebar's places come from changed.
    Places,
    /// A thumbnail was made.
    Thumbnails,
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
    /// Escape outside a rename while the search has text.
    SearchCleared,
    /// The search field took the keyboard or lost it at `now` seconds; its width animates unless `animate` is false
    /// (reduce motion).
    SearchFocus {
        focused: bool,
        now: f64,
        animate: bool,
    },
    /// A frame of the search field's width change at `now` seconds.
    SearchTick {
        now: f64,
        animate: bool,
    },
    Menu(ActionName),
    /// The activation token for opening `path`, with the application of desktop entry `with` or else its default,
    /// arrived; `None` when the compositor gave none.
    Launch {
        path: PathBuf,
        with: Option<String>,
        token: Option<String>,
    },
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
    /// A press on the accessory of the sidebar row with this key: a device's eject button.
    Accessory(Key),
    /// The passphrase typed in the unlock sheet.
    Passphrase(Passphrase),
    /// Unlock with the passphrase typed: the sheet's default button, or Return in its field.
    Unlock,
    UnlockCancelled,
    /// A secondary press on the item with this index, or on empty space.
    SecondaryPress(Option<usize>),
    /// Open the context menu at this point of the window.
    ContextMenu(cupertino_kit::geometry::Point),
    /// The context menu's open submenus changed.
    MenuOpened(Vec<usize>),
    MenuClosed,
    /// A row of the context menu was chosen.
    MenuCommand(MenuCommand),
    Close,
    /// A press on items of the icon grid moved far enough to drag them out of the window.
    DragOut(Grab),
    /// How a drag out of the window goes. The target copies or moves the files, so the window has nothing to do
    /// but forget them once it ends.
    Dragged(DragProgress),
    /// Files were dropped on the window, to go `to` (`None`: where nothing takes them), with the action agreed with
    /// their source and the modifiers held. The source is told what was done with them, by `token`.
    Dropped {
        token: cupertino_kit::platform::DropToken,
        to: Option<Destination>,
        paths: Vec<PathBuf>,
        action: cupertino_kit::platform::DropAction,
        mods: cupertino_kit::platform::Mods,
    },
    /// The transfer with this id has progress or its results waiting.
    Transferred(u64),
    /// Stop the transfer with this id: its Stop button.
    StopTransfer(u64),
    /// The keyboard came into the sidebar, or left it.
    SidebarFocus(bool),
    /// The keyboard came onto the folder's items, or left them.
    ItemsFocus(bool),
    /// The rubber band being drawn over the items, in the view's content; `None` once it is let go.
    Band(Option<cupertino_kit::geometry::Rect>),
    /// A press on the name of the only selected item was not followed by a second one: edit the name of item `index`.
    RenameItem(usize),
    /// Open or close a folder's row of the list: its triangle, Left or Right.
    Toggle(cupertino_kit::controls::TreeToggle),
    /// A press on rows of the list moved far enough to drag them out of the window.
    DragRows(RowGrab),
    /// Tab chose row `index` of the list: give it the keyboard once the list shows it.
    FocusRow(usize),
    /// The list shows the row with this key: give it the keyboard.
    FocusKey(Key),
    /// A toolbar button's menu: open it below the button.
    Popup(Popup),
    /// A folder of the path bar was clicked: go to it.
    PathSegment(PathBuf),
}

/// The toolbar's pop-up buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popup {
    /// How the folder is shown.
    View,
    /// The selection's tag colours.
    Tags,
    /// The selection's menu, or the folder's without a selection.
    Action,
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
        let places_watch =
            places::PlacesWatch::start(start.places_folders, start.trash.home_trash());
        let (thumbnails_wake, thumbnails_woken) = wake_channel();
        let file_icons = start.file_icons.unwrap_or_else(|| {
            let types = start.types.as_deref().cloned().unwrap_or_default();
            FileIcons::builtin(types, HashMap::new())
        });
        let icons = item_icons::ItemIcons::new(file_icons, start.thumbnails, thumbnails_wake);
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
                mounting: HashMap::new(),
                home: start.home.unwrap_or_else(|| {
                    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
                }),
                trash: start.trash,
                view: start.view,
                pasteboard: Vec::new(),
                observer: start.observer,
                next_transfer: 0,
                places_watch,
                places_source: start.places_source,
                icons,
                thumbnails_woken,
                dragged: Vec::new(),
                clock: start.clock,
            },
        };
        let mut commands = vec![
            model.add_window(start.browser, start.woken),
            woken(&model.session.places_watch.woken, Message::Places),
        ];
        if model.session.volumes.is_some() {
            commands.push(woken(&model.session.media_woken, Message::Media));
        }
        if model.session.icons.thumbnails() {
            commands.push(woken(&model.session.thumbnails_woken, Message::Thumbnails));
        }
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Media => model.follow_media(),
            Message::Places => {
                model.session.follow_places();
                vec![woken(&model.session.places_watch.woken, Message::Places)]
            }
            // the windows draw the thumbnails made
            Message::Thumbnails => {
                vec![woken(&model.session.thumbnails_woken, Message::Thumbnails)]
            }
            Message::Window(id, event) => {
                let before = model.windows.get(&id).map(BrowserWindow::tally);
                let commands = model.window_event(id, event);
                if let (Some(before), Some(window)) = (before, model.windows.get_mut(&id)) {
                    let after = window.tally();
                    window.announcement.follow(before, after);
                }
                model.want_thumbnails(id);
                commands
            }
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model
            .windows
            .iter()
            .flat_map(|(&id, window)| {
                let menu = window
                    .context
                    .as_ref()
                    .map(|menu| context::windows(id, menu));
                std::iter::once(view::window(&model.session, id, window))
                    .chain(menu.into_iter().flatten())
            })
            .collect()
    }
}

enum Step {
    Back,
    Forward,
    Parent,
}

impl Model {
    /// Ask for the thumbnails of the items window `id` shows, and for none once it is closed.
    fn want_thumbnails(&self, id: u64) {
        let icons = &self.session.icons;
        match self.windows.get(&id) {
            Some(window) => {
                let (shown, points) = view::shown(window);
                let entries = window.browser.entries();
                icons.want(id, shown.into_iter().filter_map(|i| entries.get(i)), points);
            }
            None => icons.forget(id),
        }
    }

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
            mounted_read_only: false,
            search: String::new(),
            search_revision: 0,
            search_focused: false,
            search_width: SearchWidth::default(),
            scroll: NO_SCROLL,
            sidebar_scroll: NO_SCROLL,
            split: SplitLayout {
                position: view::SIDEBAR_WIDTH,
                collapsed: None,
            },
            now: (self.session.clock)(),
            clock: self.session.clock,
            rename: None,
            rename_when_listed: None,
            full_screen: false,
            unlock: None,
            view: self.session.view,
            context: None,
            pressed: None,
            transfers: Vec::new(),
            sidebar_focused: false,
            sidebar_row: None,
            band: None,
            list_presses: Default::default(),
            announcement: announce::Announcement::default(),
            items_focused: false,
            focus_when_listed: None,
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

    /// Drag `grab`'s items out of window `id`, remembering them until the drag ends so the windows know their own items.
    fn drag_out(&mut self, id: u64, grab: &Grab) -> Vec<Command<Message>> {
        let Some(window) = self.windows.get(&id) else {
            return Vec::new();
        };
        let entries = window.browser.entries();
        self.session.dragged = grab
            .indices
            .iter()
            .filter_map(|&i| Some(entries.get(i)?.path.clone()))
            .collect();
        vec![dragged::start(id, window_key(id), grab, entries)]
    }

    /// The drag of the list's rows of `grab` out of window `id`, as [`Self::drag_out`] for the grid.
    fn drag_rows(&mut self, id: u64, grab: &RowGrab) -> Vec<Command<Message>> {
        let Some(window) = self.windows.get(&id) else {
            return Vec::new();
        };
        let entries = window.browser.entries();
        self.session.dragged = grab
            .indices
            .iter()
            .filter_map(|&i| Some(entries.get(i)?.path.clone()))
            .collect();
        let icon = grab
            .indices
            .first()
            .and_then(|&i| entries.get(i))
            .map(|e| self.session.icons.icon(e, dragged::ROW_ICON))
            .unwrap_or_default();
        vec![dragged::start_rows(id, window_key(id), grab, entries, icon)]
    }

    /// Put `paths` dropped on window `id` where they go: into the trash, or moved or copied into a folder as
    /// [`drops::chosen`] says. A name the folder has is given a free copy name; the failures are reported together,
    /// and the last item to arrive in the folder the window shows is selected. What the source is told was done: a
    /// move only when every item left its place, a copy when every item was copied, and nothing otherwise (an item
    /// that failed, or one moved into the folder already holding it), so the source keeps what it has.
    fn drop_items(
        &mut self,
        id: u64,
        to: &Destination,
        paths: &[PathBuf],
        agreed: cupertino_kit::platform::DropAction,
        mods: cupertino_kit::platform::Mods,
    ) -> Option<cupertino_kit::platform::DropAction> {
        use cupertino_kit::platform::DropAction as Done;
        let session = &self.session;
        if paths.is_empty() {
            return None;
        }
        let dir = match to {
            Destination::Trash => {
                let trashed: Vec<_> = paths.iter().map(|path| session.trash.trash(path)).collect();
                let all = trashed.iter().all(Result::is_ok);
                session.failed_some(Action::Trash, paths.iter().zip(trashed));
                return all.then_some(Done::Move);
            }
            Destination::Folder(dir) => dir,
        };
        let action = match drops::chosen(paths, to, agreed, mods) {
            Ok(action) => action,
            Err(e) => {
                let items: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
                session.failed(Action::Drop(DropAction::Move), &items, &e);
                return None;
            }
        };
        let placed = operations::drop_into(paths, dir, action);
        let arrived = placed.iter().rev().find_map(|p| p.as_ref().ok()).cloned();
        let done = match action {
            DropAction::Copy => placed.iter().all(Result::is_ok).then_some(Done::Copy),
            // a move into the folder holding the item leaves it where it is
            DropAction::Move => paths
                .iter()
                .zip(&placed)
                .all(|(from, to)| to.as_ref().is_ok_and(|to| to != from))
                .then_some(Done::Move),
        };
        session.failed_some(Action::Drop(action), paths.iter().zip(placed));
        if let (Some(window), Some(path)) = (self.windows.get_mut(&id), arrived) {
            if window.browser.dir() == dir.as_path() {
                window.browser.select_when_appears(path);
            }
        }
        done
    }

    fn window_event(&mut self, id: u64, event: Event) -> Vec<Command<Message>> {
        let event = match event {
            Event::Place(index) => return self.open_place(id, index),
            Event::Accessory(key) => return self.accessory(&key),
            Event::Unlock => return self.unlock(id),
            Event::MenuCommand(command) => return self.menu_command(id, command),
            Event::DragOut(grab) => return self.drag_out(id, &grab),
            Event::DragRows(grab) => return self.drag_rows(id, &grab),
            Event::Dragged(progress) => {
                if matches!(progress, DragProgress::Finished | DragProgress::Cancelled) {
                    self.session.dragged.clear();
                }
                return Vec::new();
            }
            Event::Dropped {
                token,
                to,
                paths,
                action,
                mods,
            } => {
                let done = to.and_then(|to| self.drop_items(id, &to, &paths, action, mods));
                return vec![Command::FinishDrop(token, done)];
            }
            event => event,
        };
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
                    window.now = (window.clock)();
                    window.read_available();
                }
                let mut commands = window.listed(id);
                commands.push(woken(&window.woken, Message::Window(id, Event::Woken)));
                return commands;
            }
            Event::Place(_)
            | Event::Accessory(_)
            | Event::Unlock
            | Event::MenuCommand(_)
            | Event::DragOut(_)
            | Event::DragRows(_)
            | Event::Dragged(_)
            | Event::Dropped { .. } => {}
            Event::SecondaryPress(item) => {
                // an item's press comes before the press on what holds it
                if item.is_some() || window.pressed.is_none() {
                    window.pressed = Some(item);
                }
            }
            Event::ContextMenu(at) => window.open_context(at, session),
            Event::MenuOpened(open) => {
                if let Some(menu) = window.context.as_mut() {
                    menu.open = open;
                }
            }
            Event::MenuClosed => window.context = None,
            Event::Passphrase(passphrase) => {
                if let Some(unlock) = window.unlock.as_mut() {
                    unlock.passphrase = passphrase;
                }
            }
            Event::UnlockCancelled => window.unlock = None,
            Event::Navigate(Navigation::Back) => window.step(Step::Back, session),
            Event::Navigate(Navigation::Forward) => window.step(Step::Forward, session),
            Event::Select(selection) => {
                let indices = selection.ranges().iter().flat_map(Clone::clone);
                window
                    .browser
                    .select(indices, selection.anchor(), selection.cursor());
                let renamed = window.rename_index();
                if renamed.is_some() && window.browser.selection().indices().ne(renamed) {
                    // the keyboard goes where the press went, not back to the item renamed
                    let _ = window.end_rename(session);
                }
            }
            Event::Open(index) => {
                if !window.browser.selection().contains(index) {
                    window.browser.select([index], Some(index), Some(index));
                }
                return window.open_selection(id, session);
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
                window.follow_search();
            }
            Event::SearchCleared => window.clear_search(),
            Event::SearchFocus {
                focused,
                now,
                animate,
            } => {
                window.search_focused = focused;
                window.follow_search();
                let toolbar = window.toolbar_width();
                window.search_width.tick(now, toolbar, animate);
            }
            Event::SearchTick { now, animate } => {
                let toolbar = window.toolbar_width();
                window.search_width.tick(now, toolbar, animate);
            }
            Event::Launch { path, with, token } => {
                session.launch(&path, with.as_deref(), token.as_deref())
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
            Event::NameSubmitted => return window.submit_rename(id, session),
            Event::NameCancelled => {
                if let Some(rename) = window.rename.take() {
                    return window.focus_item(id, &rename.path);
                }
            }
            Event::FocusName => {
                if window.rename.is_some() {
                    return vec![Command::Focus(view::name_field_key(id))];
                }
            }
            Event::Scroll(scroll) => {
                window.scroll = scroll;
                let toolbar = window.toolbar_width();
                window.search_width.follow_toolbar(toolbar);
            }
            Event::SidebarScroll(scroll) => window.sidebar_scroll = scroll,
            Event::Split(split) => window.split = split,
            Event::Close => {
                self.windows.remove(&id);
            }
            Event::Transferred(tid) => return window.transferred(id, tid, session),
            Event::SidebarFocus(focused) => window.sidebar_focused = focused,
            Event::ItemsFocus(focused) => window.items_focused = focused,
            Event::Band(band) => window.band = band,
            Event::Toggle(toggle) => {
                if !toggle.expanded {
                    return window.collapse(id, toggle.index);
                }
                if let Err(e) = window.browser.expand(toggle.index) {
                    eprintln!("cupertino-files: cannot open the folder's row: {e}");
                }
            }
            Event::FocusRow(index) => {
                let Some(entry) = window.browser.entries().get(index) else {
                    return Vec::new();
                };
                let key = view::row_key(id, &entry.path);
                return vec![Command::Perform(Box::pin(async move {
                    Message::Window(id, Event::FocusKey(key))
                }))];
            }
            Event::FocusKey(key) => return vec![Command::Focus(key)],
            Event::Popup(popup) => window.open_popup(popup, session),
            Event::PathSegment(path) => {
                if path != window.browser.dir() {
                    window.go(path, session);
                }
            }
            Event::RenameItem(index) => {
                let state = window.menu_state(session);
                if menu_bar::enabled(&state).contains(&(MenuAction::Rename, true)) {
                    return window.begin_rename(id, index);
                }
            }
            Event::StopTransfer(tid) => {
                if let Some(running) = window.transfers.iter().find(|t| t.id == tid) {
                    running.stop();
                }
            }
        }
        Vec::new()
    }

    /// Do `action` of the menus of the window `id`.
    fn menu(&mut self, id: u64, action: MenuAction) -> Vec<Command<Message>> {
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        match action {
            MenuAction::Copy => {
                self.session.pasteboard = window.selected_paths();
                return Vec::new();
            }
            MenuAction::Paste | MenuAction::MoveHere => {
                let moving = action == MenuAction::MoveHere;
                let session = &mut self.session;
                if session.pasteboard.is_empty() {
                    return Vec::new();
                }
                let tid = session.next_transfer;
                session.next_transfer += 1;
                let sources = match moving {
                    true => std::mem::take(&mut session.pasteboard),
                    false => session.pasteboard.clone(),
                };
                let dest = window.browser.dir().to_path_buf();
                let started =
                    transfer::Transfer::start(tid, sources, dest, moving, session.observer.clone());
                let wake = woken(&started.woken, Message::Window(id, Event::Transferred(tid)));
                window
                    .announcement
                    .say(announce::About::Transfer, started.title());
                window.transfers.push(started);
                return vec![wake];
            }
            _ => {}
        }
        let session = &self.session;
        match action {
            MenuAction::Copy | MenuAction::Paste | MenuAction::MoveHere => {}
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
                if let Some(place) = window.eject_target(session) {
                    self.eject(place);
                }
            }
            MenuAction::Open => return window.open_selection(id, session),
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
                // the folders opened in the list close with it
                window.browser.collapse_all();
                window.band = None;
                // a press or a band of the list ends with it: its nodes go without a release
                *window.list_presses.borrow_mut() = Default::default();
                window.view = view;
                window.scroll.offset = cupertino_kit::geometry::Point::ZERO;
            }
            MenuAction::View(_) => {}
        }
        if matches!(action, MenuAction::MoveToTrash | MenuAction::PutBack) {
            // the first item trashed makes the trash: its place comes at once
            self.session.follow_places();
        }
        Vec::new()
    }

    /// Do the row `command` of the context menu of the window `id`.
    fn menu_command(&mut self, id: u64, command: MenuCommand) -> Vec<Command<Message>> {
        let action = match command {
            MenuCommand::Open => MenuAction::Open,
            MenuCommand::OpenInNewWindow => MenuAction::OpenInNewWindow,
            MenuCommand::ShowOriginal => MenuAction::ShowOriginal,
            MenuCommand::PutBack => MenuAction::PutBack,
            MenuCommand::MoveToTrash => MenuAction::MoveToTrash,
            MenuCommand::Rename => MenuAction::Rename,
            MenuCommand::NewFolder => MenuAction::NewFolder,
            MenuCommand::NewFolderWithSelection => MenuAction::NewFolderWithSelection,
            MenuCommand::Duplicate => MenuAction::Duplicate,
            MenuCommand::MakeAlias => MenuAction::MakeAlias,
            MenuCommand::SortBy(key, ascending) => MenuAction::SortBy(key, ascending),
            MenuCommand::SetView(view) => MenuAction::View(view),
            MenuCommand::Copy => MenuAction::Copy,
            MenuCommand::Paste => MenuAction::Paste,
            MenuCommand::MoveHere => MenuAction::MoveHere,
            MenuCommand::OpenWith(app) => {
                let Some(window) = self.windows.get(&id) else {
                    return Vec::new();
                };
                let paths = window.selected_paths();
                return self.session.open_paths(id, paths, Some(app));
            }
            MenuCommand::ToggleTag(color) => {
                if let Some(window) = self.windows.get(&id) {
                    window.toggle_tag(color, &self.session);
                }
                return Vec::new();
            }
        };
        let state = match self.windows.get(&id) {
            Some(window) => window.menu_state(&self.session),
            None => return Vec::new(),
        };
        // the menu bar's rules decide what the window can do now, as they do for its items
        if menu_bar::enabled(&state)
            .into_iter()
            .any(|(a, enabled)| a == action && !enabled)
        {
            return Vec::new();
        }
        self.menu(id, action)
    }

    /// Take what the removable media reported: the rows follow the devices, a window on a file system that went away
    /// goes home, a passphrase sheet of a container that went away closes, and a finished mount goes to its folder in
    /// the window that asked for it.
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
            let mut arrived = Vec::new();
            for report in reports {
                let object = match &report {
                    Report::Mounted { object, .. } | Report::Done { object } => {
                        session.mounting.remove(object)
                    }
                    Report::Devices(_) => None,
                };
                if let Some(at) = session.media.take(report) {
                    arrived.extend(object.map(|window| (window, at)));
                }
            }
            session.show_media();
            for ((&id, window), held) in self.windows.iter_mut().zip(held) {
                if held && !session.media.holds(window.browser.dir()) {
                    window.go(session.home.clone(), session);
                }
                if let Some((_, at)) = arrived.iter().rev().find(|(w, _)| *w == id) {
                    window.go(at.clone(), session);
                }
                let gone = window
                    .unlock
                    .as_ref()
                    .is_some_and(|u| session.media.index_of(&u.object).is_none());
                if gone {
                    window.unlock = None;
                }
            }
        }
        vec![woken(&session.media_woken, Message::Media)]
    }

    /// Open place `index` in the window `id`, as a click on its row does: a folder is shown, a removable file system
    /// that is not mounted is mounted (the window goes there once it is), a locked container asks for its passphrase,
    /// and a row with a call under way does nothing.
    fn open_place(&mut self, id: u64, index: usize) -> Vec<Command<Message>> {
        let session = &mut self.session;
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        let Some(place) = session.places.get(index) else {
            return Vec::new();
        };
        let row = view::place_key(session, index);
        if let Some(folder) = place.folder() {
            let folder = folder.to_path_buf();
            window.go(folder, session);
            window.sidebar_row = Some(row);
            return Vec::new();
        }
        window.sidebar_row = Some(row);
        let Some(row) = index.checked_sub(session.standard_places) else {
            return Vec::new();
        };
        match session.media.device(row) {
            Some((device, false)) if device.locked => {
                window.unlock = Some(Unlock {
                    object: device.object.clone(),
                    name: device.name.clone(),
                    passphrase: Passphrase::default(),
                });
            }
            Some((_, false)) => {
                if let (Some(volumes), Some(device)) = (&session.volumes, session.media.start(row))
                {
                    session.mounting.insert(device.object.clone(), id);
                    volumes.mount(&device);
                    session.show_media();
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// Unlock the container of the passphrase sheet of the window `id` with the passphrase typed; the window goes to
    /// what it holds once that is mounted. An empty passphrase keeps the sheet open.
    fn unlock(&mut self, id: u64) -> Vec<Command<Message>> {
        let session = &mut self.session;
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        if window
            .unlock
            .as_ref()
            .is_none_or(|u| u.passphrase.is_empty())
        {
            return Vec::new();
        }
        let Some(unlock) = window.unlock.take() else {
            return Vec::new();
        };
        let started = session
            .media
            .index_of(&unlock.object)
            .and_then(|row| session.media.start(row));
        if let (Some(volumes), Some(device)) = (&session.volumes, started) {
            session.mounting.insert(device.object.clone(), id);
            volumes.unlock(&device, unlock.passphrase);
            session.show_media();
        }
        Vec::new()
    }

    /// A press on the accessory of the sidebar row `key`: the eject button of a mounted removable file system.
    fn accessory(&mut self, key: &Key) -> Vec<Command<Message>> {
        let session = &self.session;
        let row = (0..session.places.len())
            .find(|&i| view::place_key(session, i) == *key)
            .filter(|&i| session.places[i].ejectable());
        if let Some(index) = row {
            self.eject(index);
        }
        Vec::new()
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

    /// Read the standard places again, keeping the removable media's after them, and watch the trash once it exists.
    fn follow_places(&mut self) {
        let trash = self.trash.home_trash().to_path_buf();
        self.standard_places = places::reread(
            &mut self.places,
            self.standard_places,
            self.places_source.as_ref(),
            &trash.join("files"),
        );
        self.places_watch.follow(&trash);
    }

    fn show_media(&mut self) {
        self.places.truncate(self.standard_places);
        self.places.extend(self.media.places());
    }

    /// Open `path` with the application of desktop entry `with`, else its default one, handing it `token`. The
    /// associations are read at each open, so changed defaults and new applications apply at once.
    fn launch(&self, path: &Path, with: Option<&str>, token: Option<&str>) {
        let is_dir = path.is_dir();
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
                launch::spawn(APP_ID, spec.to_command());
            }
            Err(e) => self.failed(Action::Open, &[path], &e),
        }
    }

    /// Ask the window `id` for an activation token for each of `paths`, naming the application that will open it, so
    /// the dock shows it starting; each opens once its token comes.
    fn open_paths(
        &self,
        id: u64,
        paths: Vec<PathBuf>,
        with: Option<String>,
    ) -> Vec<Command<Message>> {
        if paths.is_empty() {
            return Vec::new();
        }
        let apps = match (self.applications)() {
            Ok(apps) => apps,
            Err(e) => {
                let items: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
                self.failed(Action::Open, &items, &e);
                return Vec::new();
            }
        };
        paths
            .into_iter()
            .map(|path| {
                let app_id = with
                    .clone()
                    .or_else(|| default_app_id(&apps, &path, path.is_dir()));
                let with = with.clone();
                Command::ActivationToken {
                    window: window_key(id),
                    app_id,
                    on_token: Box::new(move |token| {
                        Message::Window(id, Event::Launch { path, with, token })
                    }),
                }
            })
            .collect()
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
        // the viewport stays: the view keeps its size
        self.scroll = ScrollState {
            offset: NO_SCROLL.offset,
            content: NO_SCROLL.content,
            viewport: self.scroll.viewport,
        };
        self.rename = None;
        self.rename_when_listed = None;
        self.focus_when_listed = None;
        self.sidebar_row = None;
        self.band = None;
        if !self.search.is_empty() {
            self.search.clear();
            self.search_revision += 1;
        }
        self.follow_search();
        self.now = (self.clock)();
        self.read_available();
    }

    /// Read the free space of the folder's file system for the status bar: on entering a folder, whenever its listing
    /// changes, and after a transfer, so it follows what the window and others write there.
    fn read_available(&mut self) {
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
        // a folder whose file system cannot be read takes the changes, and their failures say why
        self.mounted_read_only = self.browser.read_only().unwrap_or(false);
    }

    /// Whether the folder shown takes nothing new: its medium can't be written, or its file system is mounted
    /// read-only.
    fn read_only(&self, session: &Session) -> bool {
        self.mounted_read_only
            || session
                .media
                .holding(self.browser.dir())
                .is_some_and(|d| d.read_only)
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

    /// The transfer `tid` of the window `id` reported: its row shows its progress; once it ended the failures are
    /// reported together (a stop is none), the last item that arrived is selected when the window still shows where it
    /// went, and the free space is read again.
    fn transferred(&mut self, id: u64, tid: u64, session: &Session) -> Vec<Command<Message>> {
        let Some(at) = self.transfers.iter().position(|t| t.id == tid) else {
            return Vec::new();
        };
        let Some(results) = self.transfers[at].poll() else {
            let wake = woken(
                &self.transfers[at].woken,
                Message::Window(id, Event::Transferred(tid)),
            );
            return vec![wake];
        };
        let done = self.transfers.remove(at);
        let arrived = results.iter().rev().find_map(|p| p.as_ref().ok()).cloned();
        let notice = match done.moving {
            true => Action::Drop(DropAction::Move),
            false => Action::Paste,
        };
        let stopped = results
            .iter()
            .any(|r| r.as_ref().is_err_and(operations::is_cancelled));
        let failures = results
            .iter()
            .filter(|r| r.as_ref().is_err_and(|e| !operations::is_cancelled(e)))
            .count();
        self.announcement
            .say(announce::About::Transfer, done.ended(stopped, failures));
        let failed = done
            .sources
            .iter()
            .zip(results)
            .filter(|(_, r)| !r.as_ref().is_err_and(operations::is_cancelled));
        session.failed_some(notice, failed);
        if let Some(path) = arrived.filter(|p| p.parent() == Some(self.browser.dir())) {
            self.browser.select_when_appears(path);
        }
        self.read_available();
        Vec::new()
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

    /// Go into the selected folder, or open the selected files with their default applications, each once the window
    /// `id` has its activation token.
    fn open_selection(&mut self, id: u64, session: &Session) -> Vec<Command<Message>> {
        let Activation { folder, files } = self.browser.activation();
        let commands = session.open_paths(id, files, None);
        if let Some(folder) = folder {
            self.go(folder, session);
        }
        commands
    }

    /// Open the context menu at `at`: of the selection after a press on an item, which it selects unless it is selected;
    /// of the folder after a press on empty space, which clears the selection; from the keyboard, of the selection, or
    /// of the folder without one. A name being edited is committed first.
    fn open_context(&mut self, at: cupertino_kit::geometry::Point, session: &Session) {
        if self.rename.is_some() {
            let _ = self.end_rename(session);
            if self.rename.is_some() {
                self.pressed = None;
                return;
            }
        }
        let on_item = match self.pressed.take() {
            Some(Some(index)) => {
                if !self.browser.selection().contains(index) {
                    self.browser.select([index], Some(index), Some(index));
                }
                true
            }
            Some(None) => {
                self.browser.select(std::iter::empty(), None, None);
                false
            }
            None => !self.browser.selection().is_empty(),
        };
        let in_trash = self.browser.dir() == session.trash.home_trash().join("files");
        let (label, items) = if on_item {
            let subject = self.subject(in_trash);
            let label = subject.name.clone();
            (
                label,
                menu_spec::item_menu(&subject, &self.selection_apps(session)),
            )
        } else {
            let background = self.background(session, in_trash);
            let label = crate::places::window_title(self.browser.dir(), &session.places);
            (label, menu_spec::background_menu(&background))
        };
        self.context = Some(context::OpenMenu {
            anchor: MenuAnchor::At(at),
            label,
            items,
            open: Vec::new(),
        });
    }

    /// Open the menu of the toolbar's button `popup` below it.
    fn open_popup(&mut self, popup: Popup, session: &Session) {
        if self.rename.is_some() {
            let _ = self.end_rename(session);
        }
        let in_trash = self.browser.dir() == session.trash.home_trash().join("files");
        let has_selection = !self.browser.selection().is_empty();
        let (label, items) = match popup {
            Popup::View => ("View".to_owned(), menu_spec::view_menu(self.view)),
            Popup::Tags => {
                let subject = self.subject(in_trash);
                let on = if has_selection {
                    subject.tags_on
                } else {
                    Vec::new()
                };
                ("Tags".to_owned(), menu_spec::tags_menu(&on, has_selection))
            }
            Popup::Action if has_selection => {
                let subject = self.subject(in_trash);
                let items = menu_spec::item_menu(&subject, &self.selection_apps(session));
                ("Action".to_owned(), items)
            }
            Popup::Action => {
                let background = self.background(session, in_trash);
                ("Action".to_owned(), menu_spec::background_menu(&background))
            }
        };
        self.context = Some(context::OpenMenu {
            anchor: MenuAnchor::Below(view::popup_key(popup)),
            label,
            items,
            open: Vec::new(),
        });
    }

    /// What the folder's menu depends on.
    fn background(&self, session: &Session, in_trash: bool) -> Background {
        Background {
            view: self.view,
            sort: self.browser.sort(),
            in_trash,
            read_only: self.read_only(session),
            pasteboard: session.pasteboard.len(),
        }
    }

    /// What the menu of the selection depends on.
    fn subject(&self, in_trash: bool) -> Subject {
        let entries = self.browser.entries();
        let selected: Vec<&crate::Entry> = self
            .browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
            .collect();
        let tags_on = TagColor::ALL
            .into_iter()
            .filter(|c| selected.iter().all(|e| has_tag(e, *c)))
            .collect();
        Subject {
            count: selected.len(),
            name: selected.first().map(|e| e.name.clone()).unwrap_or_default(),
            all_folders: selected.iter().all(|e| e.is_dir),
            single_link: selected.len() == 1 && selected[0].kind == crate::Kind::Symlink,
            in_trash,
            tags_on,
        }
    }

    /// The applications that open every selected item, for Open With.
    fn selection_apps(&self, session: &Session) -> Vec<AppChoice> {
        let apps = match (session.applications)() {
            Ok(apps) => apps,
            Err(e) => {
                eprintln!("cupertino-files: cannot read the default applications: {e}");
                return Vec::new();
            }
        };
        let entries = self.browser.entries();
        let lists: Vec<Vec<AppChoice>> = self
            .browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
            .map(|e| {
                open_with(&apps, &e.path, e.is_dir)
                    .into_iter()
                    .map(|item| AppChoice {
                        id: item.entry.id.clone(),
                        name: item.entry.name.clone(),
                        is_default: item.is_default,
                    })
                    .collect()
            })
            .collect();
        common_apps(&lists)
    }

    /// Add the tag of `color` to the selected items, or take it off when every one has it; failures are reported
    /// together.
    fn toggle_tag(&self, color: TagColor, session: &Session) {
        let entries = self.browser.entries();
        let selected: Vec<&crate::Entry> = self
            .browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
            .collect();
        let all_have = selected.iter().all(|e| has_tag(e, color));
        let paths: Vec<PathBuf> = selected.iter().map(|e| e.path.clone()).collect();
        let changed = paths.iter().map(|path| {
            let changed = if all_have {
                tags::remove_tag(path, color.title())
            } else {
                tags::add_tag(path, color.title())
            };
            (path, changed)
        });
        session.failed_some(Action::Tag, changed);
    }

    /// End the search: the folder is listed whole again.
    fn clear_search(&mut self) {
        if !self.search.is_empty() {
            self.search.clear();
            self.search_revision += 1;
            self.browser.set_filter("");
        }
        self.follow_search();
    }

    /// The search field opens while it has the keyboard or text, and rests otherwise.
    fn follow_search(&mut self) {
        let open = search_field::is_open(self.search_focused, &self.search);
        self.search_width.want(open);
    }

    /// The toolbar's width: the folder view's, which spans the same pane; unbounded before the view is laid out.
    fn toolbar_width(&self) -> f64 {
        match self.scroll.viewport.width {
            w if w > 0.0 => w,
            _ => f64::INFINITY,
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

    /// Give the keyboard to the row or icon of the item at `path` of window `id` once the view shows it.
    fn focus_item(&self, id: u64, path: &Path) -> Vec<Command<Message>> {
        let Some(index) = self.browser.entries().iter().position(|e| e.path == path) else {
            return Vec::new();
        };
        let key = match self.view {
            ViewMode::Icons => view::item_key(id, index),
            _ => view::row_key(id, path),
        };
        vec![Command::Perform(Box::pin(async move {
            Message::Window(id, Event::FocusKey(key))
        }))]
    }

    /// Close the folder of row `index` of window `id`. When the keyboard was on an item inside it, the folder's row
    /// takes the keyboard, where the selection's cursor goes.
    fn collapse(&mut self, id: u64, index: usize) -> Vec<Command<Message>> {
        let entries = self.browser.entries();
        let folder = entries.get(index).map(|e| e.path.clone());
        let cursor = self
            .browser
            .selection()
            .focus()
            .and_then(|c| entries.get(c));
        let inside = match (&folder, cursor) {
            (Some(folder), Some(cursor)) => {
                cursor.path != *folder && cursor.path.starts_with(folder)
            }
            _ => false,
        };
        if !self.browser.collapse(index) || !inside || !self.items_focused {
            return Vec::new();
        }
        folder.map_or_else(Vec::new, |folder| self.focus_item(id, &folder))
    }

    /// What the window's live region follows of its state.
    fn tally(&self) -> announce::Tally {
        announce::Tally {
            results: self.browser.search_results(),
            selected: self.browser.selection().len(),
            banding: self.band.is_some(),
        }
    }

    /// The row of the name being edited.
    fn rename_index(&self) -> Option<usize> {
        self.rename.as_ref().map(|r| r.edit.index())
    }

    /// Commit the edited name. A name that is not valid is reported and the field stays open. A rename that changes
    /// the extension is not confirmed (there is no dialog to ask in). The item's path once the name is committed, the
    /// same when the name did not change; `None` when the field stays open or the rename failed.
    fn end_rename(&mut self, session: &Session) -> Option<PathBuf> {
        let rename = self.rename.take()?;
        match rename.edit.commit() {
            Ok(None) => Some(rename.path),
            Ok(Some(name)) => match operations::rename_item(&rename.path, &name) {
                Ok(path) => {
                    self.browser.select_when_appears(path.clone());
                    Some(path)
                }
                Err(e) => {
                    session.failed(Action::Rename, &[&rename.path], &e);
                    None
                }
            },
            Err(e) => {
                session
                    .notices
                    .post(notice::bad_name(rename.edit.text(), &e));
                self.rename = Some(rename);
                None
            }
        }
    }

    /// Return committed the name of window `id`'s rename: its item takes the keyboard, at once when it is listed under
    /// that name (the name did not change), else once the view lists it.
    fn submit_rename(&mut self, id: u64, session: &Session) -> Vec<Command<Message>> {
        let Some(path) = self.end_rename(session) else {
            return Vec::new();
        };
        if self.browser.entries().iter().any(|e| e.path == path) {
            return self.focus_item(id, &path);
        }
        self.focus_when_listed = Some(path);
        Vec::new()
    }

    /// The list of the window `id` changed: the name being edited follows its item, an item just renamed takes the
    /// keyboard, and a folder just made starts its rename.
    fn listed(&mut self, id: u64) -> Vec<Command<Message>> {
        let renamed = self
            .focus_when_listed
            .take_if(|path| self.browser.entries().iter().any(|e| &e.path == path));
        if let Some(path) = renamed {
            let mut commands = self.focus_item(id, &path);
            commands.extend(self.listed(id));
            return commands;
        }
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
            eject: self
                .eject_target(session)
                .map(|i| session.places[i].name.clone()),
            read_only: self.read_only(session),
            can_paste: !session.pasteboard.is_empty(),
        }
    }

    /// The place Eject acts on: while the keyboard is in the sidebar, the mounted removable file system its row is on;
    /// else the one that holds the folder shown.
    fn eject_target(&self, session: &Session) -> Option<usize> {
        // a drive that also holds the home folder is the session's own disk: the chord never takes it
        let ejectable = |i: usize| {
            session.places[i].ejectable()
                && !i
                    .checked_sub(session.standard_places)
                    .is_some_and(|device| session.media.drive_holds(device, &session.home))
        };
        let focused = self
            .sidebar_row
            .as_ref()
            .filter(|_| self.sidebar_focused)
            .and_then(|key| {
                (0..session.places.len()).find(|&i| view::place_key(session, i) == *key)
            })
            .filter(|&i| ejectable(i));
        focused.or_else(|| {
            removable_holding(self.browser.dir(), &session.places).filter(|&i| ejectable(i))
        })
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

/// Whether `entry` has the tag of `color`.
fn has_tag(entry: &crate::Entry, color: TagColor) -> bool {
    entry
        .tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case(color.title()))
}

/// Open the window on `start` and serve it until the last window closes.
pub fn run(start: Start) -> Result<(), cupertino_kit::app::AppError> {
    prepare(start);
    cupertino_kit::app::run::<Files>()
}
