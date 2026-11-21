//! The browser window on CupertinoKit: the sidebar's places, the folder as a list or a grid of icons, the path and
//! status bars and the menus of the desktop's menu bar, over the toolkit-free [`Browser`]. It is the window `main.rs`
//! runs.

mod announce;
mod columns;
mod context;
mod dragged;
mod drops;
mod gallery;
mod jobs;
mod launch;
mod media;
mod open;
mod peek;
mod places;
mod progress;
mod reads;
mod remote;
mod search;
mod share_popover;
mod sidebar;
mod spring;
mod tabs;
mod tag_editor;
mod toolbar;
mod transfer;
mod view;

pub use drops::Destination;
pub use jobs::{JobEvent, PROGRESS_WINDOW_DELAY};
pub use launch::{Buses, Ended, Handed, LaunchEvent};
pub use peek::PeekEvent;
pub use places::PlacesSource;
pub use remote::{LookUp, LOOK_UP_DEADLINE, MAX_LOOK_UPS};
pub use share_popover::ShareEvent;
pub use tabs::TabEvent;
pub use tag_editor::TagEvent;
pub use transfer::Observer;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use cupertino_apps::{MimeApps, MimeDatabase};
use cupertino_kit::bus::Bus;
use cupertino_kit::controls::{
    MenuAnchor, Navigation, Selection, SortOrder, SplitLayout, SplitPane, TextChange,
};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::layout::ScrollState;
use cupertino_kit::platform::{DragProgress, WindowRequest};
use cupertino_kit::prelude::*;
use cupertino_menu_model::ActionName;
use zbus::zvariant::OwnedObjectPath;

use crate::browser::{Activation, Asked, Browser, Listings, Polled};
use crate::context_menu::{common_apps, AppChoice, Command as MenuCommand};
use crate::instance::Service;
use crate::menu_bar::{self, MenuAction, MenuState};
use crate::menu_spec::{self, Background, Subject};
use crate::preview::Browse;
use crate::search_field::{self, SearchWidth};
use crate::settings::ColumnId;
use crate::tab_strip::{TabId, TabStrip};
use crate::trash::Trash;
use crate::udisks::{Media, Passphrase, Volumes};
use crate::window::ViewMode;
use crate::window_prefs::WindowPrefs;
use cupertino_icons::thumbnail::Thumbnailer;
use cupertino_kit::files::entry::{Entry, Kind};
use cupertino_kit::files::grid::Grab;
use cupertino_kit::files::item_icons::ItemIcons;
use cupertino_kit::files::list::{row_key, Presses, RowGrab};
use cupertino_kit::files::name_edit::NameEdit;
use cupertino_kit::files::notice::{self, Action, Notices};
use cupertino_kit::files::open::open_with;
use cupertino_kit::files::operations::{self, DropAction};
use cupertino_kit::files::places::{removable_holding, Place};
use cupertino_kit::files::sort::SortSpec;
use cupertino_kit::files::tags::{self, TagColor};
use cupertino_kit::files::thumbnail_service::JOB_DEADLINE;
use cupertino_kit::files::{ColumnEvent, ViewEvent};

/// Identifier of the window for the server, as the desktop entry names it.
pub const APP_ID: &str = "cupertino-files";

/// What the first window opens with; [`Start::open`] hands over the folder, which the toolkit's I/O tasks list.
pub struct Start {
    /// The first window's folder, what wakes when it changes and what wakes it; none for a Files started by a call
    /// ([`Start::without_window`]).
    first: Option<(Browser, async_channel::Receiver<()>, Wake)>,
    places: Vec<Place>,
    /// What opens a file: the session's associations, read at each open so changed defaults apply at once.
    applications: Box<dyn Fn() -> io::Result<MimeApps>>,
    terminal: Option<Vec<String>>,
    types: Option<Arc<MimeDatabase>>,
    recents: PathBuf,
    /// What Files does on the buses; none for a Files of its own, whose notices are logged.
    buses: Option<Buses>,
    trash: Trash,
    /// What a new window starts with, and the file that keeps it between launches.
    prefs: WindowPrefs,
    prefs_file: Option<PathBuf>,
    home: Option<PathBuf>,
    observer: Option<Observer>,
    /// How long a file operation runs before the progress window opens by itself.
    progress_delay: Duration,
    places_source: Option<PlacesSource>,
    /// Folders whose changes may change the places, besides the trash.
    places_folders: Vec<PathBuf>,
    file_icons: Option<FileIcons>,
    /// Where the applications' own icons are looked up, and the generic one; without them an application shows its
    /// type's icon.
    app_icons: Option<(cupertino_icons::ThemeIcons, cupertino_icons::SystemIcons)>,
    thumbnails: Option<Thumbnailer>,
    /// What the list words its dates relative to.
    clock: fn() -> SystemTime,
    /// The activation token the process was started with, which raises the first window.
    activation: Option<String>,
    /// What tells, on a look-up task, whether an item a call names exists.
    look_up: LookUp,
    /// The listings the windows let go of while their work runs.
    listings: Listings,
}

impl Start {
    /// The window on `folder` with the sidebar's `places`, its items typed with `types` and the Recents folder read
    /// from `recents`; a folder that cannot be read leaves a notice, logged unless Files is on the buses
    /// ([`Start::with_buses`]).
    pub fn open(
        folder: PathBuf,
        places: Vec<Place>,
        types: Option<Arc<MimeDatabase>>,
        recents: PathBuf,
        applications: Box<dyn Fn() -> io::Result<MimeApps>>,
        terminal: Option<Vec<String>>,
    ) -> Start {
        let listings = Listings::default();
        let first = open_browser(folder, types.clone(), recents.clone(), listings.clone());
        Start {
            first: Some(first),
            listings,
            ..Start::idle(places, types, recents, applications, terminal)
        }
    }

    /// [`Start::open`] without its window: Files opens the windows the calls it serves ask for, and exits when the
    /// first call has not come within [`CALL_WAIT`], or once it opened nothing.
    pub fn idle(
        places: Vec<Place>,
        types: Option<Arc<MimeDatabase>>,
        recents: PathBuf,
        applications: Box<dyn Fn() -> io::Result<MimeApps>>,
        terminal: Option<Vec<String>>,
    ) -> Start {
        Start {
            first: None,
            places,
            applications,
            terminal,
            types,
            recents,
            buses: None,
            trash: Trash::new(),
            prefs: WindowPrefs::default(),
            prefs_file: None,
            home: None,
            observer: None,
            progress_delay: jobs::PROGRESS_WINDOW_DELAY,
            places_source: None,
            places_folders: Vec::new(),
            file_icons: None,
            app_icons: None,
            thumbnails: None,
            clock: SystemTime::now,
            activation: None,
            look_up: Arc::new(remote::look_up_item),
            listings: Listings::default(),
        }
    }

    /// Files on the buses (`buses`): one connection to the session bus, made by the toolkit, posts its notices and
    /// serves its names (the calls of later launches and of the desktop, [`crate::instance`]) once the launch claims
    /// them, or hands the launch to the Files that owns them; the removable media are those of the system bus. The
    /// first window opens once the launch serves the names, or finds no session bus.
    pub fn with_buses(self, buses: Buses) -> Start {
        Start {
            buses: Some(buses),
            ..self
        }
    }

    /// The same start without its first window, as [`Start::idle`] makes it.
    pub fn without_window(self) -> Start {
        Start {
            first: None,
            ..self
        }
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
        Start {
            prefs: WindowPrefs { view, ..self.prefs },
            ..self
        }
    }

