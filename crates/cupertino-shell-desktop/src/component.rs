//! The desktop items as a component of the shell host (`cupertino-shell`): the items of the Desktop folder as icons
//! on the main display, on a layer surface of every output that lies over the wallpaper and under every window, which
//! the server holds with its last picture while the component restarts (D19). What it draws and what its presses mean
//! is [`crate::view`]; where the items sit is [`crate::arrangement`].
//!
//! * The folder is listed and watched by Kit's file family ([`DirectoryModel`], its work run as I/O tasks); the icons
//!   are the family's ([`ItemIcons`]), pictures shown by their thumbnails.
//! * Its context menus are the toolkit's: on items Open, Open With, Move to Trash, Rename, Duplicate, Make Alias and
//!   Copy; on the desktop New Folder, Paste, Change Wallpaper…, Sort By and Clean Up.
//! * Keys, once a press gave the desktop the keyboard: Return renames the only selected item, Primary+O and
//!   Primary+Down open the selection, Primary+Backspace moves it to the trash, Primary+D duplicates it, Primary+C and
//!   Primary+V copy and paste, Primary+Shift+N makes a folder, Primary+A selects every item, the arrows move the
//!   selection, Escape clears it.
//! * A folder opens in a window of the file manager; a file with its default application.
//! * Files another application drags here and drops on the main display's desktop are copied into the desktop folder.
//! * Every file operation runs on Kit's I/O pool; copies and moves run as the family's transfers, one slice at a time
//!   ([`Transfer`]). Failures are notices on the component's own session bus connection.

mod menu;
mod work;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cupertino_apps::MimeDatabase;
use cupertino_icons::thumbnail::Thumbnailer;
use cupertino_kit::controls::{Menu, MenuAnchor, TextChange};
use cupertino_kit::core::{
    Application, Command, Dropped, Element, Key, RendererClass, Window, WindowBackground, WindowKey,
};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::files::entry::Entry;
use cupertino_kit::files::geom::Direction;
use cupertino_kit::files::item_icons::ItemIcons;
use cupertino_kit::files::model::{DirectoryModel, ModelDelta, ModelOptions, Notify};
use cupertino_kit::files::notice::{self, Action, Notices};
use cupertino_kit::files::operations::Transfer;
use cupertino_kit::files::sort::{sort_entries, SortKey, SortSpec};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::motion::Timing;
use cupertino_kit::platform::{DecorationMode, Timestamp, WindowSpec};
use cupertino_kit_shell::bus;
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    Anchor, Component, ComponentName, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role,
    Surface,
};

use self::work::{Launch, Opening, Started};
use crate::arrangement::{self, Arrangement, DesktopPrefs, Grid, Slot};
use crate::metrics as m;
use crate::view::{BandLook, Desk, ItemView};

/// What the desktop's windows are called by the roles: `cupertino-desktop@SCREEN`.
pub const APP_ID: &str = "cupertino-desktop";

/// The layer surfaces' namespace, for the server.
const NAMESPACE: &str = "cupertino-desktop";

/// The key the desktop of each output is persisted with (D19).
const HOLD: &str = "desktop";

/// What assistive technology calls the desktop.
const DESKTOP_NAME: &str = "Desktop";

/// The window the thumbnail service knows the desktop's icons by.
const THUMBNAILS_OF: u64 = 0;

/// The desktops.
pub struct Desktop;

/// An output the desktop shows on: its number, which names its windows, the server's output, its size in points, and
/// the area the server configured its window to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Screen {
    number: u64,
    /// `None` on a window system without the shell's roles, as [`prepare`] draws.
    output: Option<OutputId>,
    width: f64,
    height: f64,
    /// The output less the space the menu bar, the dock and other surfaces reserve, as the server configured the
    /// window (`Event::Configured`); `None` until it did.
    area: Option<Size>,
}

/// What a picked row or a key asks the desktop to do.
#[derive(Clone, Debug, PartialEq)]
pub enum DeskCommand {
    Open,
    OpenWith(String),
    MoveToTrash,
    Rename,
    Duplicate,
    MakeAlias,
    Copy,
    Paste,
    NewFolder,
    ChangeWallpaper,
    SortBy(Option<SortKey>),
    CleanUp,
    SelectAll,
    ClearSelection,
    Step(Direction),
}

