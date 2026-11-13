//! The desktop (`cupertino-files --desktop`): the items of the desktop folder as icons on the main display, on a layer
//! surface of every output that lies over the wallpaper and under every window ([`roles`]). What it draws and what its
//! presses mean is [`view`]; where the items sit is [`crate::desktop`].
//!
//! * The folder is listed and watched as a browser window's is ([`DirectoryModel`]); the icons are the browser's
//!   ([`ItemIcons`]), pictures shown by their thumbnails.
//! * Its context menus are the toolkit's: on items Open, Open With, Move to Trash, Rename, Duplicate, Make Alias and
//!   Copy; on the desktop New Folder, Paste, Change Wallpaper…, Sort By and Clean Up.
//! * Keys, once a press gave the desktop the keyboard: Return renames the only selected item, Primary+O and
//!   Primary+Down open the selection, Primary+Backspace moves it to the trash, Primary+D duplicates it, Primary+C and
//!   Primary+V copy and paste, Primary+Shift+N makes a folder, Primary+A selects every item, the arrows move the
//!   selection, Escape clears it.
//! * A folder opens in a browser window of the running Files (which this process hands it to); a file with its default
//!   application.
//! * Files another application drags here (an application's desktop file from the launcher, say) and drops on the main
//!   display's desktop are copied into the desktop folder.
//!
//! The process runs apart from the browser windows' process: the desktop stays up while no window is open.