    /// The windows opening with the view and bars kept at `file` ([`crate::window_prefs`]), and a window's change of
    /// them kept there. Without it nothing is read or written.
    pub fn with_prefs_file(self, file: PathBuf) -> Start {
        Start {
            prefs: WindowPrefs::load(&file),
            prefs_file: Some(file),
            ..self
        }
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

    /// Every file operation calling `observer` with its progress on its own thread, before it looks for a stop.
    pub fn with_transfer_observer(self, observer: Observer) -> Start {
        Start {
            observer: Some(observer),
            ..self
        }
    }

    /// The progress window opening by itself once a file operation has run for `delay`, in place of the usual delay.
    pub fn with_progress_delay(self, delay: Duration) -> Start {
        Start {
            progress_delay: delay,
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

    /// The applications of the Applications folder drawn with their own icons, looked up in `icons`.
    pub fn with_app_icons(
        self,
        icons: cupertino_icons::ThemeIcons,
        system: cupertino_icons::SystemIcons,
    ) -> Start {
        Start {
            app_icons: Some((icons, system)),
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

    /// The items the calls name looked up with `look_up` (on an I/O task), in place of reading their status.
    pub fn with_look_up(self, look_up: LookUp) -> Start {
        Start { look_up, ..self }
    }

    /// The first window raised with `token`, the activation token Files was started with, once it is shown; a launch
    /// that hands over to the running Files hands the token on.
    pub fn with_activation_token(self, token: Option<String>) -> Start {
        Start {
            activation: token,
            ..self
        }
    }
}

/// What a model calls when it has changes waiting.
pub(super) type Wake = Arc<dyn Fn() + Send + Sync>;

/// What a model calls when it has changes waiting, and what wakes then.
fn wake_channel() -> (Wake, async_channel::Receiver<()>) {
    let (wake, woken) = async_channel::bounded(1);
    let wake = Arc::new(move || {
        // a wake already waiting covers this change too
        let _ = wake.try_send(());
    });
    (wake, woken)
}

/// What window `id`'s file views report, as its messages: each event to the window's `Event` for it.
fn view_events(id: u64) -> Rc<dyn Fn(ViewEvent) -> Option<Message>> {
    Rc::new(move |event| {
        let event = match event {
            ViewEvent::Select(selection) => Event::Select(selection),
            ViewEvent::Open(index) => Event::Open(index),
            ViewEvent::SecondaryPress(index) => Event::SecondaryPress(index),
            ViewEvent::ContextMenu(at) => Event::ContextMenu(at),
            ViewEvent::DragOut(grab) => Event::DragOut(grab),
            ViewEvent::DragRows(grab) => Event::DragRows(grab),
            ViewEvent::Band(band) => Event::Band(band),
            ViewEvent::Scroll(scroll) => Event::Scroll(scroll),
            ViewEvent::Rename(index) => Event::RenameItem(index),
            ViewEvent::FocusRow(index) => Event::FocusRow(index),
            ViewEvent::Sort(sort) => Event::Sort(sort),
            ViewEvent::Toggle(toggle) => Event::Toggle(toggle),
            ViewEvent::Column(ColumnEvent::Scroll(folder, scroll)) => {
                Event::ColumnScroll(folder, scroll)
            }
            ViewEvent::Column(ColumnEvent::Pick {
                folder,
                index,
                secondary,
            }) => Event::ColumnPick {
                folder,
                index,
                secondary,
            },
            ViewEvent::Column(ColumnEvent::Step(by)) => Event::ColumnStep(by),
            ViewEvent::Column(ColumnEvent::Resize { index, width, all }) => {
                Event::ColumnResize { index, width, all }
            }
            ViewEvent::Column(ColumnEvent::Resized) => Event::ColumnResized,
        };
        Some(Message::Window(id, event))
    })
}

/// The browser of `folder`, what wakes when its model has changes waiting, and what wakes it: the window's other tabs
/// wake it too.
fn open_browser(
    folder: PathBuf,
    types: Option<Arc<MimeDatabase>>,
    recents: PathBuf,
    listings: Listings,
) -> (Browser, async_channel::Receiver<()>, Wake) {
    let (wake, woken) = wake_channel();
    let browser = Browser::open(
        folder,
        SortSpec::default(),
        recents,
        types,
        Arc::clone(&wake),
        listings,
    );
    (browser, woken, wake)
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
    /// Files exits once its notices are sent: the messages until then do not start that wait again.
    exiting: bool,
    /// Files started without a window and waits for the first call it serves, until [`CALL_WAIT`] passes.
    awaiting_call: bool,
    /// The calls whose items are being looked up, each by an I/O task.
    look_ups: remote::LookUps,
    /// What Files does on the buses, until the session bus connected; the launch under way once it did.
    buses: Option<Buses>,
    launching: Option<launch::Launching>,
    /// The first window, which opens once the launch serves the names or runs alone, and the token that raises it.
    first: Option<(Browser, async_channel::Receiver<()>, Wake)>,
    activation: Option<String>,
}

/// How long summing the sizes of the folders one Move to Trash trashed should take: a walk of a large folder tree on a
/// local disk.
const TRASH_SIZES_DEADLINE: Duration = Duration::from_secs(30);

/// How long a Files started without a window ([`Start::idle`]) waits for the call it was started for before it exits.
pub const CALL_WAIT: Duration = Duration::from_secs(10);

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
    /// The removable media, once the system bus connected and udisks is listened for; what wakes when they report.
    volumes: Option<Volumes>,
    media_wake: Wake,
    media_woken: async_channel::Receiver<()>,
    media: Media,
    /// The window that asked for each mount or unlock under way, by the device's block.
    mounting: HashMap<OwnedObjectPath, u64>,
    /// Where a window on a file system that goes away goes.
    home: PathBuf,
    trash: Trash,
    /// Whether the trash holds anything: the home trash as it is, the mounted volumes' as last looked at by an I/O
    /// task, which a volume that does not answer would hold up.
    trash_look: Arc<cupertino_trash::Look>,
    /// How a new window shows its folder and which bars it shows: the last choice any window made.
    prefs: std::cell::Cell<WindowPrefs>,
    /// Where that choice is kept between launches.
    prefs_file: Option<PathBuf>,
    /// What Copy took last, for Paste Item and Move Item Here in any window.
    pasteboard: Vec<PathBuf>,
    /// The pasteboard's items cannot leave their folder ([`Window::items_stay`]): Move Item Here is off.
    pasteboard_stays: bool,
    /// The file operations, which outlive the windows that started them, and the progress window.
    jobs: jobs::Jobs,
    /// The ends of file operations posted as notices, newest last, by operation id, with the failure each reported: a
    /// click on one lists it again.
    noticed: VecDeque<(u64, Option<jobs::Failure>)>,
    places_source: Option<PlacesSource>,
    /// The standard places are to be read again, and a read of them is running.
    places_due: bool,
    places_reading: bool,
    /// The write state of the folders drags hovered, filled by I/O tasks ([`Session::mounts_task`]).
    mounts: Rc<crate::space::MountedCache>,
    places_watch: places::PlacesWatch,
    /// The items' icons and thumbnails, shared with the list's rows, which ask for them as they show.
    icons: Rc<ItemIcons>,
    /// What wakes when a thumbnail is made.
    thumbnails_woken: async_channel::Receiver<()>,
    /// The items a drag out of a window carries, until it is dropped or ends: a drop held open while it copies does not
    /// make later drags look like the window's own.
    dragged: Vec<PathBuf>,
    /// What the list words its dates relative to.
    clock: fn() -> SystemTime,
    /// The bus name later launches hand their requests to, until Files exits.
    instance: Option<Service>,
    /// The tags the items the windows listed carry, which the sidebar's Tags section lists from then on.
    known_tags: RefCell<BTreeSet<String>>,
    /// The colours of the colour tags' dots.
    tag_colors: tag_editor::TagColors,
    /// The fixed-pitch family the previews set text files in.
    text_family: Option<String>,
    /// Counts the messages that may change what the windows show, any but a window's scroll and the end of a listing's
    /// or a thumbnail's work (what it brought wakes the windows): views made at the same
    /// revision may keep what they built ([`cupertino_kit::core::Element::kept`]).
    view_revision: u64,
    /// The listings the windows let go of while their work runs, which the next request for their folders takes.
    listings: Listings,
}

/// One browser window.
struct BrowserWindow {
    browser: Browser,
    woken: async_channel::Receiver<()>,
    /// What every tab's folder calls when it changes, which wakes the window.
    wake: Wake,
    /// The window's tabs, and those not shown ([`tabs`]).
    tabs: TabStrip,
    parked: HashMap<TabId, tabs::Tab>,
    /// View › Show Tab Bar: the bar shows with a single tab too.
    tab_bar: bool,
    path_bar: bool,
    status_bar: bool,
    /// The free space of the folder's file system, for the status bar.
    available: Option<u64>,
    /// Whether the folder's file system is mounted read-only, read with the free space.
    mounted_read_only: bool,
    /// The free space is to be read again: the next [`Model::io_work`] starts the task.
    space_due: bool,
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
    /// The question before the trash is emptied is up.
    confirm_empty: bool,
    /// The trash's folder, whose view has a strip with an Empty button.
    trash_dir: PathBuf,
    /// The session's look at what the mounted volumes' trash holds.
    trash_look: Arc<cupertino_trash::Look>,
    /// While the window shows the trash: a mounted volume's trash holds something, as last looked, which the Empty
    /// button deletes too. Read with the free space, and when a look finds it changed.
    volumes_hold_trash: bool,
    /// How the folder is shown.
    view: ViewMode,
    /// The context menu open in the window.
    context: Option<context::OpenMenu>,
    /// What the latest secondary press was on, until its context menu opens: an item, or empty space.
    pressed: Option<Option<usize>>,
    /// The sidebar's place the latest secondary press was on, until its context menu opens.
    pressed_place: Option<usize>,
    /// The keyboard is in the sidebar.
    sidebar_focused: bool,
    /// The key of the sidebar row a click or the arrows last chose since the window last went to another folder: the
    /// row the sidebar's keyboard is on, when it is not the selected one.
    sidebar_row: Option<Key>,
    /// The sidebar row of the place whose folder the window asked for, which is the sidebar's row once it opens.
    place_asked: Option<Key>,
    /// The rubber band being drawn over the items, in the view's content.
    band: Option<cupertino_kit::geometry::Rect>,
    /// The presses on the list's rows, which its rows and its frame share.
    list_presses: Presses,
    /// What the window's live region reads last ([`announce`]).
    announcement: announce::Announcement,
    /// The keyboard is on the folder's items.
    items_focused: bool,
    /// An item just renamed, whose row or icon takes the keyboard once the view shows it.
    focus_when_listed: Option<PathBuf>,
    /// Folders a held drag opens ([`spring`]).
    springing: spring::Springing,
    /// The columns of the column view beyond the folder shown.
    columns: columns::ColumnsView,
    /// The start of the text files previewed.
    texts: peek::Texts,
    /// The preview window, while it is open.
    peek: Option<peek::Peek>,
    /// The tags popover, while it is open.
    tag_editor: Option<tag_editor::TagEditor>,
    /// The share popover, while it is open.
    share: Option<share_popover::ShareState>,
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
    /// The session bus connected, or could not.
    SessionBus(Handed<zbus::Result<zbus::Connection>>),
    /// The session bus connection closed.
    SessionClosed,
    /// The system bus connected, or could not.
    SystemBus(Handed<zbus::Result<zbus::Connection>>),
    /// The system bus connection closed: the removable media are no longer followed.
    SystemClosed,
    /// The removable media are listened for, or why not.
    MediaWatched(Handed<zbus::Result<Volumes>>),
    /// A call on the removable media ended, or has more to do.
    MediaCalled(Handed<crate::udisks::Called>),
    /// A mount whose file system was not there yet is tried again.
    MountAgain(Handed<crate::udisks::Pending>),
    /// The launch on the session bus ([`launch`]).
    Launch(LaunchEvent),
    /// A listing of a folder the sidebar's places come from has changes waiting.
    Places,
    /// A thumbnail was made.
    Thumbnails,
    /// A look at the mounted volumes' trash ended; whether their answer changed.
    TrashLooked(bool),
    /// An event of the window with this id.
    Window(u64, Event),
    /// A file operation, or the progress window.
    Job(JobEvent),
    /// Calls of later launches or of the desktop are waiting ([`crate::instance`]).
    Remote,
    /// The items of a call, by its id, were looked up, or its [`LOOK_UP_DEADLINE`] passed first.
    LookedUp(u64),
    /// The window with this key is shown: raise it, with the activation token of the launch that asked for it.
    Raise(WindowKey, Option<String>),
    /// A notice of a file operation's end was clicked; none once no click can come any more.
    Noticed(Option<notice::Click>),
    /// [`CALL_WAIT`] passed since Files started without a window.
    CallWaitOver,
    /// The standard places were read again: they replace the ones shown.
    PlacesRead(Vec<Place>),
    /// The write state of the file systems of folders a drag hovered was read.
    MountsRead(Vec<(PathBuf, bool)>),
    /// A listing's work or watch, a thumbnail's job or the posting of the notices ended; what it brought wakes the
    /// windows.
    Worked,
    /// Files exits: the notices posted are sent, or [`EXIT_NOTICES`] passed first.
    NoticesSent,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The folder's model has changes waiting.
    Woken,
    /// The file system of the folder `dir` was read, or could not be.
    Space(PathBuf, Result<crate::space::Space, String>),
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
        /// Whether it is a folder, as its listing's entry told.
        is_dir: bool,
        with: Option<String>,
        token: Option<String>,
    },
    /// Where the link `.0` leads (or why that could not be read) arrived, for Show Original.
    Original(PathBuf, Result<PathBuf, String>),
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
    /// Ask whether to empty the trash: the Trash's Empty button, and its row of the context menu.
    AskEmptyTrash,
    /// Empty the trash: the question's default button.
    EmptyTrash,
    /// The question before the trash is emptied was answered with its Cancel button or Escape.
    EmptyTrashCancelled,
    /// A secondary press on the item with this index, or on empty space.
    SecondaryPress(Option<usize>),
    /// A secondary press on the sidebar's row of the place with this index in the session's places.
    PlacePress(usize),
    /// Open the context menu of the sidebar at this point of the window.
    SidebarMenu(cupertino_kit::geometry::Point),
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
    /// Tab chose row `index` of the list: select it alone and give it the keyboard once the list shows it.
    FocusRow(usize),
    /// The list shows the row with this key: give it the keyboard.
    FocusKey(Key),
    /// The column view shows the row with this key: give it the keyboard, its ring showing only if it showed.
    FollowKey(Key),
    /// A toolbar button's menu: open it below the button.
    Popup(Popup),
    /// A folder of the path bar was clicked: go to it.
    PathSegment(PathBuf),
    /// A drag of files is over this folder of the list, the grid, the sidebar or the path bar, or over none of them.
    SpringOver(Option<PathBuf>),
    /// The timer of a drag's dwell over a folder is due: the dwell's generation and what the timer does.
    SpringDue(u64, crate::spring::Step),
    /// A drag that was over the window left it, or ended there without a drop.
    DragLeft,
    /// The column of this folder in the column view scrolled.
    ColumnScroll(PathBuf, ScrollState),
    /// Row `index` of the column of `folder`, a folder other than the one shown, was chosen: show that folder with the
    /// row selected; `secondary` when a secondary press chose it, whose context menu follows.
    ColumnPick {
        folder: PathBuf,
        index: usize,
        secondary: bool,
    },
    /// Right (1) or Left (-1) in the column of the folder shown.
    ColumnStep(isize),
    /// The divider of column `index` was dragged to make it `width` wide; with Option held (`all`), every column.
    ColumnResize {
        index: usize,
        width: f64,
        all: bool,
    },
    /// A column's divider was let go: new windows' columns start at the width it left.
    ColumnResized,
    /// The preview column's text scrolled.
    PreviewScroll(ScrollState),
    /// The preview window.
    Peek(PeekEvent),
    /// The tab bar.
    Tab(TabEvent),
    /// The tags popover.
    Tags(TagEvent),
    /// The share popover.
    Share(ShareEvent),
    /// The tag of this name in the sidebar was chosen: the folder's items that carry it are shown.
    TagPlace(String),
}

/// The toolbar's pop-up buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popup {
    /// How the folder is shown.
    View,
    /// The selection's tag colours.
    Tags,
    /// What the selection can be sent to.
    Share,
    /// The selection's menu, or the folder's without a selection.
    Action,
    /// The selected item's menu, from the preview column's button.
    More,
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

/// The most ends of file operations whose notices list them again when clicked.
const MAX_NOTICED: usize = 64;

/// How long Files waits, as it exits, for the notices it posted to be sent.
const EXIT_NOTICES: Duration = Duration::from_secs(2);

impl Application for Files {
    type Model = Model;
    type Message = Message;
    /// Files exits itself ([`Model::exit_when_done`]): a file operation outlives the windows.
    const RESIDENT: bool = true;
    /// Large windows, scrolled and resized: their repaints are the GPU's.
    const RENDERER: cupertino_kit::core::RendererClass = cupertino_kit::core::RendererClass::Gpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        let start = START
            .with(|s| s.borrow_mut().take())
            .expect("kit::prepare runs before Files starts");
        let places_watch =
            places::PlacesWatch::start(start.places_folders, start.trash.home_trash());
        let (thumbnails_wake, thumbnails_woken) = wake_channel();
        let trash_look = cupertino_trash::Look::new(start.trash.clone());
        // the first look is under way before a window or a menu asks what the volumes hold
        trash_look.look();
        let file_icons = start.file_icons.unwrap_or_else(|| {
            let types = start.types.as_deref().cloned().unwrap_or_default();
            FileIcons::builtin(types, HashMap::new())
        });
        let icons = Rc::new(
            ItemIcons::new(file_icons, start.thumbnails, thumbnails_wake)
                .with_app_icons(start.app_icons),
        );
        let (media_wake, media_woken) = wake_channel();
        let mut model = Model {
            windows: BTreeMap::new(),
            next_window: 0,
            exiting: false,
            awaiting_call: start.first.is_none(),
            look_ups: remote::LookUps::new(start.look_up),
            buses: start.buses,
            launching: None,
            first: start.first,
            activation: start.activation,
            session: Session {
                standard_places: start.places.len(),
                places: start.places,
                notices: Notices::logged(),
                applications: start.applications,
                terminal: start.terminal,
                types: start.types,
                recents: start.recents,
                volumes: None,
                media_wake,
                media_woken,
                media: Media::default(),
                mounting: HashMap::new(),
                home: start.home.unwrap_or_else(|| {
                    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
                }),
                trash: start.trash,
                trash_look,
                prefs: std::cell::Cell::new(start.prefs),
                prefs_file: start.prefs_file,
                pasteboard: Vec::new(),
                pasteboard_stays: false,
                jobs: jobs::Jobs::new(start.progress_delay, start.observer),
                noticed: VecDeque::new(),
                places_watch,
                places_source: start.places_source,
                places_due: false,
                places_reading: false,
                mounts: Rc::default(),
                icons,
                thumbnails_woken,
                dragged: Vec::new(),
                clock: start.clock,
                instance: None,
                known_tags: RefCell::new(BTreeSet::new()),
                tag_colors: tag_editor::tag_colors(),
                text_family: cupertino_kit::app::mono_family(),
                view_revision: 0,
                listings: start.listings,
            },
        };
        let mut commands = vec![woken(&model.session.places_watch.woken, Message::Places)];
        commands.extend(model.session.look_at_trash());
        if model.first.is_none() {
            commands.push(Command::After {
                delay: CALL_WAIT,
                message: Message::CallWaitOver,
            });
        }
        if model.buses.is_some() {
            // the first window waits for the launch on the session bus
            commands.extend(cupertino_kit::bus::connect(
                Bus::Session,
                |connection| Message::SessionBus(Handed::new(connection)),
                Message::SessionClosed,
            ));
            commands.extend(cupertino_kit::bus::connect(
                Bus::System,
                |connection| Message::SystemBus(Handed::new(connection)),
                Message::SystemClosed,
            ));
        } else {
            commands.extend(model.launched());
        }
        if model.session.icons.thumbnails() {
            commands.push(woken(&model.session.thumbnails_woken, Message::Thumbnails));
        }
        commands.extend(model.io_work());
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        if !matches!(
            message,
            Message::Window(_, Event::Scroll(_)) | Message::Worked
        ) {
            model.session.view_revision += 1;
        }
        let recheck = match &message {
            Message::Thumbnails => peek::Recheck::All,
            Message::Window(id, Event::Woken) => peek::Recheck::Window(*id),
            _ => peek::Recheck::Unsettled,
        };
        let mut commands = model.handle(message);
        commands.extend(model.follow_peeks(recheck));
        commands.extend(model.session.look_at_trash());
        commands.extend(model.io_work());
        commands.extend(model.exit_when_done());
        commands
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
                let preview = peek::window(&model.session, id, window)
                    .into_iter()
                    .chain(view::tag_popover(&model.session, id, window))
                    .chain(view::share_popover(&model.session, id, window));
                std::iter::once(view::window(&model.session, id, window))
                    .chain(preview)
                    .chain(menu.into_iter().flatten())
            })
            .chain(progress::window(&model.session.jobs))
            .collect()
    }
}