#[derive(Clone)]
pub enum Message {
    /// The component's session bus connection, or none.
    Session(Option<zbus::Connection>),
    SessionClosed,
    /// The session's file types, the items' icons and the kept arrangement were read.
    Started(Started),
    /// The server's outputs changed.
    Outputs,
    /// The server closed a window.
    Closed(WindowKey),
    /// The server configured a window to this size.
    Configured(WindowKey, Size),
    /// The listing or a thumbnail has news.
    Woken,
    /// What woke the desktop is gone.
    Unheard,
    /// The listing's work or watch, a thumbnail's job or the posting of the notices ended; what it brought wakes the
    /// desktop.
    Worked,
    /// A press landed on the desktop.
    Pressed,
    Select(Vec<PathBuf>),
    Open(Vec<PathBuf>),
    /// The rubber band on screen `.0` is at this rectangle, or was let go, at this time.
    Band(u64, Option<Rect>, Timestamp),
    /// The band of this number has faded out.
    BandGone(u64),
    ContextMenu {
        output: u64,
        at: Point,
    },
    MenuOpened(Vec<usize>),
    MenuClosed,
    /// The applications that open the first selected item, for the Open With of the menu of this number.
    Choices(u64, Vec<(String, String)>),
    Command(DeskCommand),
    /// The selection is dragged this far, or the drag stopped.
    Drag(Option<Point>),
    /// Files another application dragged here were dropped on the desktop.
    DroppedIn(Dropped),
    /// The selection was let go this far from where it was, on this folder.
    Drop {
        delta: Point,
        onto: Option<PathBuf>,
    },
    BeginRename(PathBuf),
    FocusName,
    NameChanged(TextChange),
    NameSubmitted,
    NameCancelled,
    /// What opens each item asked to open.
    Resolved(Vec<Opening>),
    Launch(Launch, Option<String>),
    /// An item was renamed, or why not.
    Renamed {
        from: PathBuf,
        to: Result<PathBuf, String>,
    },
    /// A folder was made, in the free cell nearest to the point given, or why not.
    Made(Result<PathBuf, String>, Option<Point>),
    /// Items were moved to the trash, or why not, and the sizes of trashed folders to record.
    Trashed(Vec<(PathBuf, Result<(), String>)>, Vec<work::PendingSize>),
    /// Aliases were made, or why not.
    Aliased(Vec<(PathBuf, Result<PathBuf, String>)>),
    /// A slice of the transfer of this number ran.
    Sliced(u64),
    /// The arrangement was written, or why not.
    Saved(Option<String>),
}

/// An open context menu: its number, on which screen and where, whether on the selection, its open submenus, and the
/// applications of Open With once they are read.
struct OpenMenu {
    number: u64,
    output: u64,
    at: Point,
    on_items: bool,
    open: Vec<usize>,
    choices: Option<Vec<(String, String)>>,
}

/// The name of an item being edited.
struct Rename {
    path: PathBuf,
    edit: cupertino_kit::files::name_edit::NameEdit,
    revision: u64,
}

/// A copy or a move under way, slice by slice.
struct Running {
    number: u64,
    action: Action,
    sources: Vec<PathBuf>,
    transfer: Transfer,
}

pub struct Model {
    /// The desktop folder, once the start read it.
    folder: Option<PathBuf>,
    listing: Option<DirectoryModel>,
    woken: async_channel::Receiver<()>,
    wake: Notify,
    icons: Option<ItemIcons>,
    screens: Vec<Screen>,
    /// The number the next screen's windows take.
    next_screen: u64,
    /// The room the menu bar keeps at the top of an output whose server reserves none ([`prepare`]).
    menu_bar: f64,
    /// The desktops show: once the folder's first listing is in, so that a restarted desktop takes the place of the
    /// one the server holds with its items (`shell-host.md#holds`), never an empty desktop first.
    shown: bool,
    prefs: DesktopPrefs,
    prefs_file: Option<PathBuf>,
    /// The arrangement is being written; it changed since the write began.
    saving: bool,
    unsaved: bool,
    /// The items, in the order they fill the grid, and their cells.
    entries: Vec<Entry>,
    slots: Vec<Slot>,
    selection: BTreeSet<PathBuf>,
    /// The rubber band, and the screen it is on.
    band: Option<(u64, BandLook)>,
    bands: u64,
    dragged: Option<Point>,
    menu: Option<OpenMenu>,
    menus: u64,
    rename: Option<Rename>,
    /// A folder just made: it is selected and renamed once listed.
    made: Option<PathBuf>,
    /// An item just renamed: it is selected once listed under its new name.
    renamed: Option<PathBuf>,
    pasteboard: Vec<PathBuf>,
    transfers: Vec<Running>,
    next_transfer: u64,
    notices: Notices,
    /// The session bus connection the notices go out on.
    session: Option<zbus::Connection>,
}