mod layer;
mod roles;
mod view;

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cupertino_apps::{launch, Locale, MimeApps, MimeDatabase, Visibility, XdgDirs};
use cupertino_kit::controls::{Menu, MenuAnchor, MenuItem as KitItem, TextChange, TextField};
use cupertino_kit::core::{
    Application, Command, CommandRegistry, Element, Key, Shortcut, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::platform::{DecorationMode, Keysym, Mods, Timestamp, WindowSpec};
use cupertino_kit::text::ContentPurpose;
use cupertino_modifiers::Profile;
use cupertino_ui_wayland::WaylandWindowSystem;

use self::layer::DesktopShell;
use self::roles::{DesktopRoles, RoleNote};
use self::view::{BandLook, Desk, ItemView};
use super::item_icons::ItemIcons;
use crate::desktop::{self as arrangement, Arrangement, DesktopPrefs, Grid, Slot};
use crate::geom::Direction;
use crate::metrics::desktop as m;
use crate::name_edit::NameEdit;
use crate::notice::{self, Action, Notices};
use crate::open::{command_with, default_app_id, open_command, open_with, OpenError};
use crate::operations::{self, DropAction};
use crate::places::{desktop_folder, session_places};
use crate::sort::{sort_entries, SortKey, SortSpec};
use crate::thumbnail::Thumbnailer;
use crate::trash::Trash;
use crate::{DirectoryModel, Entry, ModelOptions};

/// What the desktop's windows are called by the roles: `cupertino-desktop@OUTPUT`.
pub const APP_ID: &str = "cupertino-desktop";

/// What assistive technology calls the desktop.
const DESKTOP_NAME: &str = "Desktop";

/// The program that shows the wallpaper settings, and its pane.
const SETTINGS: &str = "cupertino-settings";
const WALLPAPER_PANE: &str = "wallpaper";

/// The window the thumbnail service knows the desktop's icons by.
const THUMBNAILS_OF: u64 = 0;

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const ESCAPE: Keysym = Keysym(0xff1b);
const BACKSPACE: Keysym = Keysym(0xff08);
const LEFT: Keysym = Keysym(0xff51);
const UP: Keysym = Keysym(0xff52);
const RIGHT: Keysym = Keysym(0xff53);
const DOWN: Keysym = Keysym(0xff54);

/// An output of the compositor, by its key (the `wl_output` registry name), in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub key: u32,
    pub width: f64,
    pub height: f64,
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

/// What a launch starts once its activation token comes.
#[derive(Clone, Debug)]
pub enum Launch {
    /// Open the item at `path`, with the application of desktop entry `with` or its default one.
    Item { path: PathBuf, with: Option<String> },
    /// The settings' wallpaper pane.
    Wallpaper,
}

#[derive(Clone, Debug)]
pub enum Message {
    Role(RoleNote),
    /// The listing or a thumbnail has news.
    Woken,
    /// What woke the desktop is gone.
    Unheard,
    /// A press landed on the desktop.
    Pressed,
    Select(Vec<PathBuf>),
    Open(Vec<PathBuf>),
    /// The rubber band on output `.0` is at this rectangle, or was let go, at this time.
    Band(u32, Option<Rect>, Timestamp),
    /// The band of this number has faded out.
    BandGone(u64),
    ContextMenu {
        output: u32,
        at: Point,
    },
    MenuOpened(Vec<usize>),
    MenuClosed,
    Command(DeskCommand),
    /// The selection is dragged this far, or the drag stopped.
    Drag(Option<Point>),
    /// Files another application dragged here were dropped on the desktop.
    DroppedIn(cupertino_kit::core::Dropped),
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
    Launch(Launch, Option<String>),
}

/// An open context menu: on which output and where, whether on the selection, and its open submenus.
struct OpenMenu {
    output: u32,
    at: Point,
    on_items: bool,
    open: Vec<usize>,
}

/// The name of an item being edited.
struct Rename {
    path: PathBuf,
    edit: NameEdit,
    revision: u64,
}

pub struct Model {
    folder: PathBuf,
    listing: Option<DirectoryModel>,
    woken: async_channel::Receiver<()>,
    notes: async_channel::Receiver<Message>,
    icons: Option<ItemIcons>,
    screens: Vec<Screen>,
    /// The size each output's surface was given.
    sizes: HashMap<u32, (f64, f64)>,
    /// The room the menu bar keeps at the top of each output.
    menu_bar: f64,
    prefs: DesktopPrefs,
    prefs_file: Option<PathBuf>,
    /// The items, in the order they fill the grid, and their cells.
    entries: Vec<Entry>,
    slots: Vec<Slot>,
    selection: BTreeSet<PathBuf>,
    /// The rubber band, and the output it is on.
    band: Option<(u32, BandLook)>,
    bands: u64,
    dragged: Option<Point>,
    menu: Option<OpenMenu>,
    rename: Option<Rename>,
    /// A folder just made: it is selected and renamed once listed, in the cell it was made at.
    made: Option<PathBuf>,
    pasteboard: Vec<PathBuf>,
    trash: Trash,
    notices: Notices,
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// The default-application resolver of the session's directories and desktops, read again at each open.
fn applications() -> io::Result<MimeApps> {
    let dirs = XdgDirs::from_env().map_err(other)?;
    let db = MimeDatabase::load(&dirs.all_data_dirs()).map_err(other)?;
    MimeApps::load(&dirs, db, &Locale::from_env(), &Visibility::from_env()).map_err(other)
}

fn desk_key(output: u32) -> WindowKey {
    WindowKey(Key::from(format!("desktop/{output}")))
}

fn menu_key(output: u32) -> WindowKey {
    WindowKey(Key::from(format!("desktop/{output}/menu")))
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

fn hear(notes: &async_channel::Receiver<Message>) -> Command<Message> {
    let notes = notes.clone();
    Command::Perform(Box::pin(async move {
        notes.recv().await.unwrap_or(Message::Unheard)
    }))
}

fn after(delay: f64, message: Message) -> Command<Message> {
    Command::After {
        delay: Duration::from_secs_f64(delay),
        message,
    }
}

impl Model {
    /// A desktop on `folder` that nothing lists yet.
    fn empty(folder: PathBuf, notes: async_channel::Receiver<Message>) -> Model {
        Model {
            folder,
            listing: None,
            woken: async_channel::bounded(1).1,
            notes,
            icons: None,
            screens: Vec::new(),
            sizes: HashMap::new(),
            menu_bar: 0.0,
            prefs: DesktopPrefs::default(),
            prefs_file: None,
            entries: Vec::new(),
            slots: Vec::new(),
            selection: BTreeSet::new(),
            band: None,
            bands: 0,
            dragged: None,
            menu: None,
            rename: None,
            made: None,
            pasteboard: Vec::new(),
            trash: Trash::new(),
            notices: Notices::start(None),
        }
    }

    /// The main display, which shows the items.
    fn main(&self) -> Option<Screen> {
        self.screens.first().copied()
    }

    fn grid(&self) -> Option<Grid> {
        let main = self.main()?;
        let (width, height) = self
            .sizes
            .get(&main.key)
            .copied()
            .unwrap_or((main.width, main.height));
        Some(Grid::new(width, height, self.menu_bar))
    }

    fn save(&self) {
        if let Some(file) = &self.prefs_file {
            if let Err(e) = self.prefs.save(file) {
                eprintln!("cupertino-files: cannot keep the desktop's arrangement: {e}");
            }
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
        notices: &Notices,
        action: Action,
        results: Vec<(PathBuf, Result<T, E>)>,
    ) {
        let mut failed = Vec::new();
        let mut first = None;
        for (path, result) in &results {
            if let Err(e) = result {
                failed.push(path.as_path());
                first.get_or_insert(e);
            }
        }
        if let Some(e) = first {
            notices.post(notice::failure(action, &failed, e));
        }
    }

    /// Run `work` on a thread of its own and report what failed: copying and moving can take long.
    fn in_background(
        &self,
        action: Action,
        work: impl FnOnce() -> Vec<(PathBuf, io::Result<PathBuf>)> + Send + 'static,
    ) {
        let notices = self.notices.clone();
        let spawned = std::thread::Builder::new()
            .name("desktop-files".into())
            .spawn(move || Model::failed_some(&notices, action, work()));
        if let Err(e) = spawned {
            eprintln!("cupertino-files: cannot start the desktop's file operation: {e}");
        }
    }

    fn begin_rename(&mut self, path: PathBuf) -> Vec<Command<Message>> {
        if self.selection.len() != 1 || !self.selection.contains(&path) {
            return Vec::new();
        }
        let Some(entry) = self.entries.iter().find(|e| e.path == path) else {
            return Vec::new();
        };
        let edit = NameEdit::new(0, &entry.name, entry.is_dir);
        let revision = self.rename.as_ref().map_or(0, |r| r.revision + 1);
        self.rename = Some(Rename {
            path,
            edit,
            revision,
        });
        vec![Command::Perform(Box::pin(async { Message::FocusName }))]
    }

    /// End the rename, renaming the item when its name changed to a valid one; a name that is not valid keeps the
    /// field open.
    fn end_rename(&mut self) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        match rename.edit.commit() {
            Ok(None) => {}
            Ok(Some(name)) => match operations::rename_item(&rename.path, &name) {
                Ok(path) => {
                    // the item keeps its cell under its new name
                    let old = rename
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned());
                    if let Some(slot) = old.and_then(|old| self.prefs.positions.remove(&old)) {
                        self.prefs.positions.insert(name, slot);
                    }
                    self.selection = BTreeSet::from([path]);
                }
                Err(e) => self.failed(Action::Rename, &[&rename.path], &e),
            },
            Err(e) => {
                self.notices.post(notice::bad_name(rename.edit.text(), &e));
                self.rename = Some(rename);
            }
        }
    }

    /// Ask for an activation token for each of `paths`, naming the application that opens it, so the dock shows it
    /// starting; each opens once its token comes.
    fn open(&self, paths: Vec<PathBuf>, with: Option<String>) -> Vec<Command<Message>> {
        let Some(main) = self.main() else {
            return Vec::new();
        };
        let apps = applications().ok();
        paths
            .into_iter()
            .map(|path| {
                let app_id = match (&with, path.is_dir()) {
                    (Some(id), _) => Some(id.clone()),
                    (None, true) => Some(super::APP_ID.to_string()),
                    (None, false) => apps
                        .as_ref()
                        .and_then(|apps| default_app_id(apps, &path, false)),
                };
                let with = with.clone();
                Command::ActivationToken {
                    window: desk_key(main.key),
                    app_id,
                    on_token: Box::new(move |token| {
                        Message::Launch(Launch::Item { path, with }, token)
                    }),
                }
            })
            .collect()
    }

    fn launch(&self, what: Launch, token: Option<String>) {
        let mut command = match what {
            Launch::Wallpaper => {
                let mut command = std::process::Command::new(SETTINGS);
                command.args(["--pane", WALLPAPER_PANE]);
                command
            }
            // a folder opens in a browser window, which the running Files shows
            Launch::Item { path, with: None } if path.is_dir() => {
                let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(super::APP_ID));
                let mut command = std::process::Command::new(exe);
                command.arg(&path);
                command
            }
            Launch::Item { path, with } => {
                let spec = applications()
                    .map_err(|e| e.to_string())
                    .and_then(|apps| match &with {
                        None => {
                            open_command(&apps, &path, path.is_dir(), None).map_err(|e| match e {
                                OpenError::NoApplication { .. } => {
                                    "No application is set to open it".to_string()
                                }
                                e => e.to_string(),
                            })
                        }
                        Some(id) => {
                            let item = open_with(&apps, &path, path.is_dir())
                                .into_iter()
                                .find(|item| item.entry.id == *id)
                                .ok_or_else(|| format!("{id} no longer opens it"))?;
                            command_with(item.entry, &path, None).map_err(|e| e.to_string())
                        }
                    });
                match spec {
                    Ok(spec) => spec.to_command(),
                    Err(e) => {
                        self.failed(Action::Open, &[&path], &e);
                        return;
                    }
                }
            }
        };
        if let Some(token) = token {
            command.env("XDG_ACTIVATION_TOKEN", &token);
            command.env("DESKTOP_STARTUP_ID", &token);
        }
        launch::spawn(super::APP_ID, command);
    }

    fn trash_selection(&mut self) {
        let paths = self.selected();
        let mut sizes = Vec::new();
        let moved: Vec<_> = paths
            .iter()
            .map(|path| {
                let moved = self.trash.trash_unsized(path).map(|(item, size)| {
                    sizes.extend(size);
                    item
                });
                (path.clone(), moved)
            })
            .collect();
        Model::failed_some(&self.notices, Action::Trash, moved);
        if !sizes.is_empty() {
            let _ = std::thread::Builder::new()
                .name("trash-sizes".into())
                .spawn(move || {
                    for size in sizes {
                        if let Err(e) = size.record() {
                            eprintln!(
                                "cupertino-files: cannot record a trashed folder's size: {e}"
                            );
                        }
                    }
                });
        }
        self.selection.clear();
    }

    /// Make a folder in the desktop folder, in the free cell nearest to `at` on the main display when given.
    fn new_folder(&mut self, at: Option<Point>) -> Vec<Command<Message>> {
        match operations::new_folder(&self.folder) {
            Ok(path) => {
                if let (Some(at), Some(grid), Arrangement::Free) =
                    (at, self.grid(), self.prefs.arrangement)
                {
                    let taken = self.slots.iter().copied().collect();
                    let near = grid.nearest(crate::geom::Point { x: at.x, y: at.y });
                    if let (Some(slot), Some(name)) = (
                        grid.nearest_free(near, &taken),
                        path.file_name().map(|n| n.to_string_lossy().into_owned()),
                    ) {
                        self.prefs.positions.insert(name, slot);
                    }
                }
                self.made = Some(path);
                self.poll();
                self.rename_made()
            }
            Err(e) => {
                self.failed(Action::NewFolder, &[], &e);
                Vec::new()
            }
        }
    }

    /// Take what the listing has waiting.
    fn poll(&mut self) {
        let changed = self
            .listing
            .as_mut()
            .is_some_and(|listing| !listing.poll().is_empty());
        if changed {
            self.relist();
        }
    }

    fn command(&mut self, command: DeskCommand) -> Vec<Command<Message>> {
        let at = self.menu.as_ref().map(|menu| menu.at);
        self.menu = None;
        match command {
            DeskCommand::Open => return self.open(self.selected(), None),
            DeskCommand::OpenWith(id) => return self.open(self.selected(), Some(id)),
            DeskCommand::MoveToTrash => self.trash_selection(),
            DeskCommand::Rename => {
                if let [only] = self.selected().as_slice() {
                    return self.begin_rename(only.clone());
                }
            }
            DeskCommand::Duplicate => {
                let paths = self.selected();
                self.in_background(Action::Duplicate, move || {
                    let done = operations::duplicate(&paths);
                    paths.into_iter().zip(done).collect()
                });
            }
            DeskCommand::MakeAlias => {
                for path in self.selected() {
                    if let Err(e) = operations::make_alias(&path) {
                        self.failed(Action::MakeAlias, &[&path], &e);
                    }
                }
            }
            DeskCommand::Copy => {
                self.pasteboard = self.selected();
                let text = self
                    .pasteboard
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    return vec![Command::SetClipboard(text)];
                }
            }
            DeskCommand::Paste => {
                let (sources, dest) = (self.pasteboard.clone(), self.folder.clone());
                if !sources.is_empty() {
                    self.in_background(Action::Paste, move || {
                        let done = operations::drop_into(&sources, &dest, DropAction::Copy);
                        sources.into_iter().zip(done).collect()
                    });
                }
            }
            DeskCommand::NewFolder => return self.new_folder(at),
            DeskCommand::ChangeWallpaper => {
                let Some(main) = self.main() else {
                    return Vec::new();
                };
                return vec![Command::ActivationToken {
                    window: desk_key(main.key),
                    app_id: Some(SETTINGS.to_string()),
                    on_token: Box::new(|token| Message::Launch(Launch::Wallpaper, token)),
                }];
            }
            DeskCommand::SortBy(key) => {
                self.prefs.arrangement = key.map_or(Arrangement::Free, Arrangement::Sorted);
                self.save();
                self.relist();
            }
            DeskCommand::CleanUp => {
                if let Some(grid) = self.grid() {
                    self.slots = arrangement::cleaned(&self.slots, &grid);
                    self.keep_slots();
                }
            }
            DeskCommand::SelectAll => {
                self.selection = self.entries.iter().map(|e| e.path.clone()).collect();
            }
            DeskCommand::ClearSelection => self.selection.clear(),
            DeskCommand::Step(direction) => {
                let cursor = self
                    .entries
                    .iter()
                    .rposition(|e| self.selection.contains(&e.path));
                let next = match cursor {
                    Some(from) => arrangement::neighbour(&self.slots, from, direction),
                    None => (!self.entries.is_empty()).then_some(0),
                };
                if let Some(next) = next {
                    self.selection = BTreeSet::from([self.entries[next].path.clone()]);
                }
            }
        }
        Vec::new()
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
    fn dropped_in(&mut self, dropped: cupertino_kit::core::Dropped) -> Vec<Command<Message>> {
        let Some(sources) = dropped_sources(&dropped.data.bytes, &self.folder) else {
            return vec![Command::FinishDrop(dropped.token, None)];
        };
        let dest = self.folder.clone();
        self.in_background(Action::Paste, move || {
            let done = operations::drop_into(&sources, &dest, DropAction::Copy);
            sources.into_iter().zip(done).collect()
        });
        vec![Command::FinishDrop(
            dropped.token,
            Some(cupertino_kit::platform::DropAction::Copy),
        )]
    }

    fn drop_items(&mut self, delta: Point, onto: Option<PathBuf>) {
        self.dragged = None;
        let moving: Vec<PathBuf> = self.selected();
        if let Some(folder) = onto {
            self.in_background(Action::Drop(DropAction::Move), move || {
                let done = operations::drop_into(&moving, &folder, DropAction::Move);
                moving.into_iter().zip(done).collect()
            });
            return;
        }
        let Some(grid) = self.grid() else {
            return;
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
            return;
        }
        let delta = crate::geom::Point {
            x: delta.x,
            y: delta.y,
        };
        self.slots = arrangement::dropped(&self.slots, &indices, delta, &grid);
        self.keep_slots();
    }

    fn menu_items(&self, on_items: bool) -> Vec<KitItem<Message>> {
        let item =
            |label: &str, enabled: bool, checked: bool, command: DeskCommand| KitItem::Item {
                label: label.into(),
                enabled,
                checked,
                on_select: Message::Command(command),
            };
        if on_items {
            let selected = self.selected();
            let one = selected.len() == 1;
            let apps = selected.first().and_then(|first| {
                let apps = applications().ok()?;
                let choices: Vec<(String, String)> = open_with(&apps, first, first.is_dir())
                    .into_iter()
                    .map(|i| (i.entry.id.clone(), i.entry.name.clone()))
                    .collect();
                Some(choices)
            });
            let apps = apps.unwrap_or_default();
            return vec![
                item("Open", true, false, DeskCommand::Open),
                KitItem::Submenu {
                    label: "Open With".into(),
                    enabled: !apps.is_empty(),
                    items: apps
                        .into_iter()
                        .map(|(id, name)| item(&name, true, false, DeskCommand::OpenWith(id)))
                        .collect(),
                },
                KitItem::Separator,
                item("Move to Trash", true, false, DeskCommand::MoveToTrash),
                KitItem::Separator,
                item("Rename", one, false, DeskCommand::Rename),
                item("Duplicate", true, false, DeskCommand::Duplicate),
                item("Make Alias", one, false, DeskCommand::MakeAlias),
                KitItem::Separator,
                item("Copy", true, false, DeskCommand::Copy),
            ];
        }
        let sorted = |key: Option<SortKey>| {
            self.prefs.arrangement == key.map_or(Arrangement::Free, Arrangement::Sorted)
        };
        let sort = |label: &str, key: Option<SortKey>| {
            item(label, true, sorted(key), DeskCommand::SortBy(key))
        };
        vec![
            item("New Folder", true, false, DeskCommand::NewFolder),
            KitItem::Separator,
            item(
                "Paste",
                !self.pasteboard.is_empty(),
                false,
                DeskCommand::Paste,
            ),
            KitItem::Separator,
            item(
                "Change Wallpaper\u{2026}",
                true,
                false,
                DeskCommand::ChangeWallpaper,
            ),
            KitItem::Separator,
            KitItem::Submenu {
                label: "Sort By".into(),
                enabled: true,
                items: vec![
                    sort("None", None),
                    KitItem::Separator,
                    sort("Name", Some(SortKey::Name)),
                    sort("Kind", Some(SortKey::Kind)),
                    sort("Date Modified", Some(SortKey::DateModified)),
                    sort("Size", Some(SortKey::Size)),
                ],
            },
            item(
                "Clean Up",
                self.prefs.arrangement == Arrangement::Free && !self.entries.is_empty(),
                false,
                DeskCommand::CleanUp,
            ),
        ]
    }

    fn shortcuts(&self) -> CommandRegistry<Message> {
        let mut registry = CommandRegistry::new();
        let key = |mods: Mods, keysym: Keysym| Shortcut { mods, keysym };
        let command = |c: DeskCommand| Message::Command(c);
        if self.rename.is_some() {
            registry.insert(key(Mods::NONE, ESCAPE), Message::NameCancelled);
            return registry;
        }
        let some = !self.selection.is_empty();
        if self.selection.len() == 1 {
            registry.insert(key(Mods::NONE, RETURN), command(DeskCommand::Rename));
            registry.insert(key(Mods::NONE, KP_ENTER), command(DeskCommand::Rename));
        }
        if some {
            for open in [Keysym(u32::from(b'o')), DOWN] {
                registry.insert(key(Mods::PRIMARY, open), command(DeskCommand::Open));
            }
            registry.insert(
                key(Mods::PRIMARY, BACKSPACE),
                command(DeskCommand::MoveToTrash),
            );
            registry.insert(
                key(Mods::PRIMARY, Keysym(u32::from(b'd'))),
                command(DeskCommand::Duplicate),
            );
            registry.insert(
                key(Mods::PRIMARY, Keysym(u32::from(b'c'))),
                command(DeskCommand::Copy),
            );
        }
        registry.insert(
            key(Mods::PRIMARY, Keysym(u32::from(b'v'))),
            command(DeskCommand::Paste),
        );
        registry.insert(
            key(Mods::PRIMARY, Keysym(u32::from(b'a'))),
            command(DeskCommand::SelectAll),
        );
        for n in *b"nN" {
            registry.insert(
                key(Mods::PRIMARY | Mods::SHIFT, Keysym(u32::from(n))),
                command(DeskCommand::NewFolder),
            );
        }
        for (keysym, direction) in [
            (LEFT, Direction::Left),
            (RIGHT, Direction::Right),
            (UP, Direction::Up),
            (DOWN, Direction::Down),
        ] {
            registry.insert(
                key(Mods::NONE, keysym),
                command(DeskCommand::Step(direction)),
            );
        }
        registry.insert(
            key(Mods::NONE, ESCAPE),
            command(DeskCommand::ClearSelection),
        );
        registry
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
                    cell: Rect::new(cell.x, cell.y, cell.w, cell.h),
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

    fn name_field(&self) -> Option<(usize, Element<Message>)> {
        let rename = self.rename.as_ref()?;
        let index = self.entries.iter().position(|e| e.path == rename.path)?;
        let field = rename.edit.field();
        let field = Element::new(TextField {
            label: "Name".into(),
            placeholder: String::new(),
            value: field.text().to_string(),
            revision: rename.revision,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: Some(Box::new(Message::NameChanged)),
            on_submit: Some(Message::NameSubmitted),
            selection: Some(field.selection()),
            initial: false,
        })
        .key(name_field_key())
        .width(m::LABEL_WIDTH + 2.0 * m::LABEL_PAD.0);
        Some((index, field))
    }
}

pub struct Desktop;

impl Application for Desktop {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;

    /// A desktop that lists nothing and no output feeds; [`run`] starts the process's one.
    fn init() -> (Model, Vec<Command<Message>>) {
        let model = Model::empty(PathBuf::new(), async_channel::unbounded().1);
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Role(note) => {
                match note {
                    RoleNote::Outputs(screens) => {
                        model
                            .sizes
                            .retain(|key, _| screens.iter().any(|s| s.key == *key));
                        if model
                            .menu
                            .as_ref()
                            .is_some_and(|menu| !screens.iter().any(|s| s.key == menu.output))
                        {
                            model.menu = None;
                        }
                        model.screens = screens;
                    }
                    RoleNote::Sized { key, width, height } => {
                        model.sizes.insert(key, (width, height));
                    }
                }
                model.relist();
                return vec![hear(&model.notes)];
            }
            Message::Woken => {
                model.poll();
                // a thumbnail that came shows at the next view
                let mut commands = model.rename_made();
                commands.push(listen(&model.woken));
                return commands;
            }
            Message::Unheard => {}
            Message::DroppedIn(dropped) => return model.dropped_in(dropped),
            Message::Pressed => {
                model.menu = None;
                model.end_rename();
            }
            Message::Select(paths) => model.selection = paths.into_iter().collect(),
            Message::Open(paths) => return model.open(paths, None),
            Message::Band(output, Some(rect), at) => {
                let started = match model.band {
                    Some((on, band)) if on == output && band.ended.is_none() => band.started,
                    _ => at,
                };
                model.band = Some((
                    output,
                    BandLook {
                        rect,
                        started,
                        ended: None,
                    },
                ));
            }
            Message::Band(_, None, at) => {
                if let Some((_, band)) = model.band.as_mut() {
                    band.ended = Some(at);
                    model.bands += 1;
                    return vec![after(m::BAND_FADE_OUT, Message::BandGone(model.bands))];
                }
            }
            Message::BandGone(band) => {
                if band == model.bands && model.band.is_some_and(|(_, b)| b.ended.is_some()) {
                    model.band = None;
                }
            }
            Message::ContextMenu { output, at } => {
                model.menu = Some(OpenMenu {
                    output,
                    at,
                    on_items: !model.selection.is_empty(),
                    open: Vec::new(),
                });
            }
            Message::MenuOpened(open) => {
                if let Some(menu) = model.menu.as_mut() {
                    menu.open = open;
                }
            }
            Message::MenuClosed => model.menu = None,
            Message::Command(command) => return model.command(command),
            Message::Drag(delta) => model.dragged = delta,
            Message::Drop { delta, onto } => model.drop_items(delta, onto),
            Message::BeginRename(path) => return model.begin_rename(path),
            Message::FocusName => {
                if model.rename.is_some() {
                    return vec![Command::Focus(name_field_key())];
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
            Message::Launch(what, token) => model.launch(what, token),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let items: std::rc::Rc<[ItemView]> = model.items().into();
        let empty: std::rc::Rc<[ItemView]> = Vec::new().into();
        let main = model.main().map(|s| s.key);
        let mut windows = Vec::new();
        for screen in &model.screens {
            let on_main = Some(screen.key) == main;
            let (width, height) = model
                .sizes
                .get(&screen.key)
                .copied()
                .unwrap_or((screen.width, screen.height));
            let desk = Desk {
                output: screen.key,
                label: DESKTOP_NAME.into(),
                items: if on_main {
                    std::rc::Rc::clone(&items)
                } else {
                    std::rc::Rc::clone(&empty)
                },
                band: model
                    .band
                    .filter(|(on, _)| *on == screen.key)
                    .map(|(_, band)| band),
                dragged: model.dragged.filter(|_| on_main),
                renaming: if on_main { model.name_field() } else { None },
            };
            windows.push(Window {
                key: desk_key(screen.key),
                spec: WindowSpec {
                    title: DESKTOP_NAME.into(),
                    app_id: format!("{APP_ID}@{}", screen.key),
                    size: Size::new(width, height),
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
                    takes_drops(Element::new(desk).fill())
                } else {
                    Element::new(desk).fill()
                },
                popup: None,
                menus: None,
            });
        }
        if let Some(menu) = &model.menu {
            if model.screens.iter().any(|s| s.key == menu.output) {
                windows.extend(
                    Menu {
                        parent: desk_key(menu.output),
                        anchor: MenuAnchor::At(menu.at),
                        label: DESKTOP_NAME.into(),
                        items: model.menu_items(menu.on_items),
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

/// Connect, list the desktop folder and show it until the connection goes.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    let folder = desktop_folder(&home);
    if let Err(e) = std::fs::create_dir_all(&folder) {
        eprintln!("cupertino-files: cannot make {}: {e}", folder.display());
    }
    let dirs = XdgDirs::from_env().map_err(other)?;
    let types = Arc::new(MimeDatabase::load(&dirs.all_data_dirs()).map_err(other)?);
    let folder_icons = session_places(&home, &dirs.data_dirs)
        .map(|p| p.folder_icons)
        .unwrap_or_default();
    let file_icons = FileIcons::discover((*types).clone(), folder_icons.into_iter().collect());

    let mut system = WaylandWindowSystem::connect(Profile::cupertino())
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let conn = system.connection();
    let shell = DesktopShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let screens = roles::screens(conn);
    let (notes_in, notes) = async_channel::unbounded();
    let system = system.with_roles(DesktopRoles {
        shell,
        surfaces: Vec::new(),
        notes: notes_in,
    });

    let (wake, woken) = async_channel::bounded(1);
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        // a wake already waiting covers this change too
        let _ = wake.try_send(());
    });
    let options = ModelOptions {
        types: Some(types),
        ..ModelOptions::default()
    };
    let listing = DirectoryModel::open_notifying(folder.clone(), options, Arc::clone(&wake))
        .map_err(|e| eprintln!("cupertino-files: cannot list {}: {e}", folder.display()))
        .ok();
    let prefs_file = arrangement::location();
    let mut model = Model::empty(folder, notes);
    model.prefs = prefs_file
        .as_deref()
        .map(DesktopPrefs::load)
        .unwrap_or_default();
    model.prefs_file = prefs_file;
    model.listing = listing;
    model.woken = woken;
    model.screens = screens;
    model.icons = Some(ItemIcons::new(
        file_icons,
        Some(Thumbnailer::from_env(super::APP_ID)),
        wake,
    ));
    model.notices = Notices::start(zbus::blocking::Connection::session().ok());
    let commands = vec![listen(&model.woken), hear(&model.notes)];
    model.menu_bar = cupertino_theme::Theme::load()
        .map(|theme| theme.shapes.menu_bar_height)
        .unwrap_or_else(|_| cupertino_theme::ShapeTokens::default().menu_bar_height)
        .ceil();
    model.poll();
    cupertino_kit::app::run_on::<Desktop>(system, (model, commands))?;
    Ok(())
}

/// The desktop's `content` taking a `text/uri-list` dropped from another application ([`Message::DroppedIn`]).
fn takes_drops(content: Element<Message>) -> Element<Message> {
    use cupertino_kit::core::{DropPlace, DropTarget};
    use cupertino_kit::platform::DropAction as Offered;
    Element::from(DropTarget {
        content,
        mimes: vec![super::drops::URI_LIST.into()],
        actions: [Offered::Copy].into(),
        place: Box::new(|over| {
            over.actions.contains(&Offered::Copy).then_some(DropPlace {
                key: None,
                action: Offered::Copy,
            })
        }),
        on_hover: None,
        on_drop: Box::new(Message::DroppedIn),
        on_leave: None,
    })
}

/// The local files a `text/uri-list` dropped on the desktop names, to copy into `folder`: none when the list is not of
/// local files, is empty, or holds an item already in `folder` (the desktop's own items move by its own drag).
fn dropped_sources(bytes: &[u8], folder: &Path) -> Option<Vec<PathBuf>> {
    super::drops::uri_paths(bytes)
        .filter(|paths| !paths.is_empty())
        .filter(|paths| paths.iter().all(|p| p.parent() != Some(folder)))
}

#[cfg(test)]
mod drop_tests {
    use super::dropped_sources;
    use std::path::{Path, PathBuf};

    #[test]
    fn files_from_another_folder_are_taken_and_the_desktops_own_or_none_are_refused() {
        let desk = Path::new("/home/u/Desktop");
        assert_eq!(
            dropped_sources(
                b"file:///home/u/Documents/a.txt\r\nfile:///usr/share/applications/b.desktop\r\n",
                desk
            ),
            Some(vec![
                PathBuf::from("/home/u/Documents/a.txt"),
                PathBuf::from("/usr/share/applications/b.desktop"),
            ])
        );
        assert_eq!(
            dropped_sources(b"file:///home/u/Desktop/c.txt\r\n", desk),
            None
        );
        assert_eq!(
            dropped_sources(
                b"file:///home/u/Documents/a.txt\r\nfile:///home/u/Desktop/c.txt\r\n",
                desk
            ),
            None
        );
        assert_eq!(dropped_sources(b"", desk), None);
        assert_eq!(dropped_sources(b"https://example.org/x\r\n", desk), None);
    }
}