impl Model {
    /// Do what `message` asks.
    fn handle(&mut self, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Job(event) => self.job_event(event),
            Message::Noticed(click) => self.noticed(click),
            Message::Remote => self.remote_calls(),
            Message::LookedUp(id) => self.looked_up(id),
            Message::CallWaitOver => {
                self.awaiting_call = false;
                Vec::new()
            }
            Message::Worked => Vec::new(),
            Message::NoticesSent => vec![Command::Exit],
            Message::Raise(key, token) => {
                vec![Command::Window(key, WindowRequest::Activate(token))]
            }
            Message::Media => self.follow_media(),
            Message::SessionBus(connection) => match connection.take() {
                Some(connection) => self.session_bus(connection),
                None => Vec::new(),
            },
            // the connection's end leaves the names given up; what runs goes on, and a launch still under way runs alone
            Message::SessionClosed => match self.launching.take() {
                Some(_) => {
                    eprintln!(
                        "cupertino-files: the session bus closed before the launch was served"
                    );
                    self.launched()
                }
                None => Vec::new(),
            },
            Message::SystemBus(connection) => match connection.take() {
                Some(connection) => self.system_bus(connection),
                None => Vec::new(),
            },
            Message::SystemClosed => {
                eprintln!("cupertino-files: the system bus closed: removable media are no longer followed");
                Vec::new()
            }
            Message::MediaWatched(volumes) => match volumes.take() {
                Some(volumes) => self.media_watched(volumes),
                None => Vec::new(),
            },
            Message::MediaCalled(called) => match called.take() {
                Some(called) => self.media_called(called),
                None => Vec::new(),
            },
            Message::MountAgain(pending) => match pending.take() {
                Some(pending) => self.mount_again(pending),
                None => Vec::new(),
            },
            Message::Launch(event) => self.launch_event(event),
            Message::Places => {
                self.session.places_woken();
                vec![woken(&self.session.places_watch.woken, Message::Places)]
            }
            Message::PlacesRead(fresh) => self.session.places_read(fresh),
            Message::MountsRead(read) => self.session.mounts_read(read),
            Message::TrashLooked(changed) => {
                if changed {
                    let session = &self.session;
                    for window in self.windows.values_mut() {
                        window.trash_looked(session);
                    }
                }
                Vec::new()
            }
            // the windows draw the thumbnails made
            Message::Thumbnails => {
                vec![woken(&self.session.thumbnails_woken, Message::Thumbnails)]
            }
            Message::Window(id, event) => {
                let before = self.windows.get(&id).map(BrowserWindow::tally);
                let commands = self.window_event(id, event);
                if let Some(window) = self.windows.get_mut(&id) {
                    window.follow_view(&self.session);
                }
                if let (Some(before), Some(window)) = (before, self.windows.get_mut(&id)) {
                    let after = window.tally();
                    window.announcement.follow(before, after);
                }
                self.want_thumbnails(id);
                commands
            }
        }
    }

    /// Each preview window follows whether its item is ready to show: one still waited for starts a wait that ends
    /// [`file_preview::LOADING_DELAY`] later, when the window shows that it is loading. An item found ready is looked at
    /// again only after a message that can change it (`recheck`).
    fn follow_peeks(&mut self, recheck: peek::Recheck) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        for (&id, window) in &mut self.windows {
            let Some(look) = window.peek.as_ref() else {
                continue;
            };
            if !recheck.wants(id, look) {
                continue;
            }
            let entry = window
                .browser
                .entries()
                .iter()
                .find(|e| e.path == look.browse.current());
            let Some(entry) = entry else { continue };
            let size = peek::ready_size(&self.session, &window.texts, entry);
            let look = window.peek.as_mut().expect("checked above");
            if let Some(wait) = look.follow_ready(size) {
                commands.push(Command::After {
                    delay: std::time::Duration::from_secs_f64(crate::preview::LOADING_DELAY),
                    message: Message::Window(id, Event::Peek(PeekEvent::Waited(wait))),
                });
            }
        }
        commands
    }

    /// The work and the watches of every window's listings and the thumbnails' jobs, as I/O tasks and futures on the
    /// UI loop, and the session's kept listings whose work ended let go.
    fn io_work(&mut self) -> Vec<Command<Message>> {
        self.session.listings.sweep();
        let (mut works, mut watches, mut lookups) = (Vec::new(), Vec::new(), Vec::new());
        for window in self.windows.values_mut() {
            let browsers = std::iter::once((&mut window.browser, &mut window.columns)).chain(
                window
                    .parked
                    .values_mut()
                    .map(|tab| (&mut tab.browser, &mut tab.columns)),
            );
            for (browser, columns) in browsers {
                for (w, f) in [browser.take_work(), columns.take_work()] {
                    works.extend(w);
                    watches.extend(f);
                }
                lookups.extend(browser.take_lookups());
            }
        }
        let (w, f) = self.session.places_watch.work();
        works.extend(w);
        watches.extend(f);
        let watched = watches.into_iter().map(|watch| {
            let follow = watch.follow();
            Command::Perform(Box::pin(async move {
                follow.await;
                Message::Worked
            }))
        });
        let run = works.into_iter().map(|work| {
            cupertino_kit::task::io(
                cupertino_kit::files::model::WORK_DEADLINE,
                move || work.run(),
                |()| Message::Worked,
            )
        });
        let places = self.session.places_task();
        let mounts = self.session.mounts_task();
        let thumbnails = std::iter::from_fn(|| self.session.icons.take_job()).map(|job| {
            cupertino_kit::task::io(JOB_DEADLINE, move || job.run(), |()| Message::Worked)
        });
        let found = lookups.into_iter().map(reads::look_up);
        let spaces: Vec<Command<Message>> = self
            .windows
            .iter_mut()
            .flat_map(|(&id, window)| {
                let reads = window.texts.reads(&window.wake);
                window.space_task(id).into_iter().chain(reads)
            })
            .collect();
        watched
            .chain(run)
            .chain(thumbnails)
            .chain(found)
            .chain(spaces)
            .chain(places)
            .chain(mounts)
            .collect()
    }

    /// Exit once no browser window is open, no file operation runs and the progress window lists nothing, after the
    /// notices posted are sent.
    fn exit_when_done(&mut self) -> Vec<Command<Message>> {
        let jobs = &self.session.jobs;
        if self.exiting
            || self.awaiting_call
            || self.buses.is_some()
            || self.launching.is_some()
            || self.first.is_some()
            || !self.look_ups.is_empty()
            || !self.windows.is_empty()
            || !jobs.running.is_empty()
            || jobs.visible()
        {
            return Vec::new();
        }
        self.exiting = true;
        // first, so a launch racing the exit is refused at once and starts a Files of its own
        let release = self.session.instance.take().map(|instance| {
            let release = instance.release();
            Command::Perform(Box::pin(async move {
                release.await;
                Message::Worked
            }))
        });
        let sent = self.session.notices.sent();
        release
            .into_iter()
            .chain([
                Command::Perform(Box::pin(async move {
                    sent.await;
                    Message::NoticesSent
                })),
                Command::After {
                    delay: EXIT_NOTICES,
                    message: Message::NoticesSent,
                },
            ])
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
                let (previewed, points) = window.previewed();
                icons.want_previews(id, previewed, points);
            }
            None => icons.forget(id),
        }
    }

    /// Show `browser` in a window of its own.
    fn add_window(
        &mut self,
        browser: Browser,
        woken: async_channel::Receiver<()>,
        wake: Wake,
    ) -> Command<Message> {
        let id = self.next_window;
        self.next_window += 1;
        let waiting = self::woken(&woken, Message::Window(id, Event::Woken));
        let prefs = self.session.prefs.get();
        let mut window = BrowserWindow {
            browser,
            woken,
            wake,
            tabs: TabStrip::new(),
            parked: HashMap::new(),
            tab_bar: false,
            path_bar: prefs.path_bar,
            status_bar: prefs.status_bar,
            available: None,
            mounted_read_only: false,
            space_due: false,
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
            confirm_empty: false,
            trash_dir: self.session.trash.home_trash().join("files"),
            trash_look: self.session.trash_look.clone(),
            volumes_hold_trash: false,
            view: prefs.view,
            context: None,
            pressed: None,
            pressed_place: None,
            sidebar_focused: false,
            sidebar_row: None,
            place_asked: None,
            band: None,
            list_presses: Default::default(),
            announcement: announce::Announcement::default(),
            items_focused: false,
            focus_when_listed: None,
            springing: spring::Springing::default(),
            columns: columns::ColumnsView::new(prefs.column_width, self.session.listings.clone()),
            texts: peek::Texts::default(),
            peek: None,
            tag_editor: None,
            share: None,
        };
        window.entered();
        self.windows.insert(id, window);
        waiting
    }

    /// Open a window on each of `folders`.
    fn open_windows(&mut self, folders: Vec<PathBuf>) -> Vec<Command<Message>> {
        folders
            .into_iter()
            .map(|folder| self.open_window(folder).1)
            .collect()
    }

    /// Open a window on `folder`: its id and what wakes it. It shows the folder once its model has read it.
    fn open_window(&mut self, folder: PathBuf) -> (u64, Command<Message>) {
        let session = &self.session;
        let (browser, woken, wake) = open_browser(
            folder,
            session.types.clone(),
            session.recents.clone(),
            session.listings.clone(),
        );
        let id = self.next_window;
        (id, self.add_window(browser, woken, wake))
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
        let copy_only = window.items_stay(&self.session);
        vec![dragged::start(id, window_key(id), grab, entries, copy_only)]
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
        let copy_only = window.items_stay(&self.session);
        vec![dragged::start_rows(
            id,
            window_key(id),
            grab,
            entries,
            icon,
            copy_only,
        )]
    }

    /// Put `paths` dropped on window `id` where they go, as a file operation: into the trash, or moved or copied into
    /// a folder as [`drops::chosen`] says. A name the folder has is given a free copy name. The drop stays open until
    /// the operation ends; then its source hears what was done ([`dropped`]), so it never lets go of an item before
    /// Files has it.
    fn drop_items(
        &mut self,
        id: u64,
        token: cupertino_kit::platform::DropToken,
        to: &Destination,
        paths: Vec<PathBuf>,
        agreed: cupertino_kit::platform::DropAction,
        mods: cupertino_kit::platform::Mods,
    ) -> Vec<Command<Message>> {
        if paths.is_empty() {
            return vec![Command::FinishDrop(token, None)];
        }
        let work = match to {
            Destination::Trash => transfer::Work::Trash {
                sources: paths,
                trash: self.session.trash.clone(),
            },
            Destination::Folder(dir) => match drops::chosen(&paths, to, agreed, mods) {
                Ok(action) => transfer::Work::Transfer {
                    sources: paths,
                    dest: dir.clone(),
                    action,
                },
                Err(e) => {
                    let items: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
                    self.session
                        .failed(Action::Drop(DropAction::Move), &items, &e);
                    return vec![Command::FinishDrop(token, None)];
                }
            },
        };
        let mut commands = vec![Command::HoldDrop(token)];
        commands.extend(self.start_job(id, work, Some(token)));
        commands
    }

    /// Start `work` for window `id`, which announces it and shows its row; for `drop`, if it is one.
    fn start_job(
        &mut self,
        id: u64,
        work: transfer::Work,
        drop: Option<cupertino_kit::platform::DropToken>,
    ) -> Vec<Command<Message>> {
        let (job, commands) = self.session.jobs.start(work, id, drop);
        let title = job.transfer.title();
        if let Some(window) = self.windows.get_mut(&id) {
            window.announcement.say(announce::About::Transfer, title);
        }
        commands
    }

    /// A file operation reported, or the progress window was used.
    fn job_event(&mut self, event: JobEvent) -> Vec<Command<Message>> {
        let jobs = &mut self.session.jobs;
        match event {
            JobEvent::Reported(jid) => match jobs.poll(jid) {
                jobs::Polled::Gone => Vec::new(),
                jobs::Polled::Running(slice) => slice,
                jobs::Polled::Ended(job, outcome, seen) => self.job_ended(*job, outcome, seen),
            },
            JobEvent::Stop(jid) => {
                jobs.stop(jid);
                Vec::new()
            }
            JobEvent::Due(jid) => {
                jobs.due(jid);
                Vec::new()
            }
            JobEvent::Show => {
                jobs.show();
                Vec::new()
            }
            JobEvent::Hide => {
                jobs.hide();
                Vec::new()
            }
            JobEvent::Scroll(scroll) => {
                jobs.scroll = scroll;
                Vec::new()
            }
        }
    }

    /// A file operation ended. The window that started it, if still open, says so, selects what arrived where it
    /// shows it (or edits the name of the folder New Folder with Selection made) and reads its free space again. A
    /// failure is listed in the progress window, which opens; a stop is none. The end is one notification when it
    /// failed, when the operation ran long enough to show the progress window, or when neither a browser window nor
    /// the progress window was shown (`seen`); an operation stopped without failing says nothing. A drop's source hears
    /// what was done.
    fn job_ended(
        &mut self,
        job: jobs::Job,
        outcome: transfer::Outcome,
        seen: bool,
    ) -> Vec<Command<Message>> {
        let jobs::Job {
            mut transfer,
            origin,
            drop,
            lasted,
        } = job;
        let mut outcome = outcome;
        if let Some(took) = outcome.took.take() {
            transfer.took(took);
        }
        let failures: Vec<_> = outcome.failures(transfer.sources()).collect();
        let stopped = outcome.stopped();
        let text = transfer.ended(stopped, failures.len());
        let failure = failures.first().map(|(_, e)| jobs::Failure {
            id: transfer.id,
            title: text.clone(),
            reason: notice::explained(e),
        });
        let session = &mut self.session;
        let unseen = !seen && self.windows.is_empty();
        if failure.is_some() || (!stopped && (lasted || unseen)) {
            let reason = failure.as_ref().map(|f| f.reason.as_str());
            session
                .notices
                .post_clickable(notice::finished(&text, reason), transfer.id);
            session.noticed.push_back((transfer.id, failure.clone()));
            if session.noticed.len() > MAX_NOTICED {
                session.noticed.pop_front();
            }
        }
        if let Some(failure) = failure {
            session.jobs.failed(failure);
        }
        let mut commands = Vec::new();
        if let Some(window) = self.windows.get_mut(&origin) {
            window.announcement.say(announce::About::Transfer, text);
            let here = |p: &Path| p.parent() == Some(window.browser.dir());
            match outcome.folder.clone() {
                Some(folder) if here(&folder) => {
                    commands = window.rename_new_folder(origin, folder)
                }
                Some(_) => {}
                None => {
                    if let Some(path) = outcome.arrived().filter(|p| here(p)) {
                        window.browser.select_when_appears(path.to_path_buf());
                    }
                }
            }
            window.read_available();
        }
        if matches!(
            transfer.work,
            transfer::Work::Trash { .. } | transfer::Work::EmptyTrash { .. }
        ) {
            // the first item trashed makes the trash, and the last one deleted leaves it empty: its place comes at once
            session.follow_places();
        }
        commands.extend(
            drop.map(|token| Command::FinishDrop(token, dropped(&transfer.work, &outcome))),
        );
        commands
    }

    fn window_event(&mut self, id: u64, event: Event) -> Vec<Command<Message>> {
        let event = match event {
            Event::Place(index) => return self.open_place(id, index),
            Event::Accessory(key) => return self.accessory(&key),
            Event::Unlock => return self.unlock(id),
            Event::EmptyTrash => return self.empty_trash(id),
            Event::MenuCommand(command) => return self.menu_command(id, command),
            Event::DragOut(grab) => return self.drag_out(id, &grab),
            Event::DragRows(grab) => return self.drag_rows(id, &grab),
            Event::Dragged(progress) => {
                if matches!(
                    progress,
                    DragProgress::Dropped | DragProgress::Finished | DragProgress::Cancelled
                ) {
                    self.session.dragged.clear();
                }
                // a cancelled drag takes the window it came from back to where it was
                if progress == DragProgress::Cancelled {
                    if let Some(window) = self.windows.get_mut(&id) {
                        window.spring_back();
                    }
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
                // the drop keeps the folder a drag opened, also while its operation holds the drop open
                if let Some(window) = self.windows.get_mut(&id) {
                    window.spring_kept();
                }
                return match to {
                    Some(to) => self.drop_items(id, token, &to, paths, action, mods),
                    None => vec![Command::FinishDrop(token, None)],
                };
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
                let changed = polled.changed;
                let not_opened = polled.not_opened.clone();
                window.polled(polled, session);
                if window.not_opened(not_opened, session) {
                    self.windows.remove(&id);
                    return Vec::new();
                }
                let columns = window.columns.poll();
                window.texts.poll();
                // the tabs not shown follow their folders too
                for tab in window.parked.values_mut() {
                    let polled = tab.browser.poll();
                    for error in polled.errors.into_iter().chain(polled.not_opened) {
                        eprintln!("cupertino-files: {error}");
                    }
                    tab.columns.poll();
                }
                if changed || columns {
                    window.now = (window.clock)();
                    window.read_available();
                    session.learn_tags(window.browser.entries());
                }
                window.spring_settle();
                let mut commands = window.listed(id);
                if let Some(path) = window.browser.take_revealed() {
                    commands.extend(window.focus_item(id, &path));
                }
                commands.push(woken(&window.woken, Message::Window(id, Event::Woken)));
                return commands;
            }
            Event::Space(dir, read) => window.space_read(&dir, read),
            Event::Place(_)
            | Event::Accessory(_)
            | Event::Unlock
            | Event::EmptyTrash
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
            Event::PlacePress(place) => window.pressed_place = Some(place),
            Event::SidebarMenu(at) => window.open_place_menu(at, session),
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
            Event::AskEmptyTrash => window.ask_empty_trash(session),
            Event::EmptyTrashCancelled => window.confirm_empty = false,
            Event::SpringOver(folder) => return window.spring_over(id, folder),
            Event::SpringDue(generation, step) => return window.spring_due(id, generation, step),
            Event::DragLeft => window.spring_back(),
            Event::Navigate(Navigation::Back) => window.step(Step::Back),
            Event::Navigate(Navigation::Forward) => window.step(Step::Forward),
            Event::Select(selection) => window.select(&selection, session),
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
            Event::Launch {
                path,
                is_dir,
                with,
                token,
            } => session.launch(&path, is_dir, with.as_deref(), token.as_deref()),
            Event::Original(link, target) => window.original_found(&link, target, session),
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
            Event::SidebarFocus(focused) => window.sidebar_focused = focused,
            Event::ItemsFocus(focused) => window.items_focused = focused,
            Event::Band(band) => window.band = band,
            Event::Toggle(toggle) => {
                if !toggle.expanded {
                    return window.collapse(id, toggle.index);
                }
                window.browser.expand(toggle.index);
            }
            Event::FocusRow(index) => {
                let Some(entry) = window.browser.entries().get(index) else {
                    return Vec::new();
                };
                let key = row_key(id, &entry.path);
                let mut alone = Selection::new([], None, None);
                alone.select_only(index);
                window.select(&alone, session);
                return vec![Command::Perform(Box::pin(async move {
                    Message::Window(id, Event::FocusKey(key))
                }))];
            }
            Event::FocusKey(key) => return vec![Command::Focus(key)],
            Event::FollowKey(key) => return vec![Command::MoveFocus(key)],
            Event::Popup(popup) => window.open_popup(popup, session),
            Event::PathSegment(path) => {
                if path != window.browser.dir() {
                    window.go(path);
                }
            }
            Event::RenameItem(index) => {
                let state = window.menu_state(session);
                if menu_bar::enabled(&state).contains(&(MenuAction::Rename, true)) {
                    return window.begin_rename(id, index);
                }
            }
            Event::ColumnScroll(folder, scroll) => window.columns.scrolled(folder, scroll),
            Event::ColumnPick {
                folder,
                index,
                secondary,
            } => {
                if window.rename.is_some() {
                    let _ = window.end_rename(session);
                }
                if window.columns.pick(&mut window.browser, &folder, index) {
                    window.entered();
                }
                if secondary {
                    window.pressed = Some(window.browser.selection().indices().next());
                }
                // the keyboard follows into the column of the folder now shown, for typing and the arrows
                let picked = window.browser.selection().indices().next();
                if let Some(path) = picked
                    .and_then(|i| window.browser.entries().get(i))
                    .map(|e| e.path.clone())
                {
                    return window.follow_item(id, &path);
                }
            }
            Event::ColumnStep(by) => {
                if window.rename.is_some() {
                    return Vec::new();
                }
                if let Some(path) = window.columns.step(&mut window.browser, by) {
                    window.entered();
                    window.columns.sync(&window.browser, &mut window.scroll);
                    return window.follow_item(id, &path);
                }
            }
            Event::ColumnResize { index, width, all } => {
                window.columns.chain.resize(index, width, all);
                window.columns.resized = Some((index, all));
            }
            Event::ColumnResized => {
                // kept once the divider is let go, not at every move
                if let Some((index, all)) = window.columns.resized.take() {
                    let width = if all {
                        window.columns.chain.default_width()
                    } else {
                        window.columns.chain.width(index)
                    };
                    session.remember_column_width(width);
                }
            }
            Event::PreviewScroll(scroll) => window.columns.preview_scroll = scroll,
            Event::Peek(event) => return self.peek(id, event),
            Event::Tags(TagEvent::Filter(change)) => {
                if let Some(editor) = window.tag_editor.as_mut() {
                    editor.filter = change.text;
                    editor.revision = change.revision;
                }
            }
            Event::Tags(TagEvent::Toggle(name)) => window.toggle_named_tag(&name, session),
            Event::Tags(TagEvent::Submit) => {
                // a comma parts tags, as the attribute keeps them, so "a, b" adds two
                let typed = window
                    .tag_editor
                    .as_ref()
                    .map(|e| tags::parse_tag_list(e.filter.as_bytes()))
                    .unwrap_or_default();
                if !typed.is_empty() {
                    for name in &typed {
                        window.add_tag(name, session);
                    }
                    if let Some(editor) = window.tag_editor.as_mut() {
                        editor.filter.clear();
                        editor.revision += 1;
                    }
                }
            }
            Event::Tags(TagEvent::Close) => window.tag_editor = None,
            Event::Share(ShareEvent::Close) => window.share = None,
            Event::Share(ShareEvent::Scroll(scroll)) => {
                if let Some(share) = window.share.as_mut() {
                    share.scroll = scroll;
                }
            }
            Event::Share(ShareEvent::ShowAll) => {
                if let Some(share) = window.share.as_mut() {
                    share.all = true;
                }
            }
            Event::Share(ShareEvent::Copy) => {
                window.share = None;
                return self.menu(id, MenuAction::Copy);
            }
            Event::Share(ShareEvent::Open(app)) => {
                window.share = None;
                return self.menu_command(id, MenuCommand::OpenWith(app));
            }
            Event::Tags(TagEvent::Scroll(scroll)) => {
                if let Some(editor) = window.tag_editor.as_mut() {
                    editor.scroll = scroll;
                }
            }
            Event::TagPlace(name) => {
                window.browser.set_filter(&name);
                window.search = name;
                window.search_revision += 1;
                window.follow_search();
            }
            Event::Tab(TabEvent::Select(tab)) => window.show_tab(tab),
            Event::Tab(TabEvent::Move(tab, to)) => window.tabs.move_to(tab, to),
            Event::Tab(TabEvent::New) => return self.menu(id, MenuAction::NewTab),
            Event::Tab(TabEvent::Close(tab)) => {
                if tab == window.tabs.active_id() {
                    return self.menu(id, MenuAction::CloseTab);
                }
                window.tabs.close(tab);
                window.parked.remove(&tab);
            }
        }
        Vec::new()
    }

    /// What the preview window of window `id` asked for.
    fn peek(&mut self, id: u64, event: PeekEvent) -> Vec<Command<Message>> {
        let session = &self.session;
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        let Some(look) = window.peek.as_mut() else {
            return Vec::new();
        };
        match event {
            PeekEvent::Waited(wait) => look.waited(wait),
            // the popup shrinks back into its item as the view stops declaring it
            PeekEvent::Close | PeekEvent::Closed => window.peek = None,
            PeekEvent::Scroll(scroll) => look.scroll = scroll,
            PeekEvent::FullScreen => {
                // in full screen the preview is a window of its own; back from it, the popup again
                look.full_screen = !look.full_screen;
                if look.full_screen {
                    return vec![Command::Window(
                        peek::full_screen_key(id),
                        WindowRequest::Fullscreen(true),
                    )];
                }
            }
            PeekEvent::Open => {
                let path = look.browse.current().to_path_buf();
                let is_dir = window.is_dir(&path);
                return session.open_paths(id, vec![open::Item { path, is_dir }], None);
            }
            PeekEvent::Step(by) if look.browse.len() > 1 => {
                look.browse.step(by);
            }
            PeekEvent::Step(by) => {
                // one item: the arrows move the window's selection, which the preview follows
                let count = window.browser.entries().len();
                let at = window.browser.selection().indices().next();
                if let Some(at) = at.filter(|_| count > 0) {
                    let to = (at as isize + by).clamp(0, count as isize - 1) as usize;
                    window.browser.select([to], Some(to), Some(to));
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
                self.session.pasteboard_stays = window.items_stay(&self.session);
                return Vec::new();
            }
            MenuAction::Paste | MenuAction::MoveHere => {
                let moving = action == MenuAction::MoveHere;
                let session = &mut self.session;
                if session.pasteboard.is_empty() || (moving && session.pasteboard_stays) {
                    return Vec::new();
                }
                let (sources, action) = match moving {
                    true => (std::mem::take(&mut session.pasteboard), DropAction::Move),
                    false => (session.pasteboard.clone(), DropAction::Copy),
                };
                let dest = window.browser.dir().to_path_buf();
                let work = transfer::Work::Transfer {
                    sources,
                    dest,
                    action,
                };
                return self.start_job(id, work, None);
            }
            MenuAction::Duplicate => {
                let sources = window.selected_paths();
                if sources.is_empty() {
                    return Vec::new();
                }
                return self.start_job(id, transfer::Work::Duplicate { sources }, None);
            }
            MenuAction::NewFolderWithSelection => {
                let sources = window.selected_paths();
                let dir = window.browser.dir().to_path_buf();
                return self.start_job(id, transfer::Work::NewFolderWith { dir, sources }, None);
            }
            MenuAction::ShowProgress => {
                self.session.jobs.show();
                return Vec::new();
            }
            _ => {}
        }
        let session = &mut self.session;
        match action {
            MenuAction::Copy
            | MenuAction::Paste
            | MenuAction::MoveHere
            | MenuAction::Duplicate
            | MenuAction::NewFolderWithSelection
            | MenuAction::ShowProgress => {}
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
                    return self.eject(place);
                }
            }
            MenuAction::Open => return window.open_selection(id, session),
            MenuAction::SelectAll => window.browser.select_all(),
            MenuAction::SortBy(key, ascending) => window.browser.set_sort(key, ascending),
            MenuAction::TogglePathBar => window.path_bar = !window.path_bar,
            MenuAction::ToggleStatusBar => window.status_bar = !window.status_bar,
            MenuAction::ToggleSidebar => {
                window.split.collapsed = match window.split.collapsed {
                    Some(_) => None,
                    None => Some(SplitPane::Leading),
                }
            }
            MenuAction::Back => window.step(Step::Back),
            MenuAction::Forward => window.step(Step::Forward),
            MenuAction::EnclosingFolder => window.step(Step::Parent),
            MenuAction::Recents => {
                window.go(cupertino_kit::files::recents::location().to_path_buf())
            }
            MenuAction::Home => window.go(session.home.clone()),
            MenuAction::MoveToTrash => {
                let commands = window.trash_selection(session).into_iter().collect();
                // the first item trashed makes the trash: its place comes at once
                session.follow_places();
                return commands;
            }
            MenuAction::PutBack => window.put_back(session),
            MenuAction::NewFolder => match operations::new_folder(window.browser.dir()) {
                Ok(path) => return window.rename_new_folder(id, path),
                Err(e) => session.failed(Action::NewFolder, &[], &e),
            },
            MenuAction::Rename => {
                let first = window.browser.selection().indices().next();
                if let Some(index) = first {
                    return window.begin_rename(id, index);
                }
            }
            MenuAction::MakeAlias => {
                let paths = window.selected_paths();
                let made = paths.iter().map(|p| (p, operations::make_alias(p)));
                session.failed_some(Action::MakeAlias, made);
            }
            MenuAction::ShowOriginal => return window.show_original(id),
            MenuAction::View(view) if view != window.view => {
                // the folders opened in the list close with it
                window.browser.collapse_all();
                window.band = None;
                // a press or a band of the list ends with it: its nodes go without a release
                window.list_presses = Presses::default();
                window.view = view;
                window.scroll.offset = cupertino_kit::geometry::Point::ZERO;
                // the gallery shows an item: the first, when none is selected
                if view == ViewMode::Gallery && window.browser.selection().is_empty() {
                    if let Some(first) = gallery::item(window) {
                        window.browser.select([first], Some(first), Some(first));
                    }
                }
            }
            MenuAction::View(_) => {}
            MenuAction::Peek => window.toggle_peek(),
            MenuAction::NewTab => window.new_tab(session),
            MenuAction::CloseTab => {
                if !window.close_tab() {
                    self.windows.remove(&id);
                }
            }
            MenuAction::NextTab => {
                let tab = window.tabs.clone().cycle(1);
                window.show_tab(tab);
            }
            MenuAction::PreviousTab => {
                let tab = window.tabs.clone().cycle(-1);
                window.show_tab(tab);
            }
            MenuAction::ToggleTabBar => window.tab_bar = !window.tab_bar_shown(),
        }
        if matches!(
            action,
            MenuAction::TogglePathBar | MenuAction::ToggleStatusBar | MenuAction::View(_)
        ) {
            if let Some(window) = self.windows.get(&id) {
                self.session.remember(window);
            }
        }
        if action == MenuAction::PutBack {
            // the item put back may have emptied the trash: its place follows at once
            self.session.follow_places();
        }
        Vec::new()
    }

    /// Do the row `command` of the context menu of the window `id`.
    fn menu_command(&mut self, id: u64, command: MenuCommand) -> Vec<Command<Message>> {
        let Some(action) = command.action() else {
            match command {
                MenuCommand::OpenWith(app) => {
                    let Some(window) = self.windows.get(&id) else {
                        return Vec::new();
                    };
                    return self
                        .session
                        .open_paths(id, window.selected_items(), Some(app));
                }
                MenuCommand::ToggleTag(color) => {
                    if let Some(window) = self.windows.get(&id) {
                        window.toggle_tag(color, &self.session);
                    }
                }
                MenuCommand::EmptyTrash => {
                    if let Some(window) = self.windows.get_mut(&id) {
                        window.ask_empty_trash(&self.session);
                    }
                }
                MenuCommand::OpenPlace(place) => return self.open_place(id, place),
                MenuCommand::OpenPlaceInNewWindow(place) => {
                    let folder = self
                        .session
                        .places
                        .get(place)
                        .and_then(|p| p.folder())
                        .map(Path::to_path_buf);
                    return self.open_windows(folder.into_iter().collect());
                }
                MenuCommand::EjectPlace(place)
                    if self
                        .session
                        .places
                        .get(place)
                        .is_some_and(|p| p.ejectable()) =>
                {
                    self.eject(place);
                }
                _ => {}
            }
            return Vec::new();
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

    /// Empty the trash as a file operation, once its question was answered: it runs on a worker with the others,
    /// the progress window lists it and a failure stays there.
    fn empty_trash(&mut self, id: u64) -> Vec<Command<Message>> {
        let Some(window) = self.windows.get_mut(&id) else {
            return Vec::new();
        };
        window.confirm_empty = false;
        // what the home trash holds now; the worker lists it again with the mounted volumes', which are not read
        // on this thread, and says what it took
        let sources = self.session.trash.home_entries();
        if sources.is_empty() && !self.session.trash_look.volumes_held() {
            return Vec::new();
        }
        let work = transfer::Work::EmptyTrash {
            sources,
            trash: self.session.trash.clone(),
        };
        self.start_job(id, work, None)
    }

    /// Open place `index` in the window `id`, as a click on its row does: a folder is shown, a removable device is
    /// opened ([`Model::open_device`]).
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
            window.go(folder);
            if window.browser.is_waiting() {
                window.place_asked = Some(row.clone());
            }
            window.sidebar_row = Some(row);
            return Vec::new();
        }
        window.sidebar_row = Some(row);
        match index.checked_sub(session.standard_places) {
            Some(row) => self.open_device(id, row),
            None => Vec::new(),
        }
    }
}