fn desk_key(screen: u64) -> WindowKey {
    WindowKey(Key::from(format!("desktop/{screen}")))
}

fn menu_key(screen: u64) -> WindowKey {
    WindowKey(Key::from(format!("desktop/{screen}/menu")))
}

fn name_field_key() -> Key {
    Key::from("desktop/name")
}

fn listen(woken: &async_channel::Receiver<()>) -> Command<Message> {
    let woken = woken.clone();
    Command::Perform(Box::pin(async move {
        woken
            .recv()
            .await
            .map_or(Message::Unheard, |()| Message::Woken)
    }))
}

fn after(delay: f64, message: Message) -> Command<Message> {
    Command::After {
        delay: Duration::from_secs_f64(delay),
        message,
    }
}

impl Model {
    /// A desktop that lists nothing yet and shows on no screen.
    fn empty() -> Model {
        let (wake, woken) = async_channel::bounded(1);
        let wake: Notify = Arc::new(move || {
            // a wake already waiting covers this change too
            let _ = wake.try_send(());
        });
        Model {
            folder: None,
            listing: None,
            woken,
            wake,
            icons: None,
            screens: Vec::new(),
            next_screen: 0,
            menu_bar: 0.0,
            shown: false,
            prefs: DesktopPrefs::default(),
            prefs_file: None,
            saving: false,
            unsaved: false,
            entries: Vec::new(),
            slots: Vec::new(),
            selection: BTreeSet::new(),
            band: None,
            bands: 0,
            dragged: None,
            menu: None,
            menus: 0,
            rename: None,
            made: None,
            renamed: None,
            pasteboard: Vec::new(),
            transfers: Vec::new(),
            next_transfer: 0,
            notices: Notices::logged(),
            session: None,
        }
    }

    /// List `folder`, its items typed with `types` and drawn with `file_icons` and the thumbnails of `thumbnails`.
    fn list(
        &mut self,
        folder: PathBuf,
        types: Arc<MimeDatabase>,
        file_icons: FileIcons,
        thumbnails: Thumbnailer,
    ) {
        let options = ModelOptions {
            types: Some(types),
            ..ModelOptions::default()
        };
        self.listing = Some(DirectoryModel::open(
            folder.clone(),
            options,
            Arc::clone(&self.wake),
        ));
        self.icons = Some(ItemIcons::new(
            file_icons,
            Some(thumbnails),
            Arc::clone(&self.wake),
        ));
        self.folder = Some(folder);
    }

    /// The main display, which shows the items.
    fn main(&self) -> Option<Screen> {
        self.screens.first().copied()
    }

    /// The main display's grid: within the area the server configured, or, without the shell's roles, the whole
    /// screen under the menu bar's room; none until the server configured the window.
    fn grid(&self) -> Option<Grid> {
        let main = self.main()?;
        match (main.area, main.output) {
            (Some(area), _) => Some(Grid::new(area.width, area.height, 0.0)),
            (None, None) => Some(Grid::new(main.width, main.height, self.menu_bar)),
            (None, Some(_)) => None,
        }
    }

    /// The server configured window `key` to `size`: a desktop lays its items out in it.
    fn configured(&mut self, key: &WindowKey, size: Size) {
        let Some(screen) = self.screens.iter_mut().find(|s| desk_key(s.number) == *key) else {
            return;
        };
        if screen.area != Some(size) {
            screen.area = Some(size);
            self.relist();
        }
    }

    /// Show the desktops once the folder's first listing is in, or once it cannot be listed.
    fn reveal(&mut self) {
        self.shown |=
            self.folder.is_some() && self.listing.as_ref().is_none_or(|l| !l.is_loading());
    }

    /// The screen the server's outputs are now, each keeping its number while its output stays.
    fn follow_outputs(&mut self) {
        let mut screens = Vec::new();
        for output in outputs::all() {
            let Some(geometry) = output.geometry else {
                continue;
            };
            let (number, area) = match self.screens.iter().find(|s| s.output == Some(output.id)) {
                Some(screen) => (screen.number, screen.area),
                None => {
                    self.next_screen += 1;
                    (self.next_screen, None)
                }
            };
            screens.push(Screen {
                number,
                output: Some(output.id),
                width: geometry.width(),
                height: geometry.height(),
                area,
            });
        }
        let shown = |n: u64| screens.iter().any(|s| s.number == n);
        if self.menu.as_ref().is_some_and(|menu| !shown(menu.output)) {
            self.menu = None;
        }
        if self.band.is_some_and(|(on, _)| !shown(on)) {
            self.band = None;
        }
        self.screens = screens;
        self.relist();
    }

    /// The server closed window `key`: a desktop's window is made anew on its output, a menu is gone.
    fn closed(&mut self, key: &WindowKey) {
        if let Some(i) = self.screens.iter().position(|s| desk_key(s.number) == *key) {
            self.next_screen += 1;
            let number = self.next_screen;
            let old = std::mem::replace(&mut self.screens[i].number, number);
            // the new window is configured anew
            self.screens[i].area = None;
            if self.menu.as_ref().is_some_and(|menu| menu.output == old) {
                self.menu = None;
            }
            if self.band.is_some_and(|(on, _)| on == old) {
                self.band = None;
            }
        } else if self
            .menu
            .as_ref()
            .is_some_and(|menu| menu_key(menu.output) == *key)
        {
            self.menu = None;
        }
    }

    /// The arrangement changed: it is written after this update.
    fn save(&mut self) {
        if self.prefs_file.is_some() {
            self.unsaved = true;
        }
    }

    /// Lay the listing out again: in the arrangement's order, each item in its cell, which is kept once the folder is
    /// listed in full.
    fn relist(&mut self) {
        let Some(grid) = self.grid() else {
            return;
        };
        let Some(listing) = &self.listing else {
            return;
        };
        let loading = listing.is_loading();
        let mut entries: Vec<Entry> = listing.entries().to_vec();
        if let Arrangement::Sorted(key) = self.prefs.arrangement {
            sort_entries(
                &mut entries,
                &SortSpec {
                    key,
                    ascending: Arrangement::ascending(key),
                    folders_first: false,
                },
            );
        }
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        self.slots = arrangement::arrange(&names, &self.prefs, &grid);
        if !loading {
            let positions = names
                .iter()
                .zip(&self.slots)
                .map(|(name, slot)| (name.to_string(), *slot))
                .collect();
            if positions != self.prefs.positions {
                self.prefs.positions = positions;
                self.save();
            }
        }
        self.entries = entries;
        let listed = |p: &Path| self.entries.iter().any(|e| e.path == p);
        self.selection.retain(|p| listed(p));
        if self.rename.as_ref().is_some_and(|r| !listed(&r.path)) {
            self.rename = None;
        }
        if let Some(renamed) = self.renamed.take() {
            if listed(&renamed) {
                self.selection = BTreeSet::from([renamed]);
            } else {
                self.renamed = Some(renamed);
            }
        }
        if let Some(icons) = &self.icons {
            icons.want(THUMBNAILS_OF, &self.entries, m::ICON_SIZE);
        }
    }

    /// The folder just made, once listed: selected alone and renamed.
    fn rename_made(&mut self) -> Vec<Command<Message>> {
        let Some(made) = self.made.clone() else {
            return Vec::new();
        };
        if !self.entries.iter().any(|e| e.path == made) {
            return Vec::new();
        }
        self.made = None;
        self.selection = BTreeSet::from([made.clone()]);
        self.begin_rename(made)
    }