impl Session {
    /// The look at the mounted volumes' trash asked for, as an I/O task, unless one is still out.
    fn look_at_trash(&self) -> Option<Command<Message>> {
        let look = self.trash_look.next()?;
        Some(cupertino_kit::task::io(
            cupertino_trash::LOOK_DEADLINE,
            look,
            Message::TrashLooked,
        ))
    }

    /// What waits for the next click on a notice of a file operation's end; nothing without a session bus.
    fn notice_clicks(&self) -> Option<Command<Message>> {
        let clicks = self.notices.clicks()?.clone();
        Some(Command::Perform(Box::pin(async move {
            Message::Noticed(clicks.recv().await.ok())
        })))
    }

    /// `window`'s view and bars become what a new window starts with, kept in the prefs file when there is one.
    fn remember(&self, window: &BrowserWindow) {
        let prefs = WindowPrefs {
            view: window.view,
            path_bar: window.path_bar,
            status_bar: window.status_bar,
            column_width: self.prefs.get().column_width,
        };
        self.keep(prefs);
    }

    /// A column was made `width` wide: new windows' columns start at that width.
    fn remember_column_width(&self, width: f64) {
        let prefs = WindowPrefs {
            column_width: width,
            ..self.prefs.get()
        };
        self.keep(prefs);
    }

    /// `prefs` become what a new window starts with, kept in the prefs file when there is one.
    fn keep(&self, prefs: WindowPrefs) {
        if self.prefs.replace(prefs) == prefs {
            return;
        }
        if let Some(file) = &self.prefs_file {
            if let Err(e) = prefs.save(file) {
                eprintln!(
                    "cupertino-files: cannot keep the window's view in {}: {e}",
                    file.display()
                );
            }
        }
    }

    /// The tags `entries` carry join those the sidebar lists.
    fn learn_tags(&self, entries: &[Entry]) {
        let mut known = self.known_tags.borrow_mut();
        for tag in entries.iter().flat_map(|e| &e.tags) {
            if !known.contains(tag) {
                known.insert(tag.clone());
            }
        }
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

    /// Read the standard places again, keeping the removable media's after them, and watch the trash once it exists.
    fn show_media(&mut self) {
        self.places.truncate(self.standard_places);
        self.places.extend(self.media.places());
    }
}

impl BrowserWindow {
    /// The window's title: its folder's name, or in the column view the folder selected in the last column, whose
    /// items the column after it shows.
    pub(super) fn title(&self, session: &Session) -> String {
        let child = (self.view == ViewMode::Columns)
            .then(|| self.columns.chain(&self.browser).child)
            .flatten();
        let dir = child.as_deref().unwrap_or(self.browser.dir());
        cupertino_kit::files::places::window_title(dir, &session.places)
    }