    /// Whether `path` is a folder, as the listing's entry for it tells; a path not listed is none.
    fn is_dir(&self, path: &Path) -> bool {
        self.entries.iter().any(|e| e.is_dir && e.path == path)
    }

    fn selected(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|e| self.selection.contains(&e.path))
            .map(|e| e.path.clone())
            .collect()
    }

    fn failed(&self, action: Action, items: &[&Path], error: &dyn std::fmt::Display) {
        self.notices.post(notice::failure(action, items, error));
    }

    /// Report the failures among `results` as one notice.
    fn failed_some<T, E: std::fmt::Display>(
        &self,
        action: Action,
        results: &[(PathBuf, Result<T, E>)],
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
            self.notices.post(notice::failure(action, &failed, e));
        }
    }

    fn begin_rename(&mut self, path: PathBuf) -> Vec<Command<Message>> {
        if self.selection.len() != 1 || !self.selection.contains(&path) {
            return Vec::new();
        }
        let Some(entry) = self.entries.iter().find(|e| e.path == path) else {
            return Vec::new();
        };
        let edit = cupertino_kit::files::name_edit::NameEdit::new(0, &entry.name, entry.is_dir);
        let revision = self.rename.as_ref().map_or(0, |r| r.revision + 1);
        self.rename = Some(Rename {
            path,
            edit,
            revision,
        });
        vec![Command::Perform(Box::pin(async { Message::FocusName }))]
    }

    /// End the rename, renaming the item when its name changed to a valid one; a name that is not valid keeps the
    /// field open. The item keeps its cell under its new name.
    fn end_rename(&mut self) -> Vec<Command<Message>> {
        let Some(rename) = self.rename.take() else {
            return Vec::new();
        };
        match rename.edit.commit() {
            Ok(None) => Vec::new(),
            Ok(Some(name)) => {
                let old = rename
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                if let Some(slot) = old.and_then(|old| self.prefs.positions.get(&old).copied()) {
                    self.prefs.positions.insert(name.clone(), slot);
                }
                vec![work::rename(rename.path, name)]
            }
            Err(e) => {
                self.notices.post(notice::bad_name(rename.edit.text(), &e));
                self.rename = Some(rename);
                Vec::new()
            }
        }
    }

    /// Take what the listing has waiting. A folder that does not open is said on standard error and listed no more.
    fn poll(&mut self) {
        let Some(listing) = self.listing.as_mut() else {
            return;
        };
        let deltas = listing.poll();
        if let Some(error) = deltas.iter().find_map(|delta| match delta {
            ModelDelta::OpenFailed { error, .. } => Some(error),
            _ => None,
        }) {
            eprintln!(
                "cupertino-shell-desktop: cannot list {}: {error}",
                listing.dir().display()
            );
            self.listing = None;
            return;
        }
        if !deltas.is_empty() {
            self.relist();
        }
    }

    /// The listing's watch and its next work, and the thumbnails' jobs, as a future on the UI loop and I/O tasks.
    fn io_work(&mut self) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        if let Some(listing) = self.listing.as_mut() {
            commands.extend(listing.take_watch().map(|watch| {
                let follow = watch.follow();
                Command::Perform(Box::pin(async move {
                    follow.await;
                    Message::Worked
                }))
            }));
            commands.extend(listing.take_work().map(|work| {
                cupertino_kit::task::io(
                    cupertino_kit::files::model::WORK_DEADLINE,
                    move || work.run(),
                    |()| Message::Worked,
                )
            }));
        }
        if let Some(icons) = &self.icons {
            commands.extend(std::iter::from_fn(|| icons.take_job()).map(|job| {
                cupertino_kit::task::io(
                    cupertino_kit::files::thumbnail_service::JOB_DEADLINE,
                    move || job.run(),
                    |()| Message::Worked,
                )
            }));
        }
        commands
    }

    /// The current cells become the kept ones: the items no longer follow a sort.
    fn keep_slots(&mut self) {
        self.prefs.arrangement = Arrangement::Free;
        self.prefs.positions = self
            .entries
            .iter()
            .zip(&self.slots)
            .map(|(e, s)| (e.name.clone(), *s))
            .collect();
        self.save();
    }

    /// Files dropped from another application: copied into the desktop folder, an application's desktop file
    /// among them, as a paste does. The desktop's own items, and a list that is not of local files, are refused.
    fn dropped_in(&mut self, dropped: Dropped) -> Vec<Command<Message>> {
        let sources = self
            .folder
            .as_deref()
            .and_then(|folder| work::dropped_sources(&dropped.data.bytes, folder));
        let (Some(sources), Some(dest)) = (sources, self.folder.clone()) else {
            return vec![Command::FinishDrop(dropped.token, None)];
        };
        let mut commands = vec![Command::FinishDrop(
            dropped.token,
            Some(cupertino_kit::platform::DropAction::Copy),
        )];
        let transfer = Transfer::into_folder(
            sources.clone(),
            dest,
            cupertino_kit::files::operations::DropAction::Copy,
        );
        commands.extend(self.transfer(Action::Paste, sources, transfer));
        commands
    }

    fn drop_items(&mut self, delta: Point, onto: Option<PathBuf>) -> Vec<Command<Message>> {
        self.dragged = None;
        let moving: Vec<PathBuf> = self.selected();
        if let Some(folder) = onto {
            let action = cupertino_kit::files::operations::DropAction::Move;
            let transfer = Transfer::into_folder(moving.clone(), folder, action);
            return self.transfer(Action::Drop(action), moving, transfer);
        }
        let Some(grid) = self.grid() else {
            return Vec::new();
        };
        // the item the pointer holds leads: it is the selected one nearest to the press, the first selected in order
        let indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.selection.contains(&e.path))
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            return Vec::new();
        }
        self.slots = arrangement::dropped(&self.slots, &indices, delta, &grid);
        self.keep_slots();
        Vec::new()
    }

    /// The items as the main display shows them.
    fn items(&self) -> Vec<ItemView> {
        let Some(grid) = self.grid() else {
            return Vec::new();
        };
        let ringed = self.menu.as_ref().is_some_and(|menu| menu.on_items);
        self.entries
            .iter()
            .zip(&self.slots)
            .map(|(entry, slot)| {
                let cell = grid.cell(*slot);
                let selected = self.selection.contains(&entry.path);
                ItemView {
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    is_dir: entry.is_dir,
                    cell,
                    icon: self
                        .icons
                        .as_ref()
                        .map(|icons| icons.icon(entry, m::ICON_SIZE))
                        .unwrap_or_default(),
                    selected,
                    ringed: ringed && selected,
                }
            })
            .collect()
    }

    /// What `message` asks of the desktop, before the arrangement is written.
    fn handle(&mut self, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Session(connection) => return self.session(connection),
            Message::SessionClosed => {
                // the notices are said on standard error from now on
                eprintln!("cupertino-shell-desktop: the session bus connection closed");
                self.session = None;
                self.notices = Notices::logged();
            }
            Message::Started(started) => return self.started(started),
            Message::Outputs => self.follow_outputs(),
            Message::Closed(key) => self.closed(&key),
            Message::Configured(key, size) => self.configured(&key, size),
            Message::Woken => {
                self.poll();
                // a thumbnail that came shows at the next view
                let mut commands = self.rename_made();
                commands.push(listen(&self.woken));
                commands.extend(self.io_work());
                return commands;
            }
            Message::Unheard | Message::Worked => {}
            Message::DroppedIn(dropped) => return self.dropped_in(dropped),
            Message::Pressed => {
                self.menu = None;
                return self.end_rename();
            }
            Message::Select(paths) => self.selection = paths.into_iter().collect(),
            Message::Open(paths) => return self.open(paths, None),
            Message::Band(output, Some(rect), at) => {
                let started = match self.band {
                    Some((on, band)) if on == output && band.ended.is_none() => band.started,
                    _ => at,
                };
                self.band = Some((
                    output,
                    BandLook {
                        rect,
                        started,
                        ended: None,
                    },
                ));
            }
            Message::Band(_, None, at) => {
                if let Some((_, band)) = self.band.as_mut() {
                    band.ended = Some(at);
                    self.bands += 1;
                    let fade = Timing::DESKTOP_BAND_FADE_OUT.duration();
                    return vec![after(fade, Message::BandGone(self.bands))];
                }
            }
            Message::BandGone(band) => {
                if band == self.bands && self.band.is_some_and(|(_, b)| b.ended.is_some()) {
                    self.band = None;
                }
            }
            Message::ContextMenu { output, at } => return self.context_menu(output, at),
            Message::MenuOpened(open) => {
                if let Some(menu) = self.menu.as_mut() {
                    menu.open = open;
                }
            }
            Message::MenuClosed => self.menu = None,
            Message::Choices(number, choices) => {
                if let Some(menu) = self.menu.as_mut().filter(|menu| menu.number == number) {
                    menu.choices = Some(choices);
                }
            }
            Message::Command(command) => return self.command(command),
            Message::Drag(delta) => self.dragged = delta,
            Message::Drop { delta, onto } => return self.drop_items(delta, onto),
            Message::BeginRename(path) => return self.begin_rename(path),
            Message::FocusName => {
                if self.rename.is_some() {
                    return vec![Command::Focus(name_field_key())];
                }
            }
            Message::NameChanged(change) => {
                if let Some(rename) = self.rename.as_mut() {
                    rename.edit.field_mut().set_text(&change.text);
                    rename.revision = change.revision;
                }
            }
            Message::NameSubmitted => return self.end_rename(),
            Message::NameCancelled => self.rename = None,
            message => return self.worked(message),
        }
        Vec::new()
    }

    /// The component's session bus connection: the notices go out on it.
    fn session(&mut self, connection: Option<zbus::Connection>) -> Vec<Command<Message>> {
        let Some(connection) = connection else {
            eprintln!(
                "cupertino-shell-desktop: no session bus connection; notices go to standard error"
            );
            return Vec::new();
        };
        let (notices, posting) = Notices::on(connection.clone());
        self.notices = notices;
        self.session = Some(connection);
        vec![Command::Perform(Box::pin(async move {
            posting.await;
            Message::Worked
        }))]
    }

    /// The start's reads came: the folder is listed and watched.
    fn started(&mut self, started: Started) -> Vec<Command<Message>> {
        let Some(session) = started.take() else {
            return Vec::new();
        };
        let session = match session {
            Ok(session) => session,
            Err(e) => {
                // the host starts it again
                eprintln!("cupertino-shell-desktop: {e}");
                return vec![Command::Exit];
            }
        };
        self.prefs = session.prefs;
        self.prefs_file = session.prefs_file;
        self.list(
            session.folder,
            session.types,
            session.file_icons,
            Thumbnailer::from_env(APP_ID),
        );
        self.relist();
        let mut commands = vec![listen(&self.woken)];
        commands.extend(self.io_work());
        commands
    }
}