    /// After each event: the column view follows the folder and the selection, the preview window follows a single
    /// selected item and closes once its items are gone, and the texts previewed are read.
    fn follow_view(&mut self, session: &Session) {
        if self.view == ViewMode::Columns {
            self.columns.sync(&self.browser, &mut self.scroll);
        }
        let selected = self.selected_paths();
        if let Some(look) = self.peek.as_mut() {
            // a new selection is previewed from its first item
            if !selected.is_empty() && selected != look.browse.items() {
                if let Some(browse) = Browse::new(selected.clone(), 0) {
                    look.browse = browse;
                }
            }
            let listed = |p: &Path| self.browser.entries().iter().any(|e| e.path == p);
            if !listed(look.browse.current()) {
                self.peek = None;
            }
        }
        if let Some(look) = self.peek.as_mut() {
            let current = look.browse.current().to_path_buf();
            let is_dir = self
                .browser
                .entries()
                .iter()
                .any(|e| e.is_dir && e.path == current);
            look.follow_app(session, &current, is_dir);
        }
        let (previewed, _) = self.previewed();
        let previewed: Vec<Entry> = previewed.cloned().collect();
        self.texts.follow(&previewed);
    }

    /// The items previewed: the preview column's file, the gallery's item and the preview window's item, and the side
    /// of their pictures in points.
    fn previewed(&self) -> (impl Iterator<Item = &Entry>, f64) {
        let entries = self.browser.entries();
        let column = (self.view == ViewMode::Columns)
            .then(|| self.columns.chain(&self.browser).preview)
            .flatten()
            .and_then(|i| entries.get(i));
        let gallery = (self.view == ViewMode::Gallery)
            .then(|| gallery::item(self))
            .flatten()
            .and_then(|i| entries.get(i));
        let window = self
            .peek
            .as_ref()
            .and_then(|look| entries.iter().find(|e| e.path == look.browse.current()));
        (
            column.into_iter().chain(gallery).chain(window),
            peek::PICTURE,
        )
    }

    /// Whether the tab bar shows: with more than one tab, or when View › Show Tab Bar asked for it.
    fn tab_bar_shown(&self) -> bool {
        self.tab_bar || self.tabs.len() > 1
    }

    /// Open a tab after the one shown, on the folder shown, in the same view, and show it.
    fn new_tab(&mut self, session: &Session) {
        let browser = Browser::open(
            self.browser.dir().to_path_buf(),
            self.browser.sort(),
            session.recents.clone(),
            session.types.clone(),
            Arc::clone(&self.wake),
            session.listings.clone(),
        );
        // the tab parked forgets a folder it asked for
        self.browser.stop_waiting();
        let shown = self.tabs.active_id();
        let mut tab = tabs::Tab {
            browser,
            view: self.view,
            scroll: ScrollState {
                viewport: self.scroll.viewport,
                ..NO_SCROLL
            },
            search: String::new(),
            search_revision: 0,
            available: self.available,
            mounted_read_only: self.mounted_read_only,
            columns: columns::ColumnsView::new(
                self.columns.chain.default_width(),
                session.listings.clone(),
            ),
        };
        self.tabs.open();
        self.swap_tab(&mut tab);
        self.parked.insert(shown, tab);
        self.entered();
    }

    /// Close the tab shown and show the next one, else the one before; `false` when it was the last tab, so the window
    /// closes instead.
    fn close_tab(&mut self) -> bool {
        let shown = self.tabs.active_id();
        let Some(next) = self.tabs.close(shown) else {
            return false;
        };
        if let Some(mut tab) = self.parked.remove(&next) {
            self.swap_tab(&mut tab);
        }
        self.left_tab();
        true
    }

    /// Show tab `id`, parking the one shown.
    fn show_tab(&mut self, id: TabId) {
        let shown = self.tabs.active_id();
        if id == shown || !self.tabs.select(id) {
            return;
        }
        let Some(mut tab) = self.parked.remove(&id) else {
            return;
        };
        // the tab parked forgets a folder it asked for
        self.browser.stop_waiting();
        self.swap_tab(&mut tab);
        self.parked.insert(shown, tab);
        self.left_tab();
    }

    /// Swap the tab shown for `tab`.
    fn swap_tab(&mut self, tab: &mut tabs::Tab) {
        std::mem::swap(&mut self.browser, &mut tab.browser);
        std::mem::swap(&mut self.view, &mut tab.view);
        std::mem::swap(&mut self.scroll, &mut tab.scroll);
        std::mem::swap(&mut self.search, &mut tab.search);
        std::mem::swap(&mut self.search_revision, &mut tab.search_revision);
        std::mem::swap(&mut self.available, &mut tab.available);
        std::mem::swap(&mut self.mounted_read_only, &mut tab.mounted_read_only);
        std::mem::swap(&mut self.columns, &mut tab.columns);
    }

    /// Another tab is shown: what belonged to the one left ends, and the one shown catches up with its folder.
    fn left_tab(&mut self) {
        self.rename = None;
        self.rename_when_listed = None;
        self.focus_when_listed = None;
        self.context = None;
        self.pressed = None;
        self.band = None;
        self.sidebar_row = None;
        self.peek = None;
        self.tag_editor = None;
        self.share = None;
        self.list_presses = Presses::default();
        let polled = self.browser.poll();
        for error in polled.errors {
            eprintln!("cupertino-files: {error}");
        }
        // the search field shows the tab's search; the revision moves on so the field takes it
        self.search_revision += 1;
        self.follow_search();
        self.now = (self.clock)();
    }

    /// Space, or File › Quick Preview: open the preview window on the selected items, or close it.
    fn toggle_peek(&mut self) {
        match self.peek.as_mut() {
            Some(_) => self.peek = None,
            None => {
                let selected = self.selected_paths();
                self.peek = Browse::new(selected, 0).map(peek::Peek::new);
            }
        }
    }

    /// Go to the folder `to`: at once when it is the folder shown, else once it opens ([`BrowserWindow::polled`]).
    fn go(&mut self, to: PathBuf) {
        self.go_selecting(to, None);
    }

    /// [`BrowserWindow::go`], selecting the item at `select` as soon as the listing shows it.
    fn go_selecting(&mut self, to: PathBuf, select: Option<PathBuf>) {
        self.place_asked = None;
        if self.browser.go(to, select) {
            self.entered();
        }
    }

    fn step(&mut self, step: Step) {
        self.place_asked = None;
        match step {
            Step::Back => self.browser.back(),
            Step::Forward => self.browser.forward(),
            Step::Parent => self.browser.parent(),
        };
    }