thread_local! {
    /// The desktop [`prepare`] gave the thread's next [`Desktop`].
    static START: RefCell<Option<Model>> = const { RefCell::new(None) };
}

/// Give the next [`Desktop`] the thread starts the items of `folder` on one screen of `size` points, typed with
/// `types`, drawn with `file_icons` and their thumbnails made in `thumbnails`, without the shell's roles, a kept
/// arrangement or a session bus: how a driver of the headless window system shows the desktop.
pub fn prepare(
    folder: PathBuf,
    size: Size,
    types: Arc<MimeDatabase>,
    file_icons: FileIcons,
    thumbnails: Thumbnailer,
) {
    let mut model = Model::empty();
    model.list(folder, types, file_icons, thumbnails);
    model.screens = vec![Screen {
        number: 0,
        output: None,
        width: size.width,
        height: size.height,
        area: None,
    }];
    model.menu_bar = cupertino_kit_shell::design::menu_bar().height.ceil();
    model.shown = true;
    START.with(|s| *s.borrow_mut() = Some(model));
}

impl Application for Desktop {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// Full screen but static: it repaints when its icons change, so a GPU context would only cost memory.
    const RENDERER: RendererClass = RendererClass::Cpu;

    /// The desktop [`prepare`] gave the thread; else the session's, whose folder, file types and icons are read on
    /// the I/O pool, shown on the server's outputs, with its notices on the component's session bus connection.
    fn init() -> (Model, Vec<Command<Message>>) {
        if let Some(mut model) = START.with(|s| s.borrow_mut().take()) {
            model.relist();
            let mut commands = vec![listen(&model.woken)];
            commands.extend(model.io_work());
            return (model, commands);
        }
        let mut model = Model::empty();
        model.follow_outputs();
        let mut commands = bus::session(
            |connection| Message::Session(connection.ok()),
            Message::SessionClosed,
        );
        commands.push(work::start());
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let mut commands = model.handle(message);
        model.reveal();
        commands.extend(model.write_prefs());
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        if !model.shown {
            return Vec::new();
        }
        let items: std::rc::Rc<[ItemView]> = model.items().into();
        let empty: std::rc::Rc<[ItemView]> = Vec::new().into();
        let main = model.main().map(|s| s.number);
        let mut windows = Vec::new();
        for screen in &model.screens {
            let on_main = Some(screen.number) == main;
            let desk = Desk {
                output: screen.number,
                label: DESKTOP_NAME.into(),
                items: if on_main {
                    std::rc::Rc::clone(&items)
                } else {
                    std::rc::Rc::clone(&empty)
                },
                band: model
                    .band
                    .filter(|(on, _)| *on == screen.number)
                    .map(|(_, band)| band),
                dragged: model.dragged.filter(|_| on_main),
                renaming: if on_main { model.name_field() } else { None },
            };
            windows.push(Window {
                key: desk_key(screen.number),
                spec: WindowSpec {
                    title: DESKTOP_NAME.into(),
                    app_id: format!("{APP_ID}@{}", screen.number),
                    size: screen
                        .area
                        .unwrap_or(Size::new(screen.width, screen.height)),
                    min_size: None,
                    max_size: None,
                    decorations: DecorationMode::None,
                    foreign_parent: None,
                    input_region: None,
                },
                shortcuts: model.shortcuts(),
                on_close: Some(Message::Unheard),
                background: WindowBackground::None,
                // files dragged from another application land on the main display's desktop
                content: if on_main {
                    work::takes_drops(Element::new(desk).fill())
                } else {
                    Element::new(desk).fill()
                },
                popup: None,
                menus: None,
            });
        }
        if let Some(menu) = &model.menu {
            if model.screens.iter().any(|s| s.number == menu.output) {
                windows.extend(
                    Menu {
                        parent: desk_key(menu.output),
                        anchor: MenuAnchor::At(menu.at),
                        label: DESKTOP_NAME.into(),
                        items: model.menu_items(menu),
                        current: None,
                        open: menu.open.clone(),
                        on_open: Box::new(Message::MenuOpened),
                        on_close: Message::MenuClosed,
                    }
                    .windows(menu_key(menu.output)),
                );
            }
        }
        windows
    }
}