    /// What the browser's poll brought besides its listing: its errors on standard error, a folder asked for that
    /// opened entered, and the notice of one that did not, which leaves the window where it is: a folder gone to is
    /// "couldn't be opened" by its name, one of the history or the enclosing folder by its title.
    fn polled(&mut self, polled: Polled, session: &Session) {
        for error in polled.errors {
            eprintln!("cupertino-files: {error}");
        }
        if let Some(asked) = polled.entered {
            let row = self.place_asked.take().filter(|_| asked == Asked::Go);
            self.spring_entered(asked);
            if row.is_some() {
                self.sidebar_row = row;
            }
        }
        if let Some(unopened) = polled.unopened {
            self.place_asked = None;
            self.spring_unopened();
            match unopened.asked {
                Asked::Go => session.failed(Action::Open, &[&unopened.dir], &unopened.error),
                Asked::Back | Asked::Forward | Asked::Parent | Asked::Return => {
                    let name =
                        cupertino_kit::files::places::window_title(&unopened.dir, &session.places);
                    session
                        .notices
                        .post(notice::unopened(&name, &unopened.error));
                }
            }
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

    /// Ask for the free space of the folder's file system for the status bar: on entering a folder, whenever its
    /// listing changes, and after a transfer. An I/O task reads it ([`Event::Space`]); the last answer stays until then.
    fn read_available(&mut self) {
        self.space_due = true;
        self.volumes_hold_trash = self.in_trash() && self.trash_look.volumes_hold();
    }

    /// A look at the mounted volumes found their trash changed: the Empty button of a window on the trash, and the
    /// Empty Trash row of the Trash place's menu when it is open, follow.
    fn trash_looked(&mut self, session: &Session) {
        self.volumes_hold_trash = self.in_trash() && session.trash_look.volumes_held();
        if let Some(menu) = self.context.as_mut() {
            if matches!(menu.ring, Some(context::Ringed::Place(_))) {
                let full = session.trash.home_holds_items() || session.trash_look.volumes_held();
                for item in &mut menu.items {
                    if matches!(
                        item.kind,
                        crate::context_menu::ItemKind::Command(MenuCommand::EmptyTrash)
                    ) {
                        item.enabled = full;
                    }
                }
            }
        }
    }

    /// Whether the folder shown takes nothing new: its medium can't be written, or its file system is mounted
    /// read-only.
    /// The folder's items can be copied out but not moved: an application's entry in Applications, a file on a
    /// read-only medium.
    fn items_stay(&self, session: &Session) -> bool {
        self.browser.dir() == cupertino_kit::files::applications::location()
            || self.read_only(session)
    }

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

    /// Move the selected items to the trash; the listing drops them when the watcher reports it. Summing a trashed
    /// folder's files can take long: the I/O task that records the sizes.
    fn trash_selection(&self, session: &Session) -> Option<Command<Message>> {
        let paths = self.selected_paths();
        let mut sizes = Vec::new();
        let moved: Vec<_> = paths
            .iter()
            .map(|path| {
                let moved = session.trash.trash_unsized(path).map(|(item, size)| {
                    sizes.extend(size);
                    item
                });
                (path, moved)
            })
            .collect();
        session.failed_some(Action::Trash, moved);
        // each move is a rename; the sizes are summed on the I/O pool
        (!sizes.is_empty()).then(|| {
            cupertino_kit::task::io(
                TRASH_SIZES_DEADLINE,
                move || {
                    for size in sizes {
                        if let Err(e) = size.record() {
                            eprintln!(
                                "cupertino-files: cannot record a trashed folder's size: {e}"
                            );
                        }
                    }
                },
                |()| Message::Worked,
            )
        })
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
        let files = files
            .into_iter()
            .map(|path| open::Item {
                path,
                is_dir: false,
            })
            .collect();
        let commands = session.open_paths(id, files, None);
        if let Some(folder) = folder {
            self.go(folder);
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
            let label =
                cupertino_kit::files::places::window_title(self.browser.dir(), &session.places);
            (label, self.background_items(session, in_trash))
        };
        self.context = Some(context::OpenMenu {
            ring: on_item.then_some(context::Ringed::Items),
            anchor: MenuAnchor::At(at),
            label,
            items,
            open: Vec::new(),
        });
    }

    /// Whether the window shows the trash folder.
    fn in_trash(&self) -> bool {
        self.browser.dir() == self.trash_dir
    }

    /// Put up the question before the trash is emptied, unless neither the home trash nor a mounted volume's holds
    /// anything.
    fn ask_empty_trash(&mut self, session: &Session) {
        let holds_items =
            (self.in_trash() && !self.browser.entries().is_empty()) || session.trash_look.full();
        self.context = None;
        self.confirm_empty = holds_items;
    }

    /// Open the context menu of the sidebar's place that a secondary press was on, at `at`.
    fn open_place_menu(&mut self, at: cupertino_kit::geometry::Point, session: &Session) {
        let Some(index) = self.pressed_place.take() else {
            return;
        };
        let Some(place) = session.places.get(index) else {
            return;
        };
        let mut items = vec![crate::context_menu::MenuItem::command(
            "Open",
            MenuCommand::OpenPlace(index),
            true,
        )];
        if place.folder().is_some() {
            items.push(crate::context_menu::MenuItem::command(
                "Open in New Window",
                MenuCommand::OpenPlaceInNewWindow(index),
                true,
            ));
        }
        if place.ejectable() {
            items.push(crate::context_menu::MenuItem::separator());
            items.push(crate::context_menu::MenuItem::command(
                "Eject",
                MenuCommand::EjectPlace(index),
                true,
            ));
        }
        if place.kind == cupertino_kit::files::places::PlaceKind::Trash {
            items.push(crate::context_menu::MenuItem::separator());
            items.push(crate::context_menu::MenuItem::command(
                "Empty Trash",
                MenuCommand::EmptyTrash,
                session.trash_look.full(),
            ));
        }
        self.context = Some(context::OpenMenu {
            ring: Some(context::Ringed::Place(index)),
            anchor: MenuAnchor::At(at),
            label: place.name.clone(),
            items,
            open: Vec::new(),
        });
    }

    /// Open the menu of the toolbar's button `popup` below it.
    fn open_popup(&mut self, popup: Popup, session: &Session) {
        if self.rename.is_some() {
            let _ = self.end_rename(session);
        }
        if popup == Popup::Tags
            && (self.tag_editor.is_some() || !self.browser.selection().is_empty())
        {
            // the tags of the selection, edited in a popover; without a selection the menu shows them unavailable
            self.context = None;
            self.tag_editor = match self.tag_editor {
                Some(_) => None,
                None => Some(tag_editor::TagEditor::new()),
            };
            return;
        }
        if popup == Popup::Share {
            // the selection's apps and copy, in a popover; without a selection the button does nothing
            self.context = None;
            self.tag_editor = None;
            self.share = match (self.share.as_ref(), self.browser.selection().is_empty()) {
                (None, false) => Some(share_popover::ShareState::new()),
                _ => None,
            };
            return;
        }
        let in_trash = self.browser.dir() == session.trash.home_trash().join("files");
        let has_selection = !self.browser.selection().is_empty();
        let (label, items) = match popup {
            Popup::Share => return,
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
            Popup::Action | Popup::More if has_selection => {
                let subject = self.subject(in_trash);
                let items = menu_spec::item_menu(&subject, &self.selection_apps(session));
                ("Action".to_owned(), items)
            }
            Popup::Action | Popup::More => (
                "Action".to_owned(),
                self.background_items(session, in_trash),
            ),
        };
        self.context = Some(context::OpenMenu {
            ring: None,
            anchor: MenuAnchor::Below(view::popup_key(popup)),
            label,
            items,
            open: Vec::new(),
        });
    }

    /// The rows of the folder's menu, the row that empties the trash off while the trash holds nothing.
    fn background_items(
        &self,
        session: &Session,
        in_trash: bool,
    ) -> Vec<crate::context_menu::MenuItem> {
        let mut items = menu_spec::background_menu(&self.background(session, in_trash));
        let empty = self.browser.entries().is_empty();
        for item in &mut items {
            if matches!(
                item.kind,
                crate::context_menu::ItemKind::Command(MenuCommand::EmptyTrash)
            ) {
                item.enabled &= !empty;
            }
        }
        items
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
        let selected: Vec<&Entry> = self
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
            single_link: selected.len() == 1 && selected[0].kind == Kind::Symlink,
            in_trash,
            tags_on,
        }
    }

    /// The applications that open every selected item, for Open With.
    pub(super) fn selection_apps(&self, session: &Session) -> Vec<AppChoice> {
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
                        icon: item.entry.icon.clone(),
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
        let selected: Vec<&Entry> = self
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

    /// Take the tag `name` off every selected item when each carries it, else add it to those without it.
    fn toggle_named_tag(&self, name: &str, session: &Session) {
        let entries = self.browser.entries();
        let selected: Vec<&Entry> = self
            .browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
            .collect();
        if selected.iter().all(|e| carries(e, name)) {
            let paths: Vec<PathBuf> = selected.iter().map(|e| e.path.clone()).collect();
            let changed = paths.iter().map(|p| (p, tags::remove_tag(p, name)));
            session.failed_some(Action::Tag, changed);
        } else {
            self.add_tag(name, session);
        }
    }

    /// Add the tag `name` to the selected items that lack it; the sidebar lists it from now on.
    fn add_tag(&self, name: &str, session: &Session) {
        let entries = self.browser.entries();
        let paths: Vec<PathBuf> = self
            .browser
            .selection()
            .indices()
            .filter_map(|i| entries.get(i))
            .filter(|e| !carries(e, name))
            .map(|e| e.path.clone())
            .collect();
        let changed = paths.iter().map(|p| (p, tags::add_tag(p, name)));
        session.failed_some(Action::Tag, changed);
        session.known_tags.borrow_mut().insert(name.to_owned());
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

    /// A folder was made in window `id`: the list selects it and its name is edited, at once when the list shows it
    /// already (its folder was read again before the operation's end arrived) and otherwise when it does.
    fn rename_new_folder(&mut self, id: u64, path: PathBuf) -> Vec<Command<Message>> {
        self.browser.select_when_appears(path.clone());
        self.rename_when_listed = Some(path);
        self.listed(id)
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
            ViewMode::Icons | ViewMode::Gallery => view::item_key(id, index),
            _ => row_key(id, path),
        };
        vec![Command::Perform(Box::pin(async move {
            Message::Window(id, Event::FocusKey(key))
        }))]
    }

    /// Give the keyboard to the row of the item at `path` of window `id` in the column view once it shows it, its ring
    /// showing only if it showed: a click in another column moves the keyboard there without a ring.
    fn follow_item(&self, id: u64, path: &Path) -> Vec<Command<Message>> {
        if !self.browser.entries().iter().any(|e| e.path == path) {
            return Vec::new();
        }
        let key = row_key(id, path);
        vec![Command::Perform(Box::pin(async move {
            Message::Window(id, Event::FollowKey(key))
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

    /// Select as `selection` does; a rename of an item it leaves out ends, as the keyboard goes where the selection
    /// went, not back to the item renamed.
    fn select(&mut self, selection: &Selection, session: &Session) {
        let indices = selection.ranges().iter().flat_map(Clone::clone);
        self.browser
            .select(indices, selection.anchor(), selection.cursor());
        let renamed = self.rename_index();
        if renamed.is_some() && self.browser.selection().indices().ne(renamed) {
            let _ = self.end_rename(session);
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
            single_link: matches!(selected.as_slice(), [e] if e.kind == Kind::Symlink),
            in_trash: self.browser.dir() == session.trash.home_trash().join("files"),
            path_bar: self.path_bar,
            status_bar: self.status_bar,
            sidebar: self.split.collapsed.is_none(),
            full_screen: self.full_screen,
            can_go_back: history.back_target().is_some(),
            can_go_forward: history.forward_target().is_some(),
            has_parent: !self.browser.is_virtual() && self.browser.dir().parent().is_some(),
            eject: self
                .eject_target(session)
                .map(|i| session.places[i].name.clone()),
            read_only: self.read_only(session),
            can_paste: !session.pasteboard.is_empty(),
            can_move: !session.pasteboard.is_empty() && !session.pasteboard_stays,
            jobs: !session.jobs.is_empty(),
            tabs: self.tabs.len(),
            tab_bar: self.tab_bar_shown(),
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

/// What the source of a drop is told was done once `work` ended with `outcome`: a move only when every item left its
/// place, a copy when every item was copied, and nothing otherwise (an item that failed or was stopped, or one moved
/// into the folder already holding it), so the source keeps what it has.
fn dropped(
    work: &transfer::Work,
    outcome: &transfer::Outcome,
) -> Option<cupertino_kit::platform::DropAction> {
    use cupertino_kit::platform::DropAction as Done;
    let all = outcome.results.iter().all(Result::is_ok);
    match work {
        transfer::Work::Trash { .. } => all.then_some(Done::Move),
        transfer::Work::Transfer {
            action: DropAction::Copy,
            ..
        } => all.then_some(Done::Copy),
        transfer::Work::Transfer { sources, .. } => sources
            .iter()
            .zip(&outcome.results)
            .all(|(from, to)| to.as_ref().is_ok_and(|to| to != from))
            .then_some(Done::Move),
        transfer::Work::Duplicate { .. }
        | transfer::Work::NewFolderWith { .. }
        | transfer::Work::EmptyTrash { .. } => None,
    }
}

/// Whether `entry` carries the tag `name`.
fn carries(entry: &Entry, name: &str) -> bool {
    entry.tags.iter().any(|t| t.eq_ignore_ascii_case(name))
}

/// Whether `entry` has the tag of `color`.
fn has_tag(entry: &Entry, color: TagColor) -> bool {
    entry
        .tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case(color.title()))
}

/// Open the window on `start` and serve it until the last window closes and every file operation has ended, or until
/// the launch was handed over; how it ended.
pub fn run(start: Start) -> Result<Ended, cupertino_kit::app::AppError> {
    prepare(start);
    cupertino_kit::app::run::<Files>()?;
    Ok(launch::ended())
}