impl Program for Desktop {
    /// A desktop is a layer surface under the windows, over the area of its output the menu bar, the dock and other
    /// surfaces leave (exclusive zone 0), taking the keyboard once a press lands on it; the server holds its picture
    /// while the component restarts. Its context menu is a Kit popup of it.
    fn surface(model: &Model, key: &WindowKey) -> Surface {
        let Some(screen) = model.screens.iter().find(|s| desk_key(s.number) == *key) else {
            return Surface {
                role: Role::Window,
                persistent: None,
                holes: Vec::new(),
            };
        };
        Surface {
            role: Role::Layer(LayerSpec {
                namespace: NAMESPACE,
                output: screen.output,
                layer: Layer::Bottom,
                anchor: Anchor {
                    top: true,
                    bottom: true,
                    left: true,
                    right: true,
                },
                size: Size::new(0.0, 0.0),
                margin: Margin::default(),
                // within the space other surfaces reserve, which the items stay clear of
                exclusive_zone: 0,
                keyboard: Keyboard::OnDemand,
            }),
            persistent: Some(HOLD),
            holes: Vec::new(),
        }
    }

    /// Once the folder's first listing is in: until then the server holds the picture of the instance before.
    fn showing(model: &Model) -> bool {
        model.shown
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Outputs => Some(Message::Outputs),
            Event::Closed(key) => Some(Message::Closed(key)),
            Event::Configured { window, size } => Some(Message::Configured(window, size)),
            _ => None,
        }
    }
}

impl Component for Desktop {
    const NAME: ComponentName = ComponentName::DESKTOP;
}
