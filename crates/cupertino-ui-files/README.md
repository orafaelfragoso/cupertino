# cupertino-ui-files

CupertinoKit's file family: the items of a folder, their order, types, tags and names, the icon layout, opening,
renaming and the file operations behind the file views, and the views' measured metrics. Programs name it as
`kit::files`, item by item (`cupertino-kit`'s *Public API*); a `shell` or `apps` crate never depends on it by name.
Files names it now, the desktop component (W6.10) and the dialogs component (W6.11) after it. GPL-3.0-or-later.

It is a crate of its own for D32's fourth reason (`docs/plans/layered-architecture.md`): `cupertino-ui-controls` is
28 005 lines, the family about 8 800 more with dependencies no control needs (`cupertino-apps`, `cupertino-trash`,
`xattr`, `tree_magic_mini`, `mime_guess`, `libc`, `serde`, and with W4.12 more), and the two split at the controls'
public items: the family builds on them and `cupertino-ui-controls` gains no dependency.

## Modules and API

| Module | Role | Main items |
|---|---|---|
| `entry` | an item of a folder | `Entry` (name, path, `Kind`, size, dates, device and inode, hidden, tags, type, `NameKey`, an application's icon), `Entry::from_path`, `extension_of` |
| `natural`, `sort` | name collation (case-, width-insensitive, numeric, localized) and ordering | `NameKey`, `compare_names`, `type_select_key`; `SortSpec { key, ascending, folders_first }`, `SortKey`, `sort_entries`, `insertion_index`, `compare` |
| `mime`, `tags` | metadata | `detect`, `entry_mime`, `kind_group` (`KindGroup`), `kind_description`; `read_tags` / `write_tags` (`user.xdg.tags`), `add_tag`, `remove_tag`, `parse_tag_list`, `TagColor` (seven label colours), `parse_reference_tags` |
| `applications`, `recents` | the two virtual folders | `location`, `application_dirs`, `session_entries`, `any_installed`; `location`, `recent_entries`, `parse_xbel` and `XBEL_NAME` (`cupertino_apps::recent`'s) |
| `places` | the places of a sidebar, over the user directories of `cupertino_defaults::user_dirs` | `Place` (name, path, `PlaceGroup`, `PlaceKind` with its symbols and folder icon; `Volume`, `Medium` of a removable file system), `standard_places` (a file browser's sidebar: the home folder, Applications, Desktop, Documents, Downloads, the computer, the network, Trash), `chooser_places` (a file chooser's: Home, then Desktop, Documents, Downloads, Music, Pictures and Videos where they exist), `session_places` (`SessionPlaces`: the running session's, with `folder_icons`), `desktop_folder`, `recents_place`, `follow_trash`, `removable_holding`, `window_title`, `computer_name`, `network_root` |
| `model` | a folder's items as a sorted list that follows its changes, read by work the program runs | `DirectoryModel::{open, open_recents, open_applications}` (touch no file), `take_work` (one `ModelWork` at a time, in the order asked: the first checks, watches, opens and lists the folder in batches, a later one reads the items a change named or lists a virtual folder again; `ModelWork::run` keeps a panic of the work as `OpenFailed` or `Error`), `take_watch` (`ModelWatch::follow`, a future that waits on an inotify descriptor through `async-io`'s reactor and ends when its model drops), `poll` → `ModelDelta` (`Opened` or `OpenFailed { kind, error }` first, then `Reloaded`, `LoadComplete`, `Inserted`, `Removed`, `Changed`, `Moved`, `DirectoryGone`, `Error`), `opened`, `is_working`, `set_notify`, `set_sort`, `entries`; `ModelOptions`, `Notify`, `WORK_DEADLINE` (10 s), `enumerate_blocking` |
| `layout::icon`, `geom` | icon view | `ideal_cell_size`, `max_title_lines`, `layout` (flow / justified, 32 pt section headers), `strip` (a gallery's row), `IconLayout::{hit, hits_in_rect, neighbour}`, `FreeGrid`, in `cupertino-geometry`'s points, sizes and rectangles; `Direction` |
| `rename`, `name_edit` | names | `RenameTrigger` (a press on the only selected item's name renames it after the double-click interval), `validate_name`, `needs_extension_warning`, `initial_selection`, `unique_name`, `NameError`; `NameEdit::{field, field_mut, commit}` |
| `open` | opening files | `open_command` (default application of `mimeapps.list` through `cupertino-apps`; `Terminal=true` under `--terminal`), `command_with`, `open_with` (Open With list: default first, rest by name), `application_at`, `default_app_id`, `OpenError` |
| `operations` | file changes, blocking functions the program runs off its UI thread | `rename_item` (no replace), `new_folder`, `new_folder_with`, `make_alias`, `duplicate`, `drop_action`, `same_volume`, `drop_into` (move or copy, `name copy` on a clash), `transfer` (the same for a list of items, with `Progress` and a `Cancel`), `duplicate_with`, `stopped`, `is_cancelled`; `Transfer::{into_folder, duplicate, take_slice, progress, cancel, results}`, a copy or a move in slices: a `TransferSlice` copies until the first megabyte or item that ends past `SLICE` (a tenth of a second), within `SLICE_DEADLINE` (10 s), and `transfer`, `drop_into` and `duplicate_with` run its slices one after the other. `new_folder_with` stops at the first item that fails |
| `file_icons` | the icon of an item's type, as image renditions | `FileIcons::{discover, new, builtin, without_host, name, pack_has, images, database}`, `RENDITION_SCALES`, `document_icon_name`, `FILE_ICON_NAMES` |
| `thumbnail_service` | the thumbnails of the items in view, made by jobs the program runs | `ThumbnailService::{new, want, forget, get, failed, queued, memory, take_job}` (a `ThumbnailJob` while fewer than `JOBS`, two, are being made; `ThumbnailJob::run` reads and decodes, and keeps a panic as the file's failure), `Stamp`, `Thumbnail`, `has_thumbnail`, `MEMORY_BUDGET`, `JOB_DEADLINE` (5 s) |
| `item_icons` | what a view draws for an item | `ItemIcons::{new, with_app_icons, icon, thumbnail, preview_picture, want, want_previews, forget, thumbnails, take_job}`: a picture's thumbnail, the family's art, an application's own icon, else its type's icon |
| `notice` | failures as notifications | `failure`, `unopened`, `bad_name`, `finished` (an operation that ended unseen) → `Notice`; `Notices::{logged, on, post, post_clickable, clicks, sent}` (`on` gives the future that posts them in order on the program's connection and hears their clicks, `Click`); `shown`, `escape`, `explained` (an error as a sentence for a window) |
| `item_art` | the common items' art | `Art::{for_icon}` (folder, text, document, archive), `draw(art, px, seed)` (premultiplied RGBA) |
| `metrics` | the views' measured values | `icon_view`, `list_view`, `column_view`, `timing`, `selection`, `marquee`, `drag` |
| `grid` | the folder as a grid of icons with their names | `IconGrid<M>` (the content of the program's scroll view), `ItemIcon`, `NameShape`, `Grab`, `name` and `laid_out_name` (an item's name as the grid draws and lays it out), `cell_key`, `cell_index` |
| `list` | the folder as an outline table: a header, a row an item with its icon and name first, folders that open in place, the band and the presses of its rows | `FileList<M>` (an element with `Element::from`), `Presses` (what a list's rows and its frame share, or a column's: the program keeps one a window between views), `RowGrab`, `row_key`, `ROW_HEIGHT`, `ROW_ICON` |
| `columns` | a chain of folders side by side in a row that scrolls sideways, a divider after each column that resizes it | `FileColumns<M>` (an element with `Element::from`), `FileColumn` (the folder shown, another folder, one not listed yet, and the program's preview at the end), `column_key`, `ROW_HEIGHT`, `ROW_ICON` |
| `band`, `clicks` (private) | the rubber band of the grid and the list, and what a press on an item means besides selecting it: a double click opens, a press on the only selected item's name renames it after the double-click interval | |
| the crate's root | what a view reports | `ViewEvent`, `ColumnEvent` |

The family starts no thread and touches no file on its own: the functions that read or write files are blocking
functions their caller runs, or work a value hands out that its caller runs (a model's `ModelWork`, a
`ThumbnailJob`), or a future it runs on its loop (a model's watch, the notices' posting), and only
`operations` and the tag functions write, into locations the caller gives. A program runs a model like this:

```rust
use std::sync::Arc;
use cupertino_ui_files::model::{DirectoryModel, ModelDelta, ModelOptions};

let options = ModelOptions { watch: false, ..ModelOptions::default() };
let mut model = DirectoryModel::open("/home/user/Pictures", options, Arc::new(|| {}));
// a program hands each work to `kit::task::io` and polls when `notify` wakes it
while let Some(work) = model.take_work() {
    work.run();
    for delta in model.poll() {
        if let ModelDelta::LoadComplete { count } = delta {
            println!("{count} items");
        }
    }
}
```

`cargo bench -p cupertino-ui-files --bench directory` times the order of 50 000 names, the icon layout and a model's
listing of 50 000 files.

The views' tests drive each view in a window on the headless window system through its public items
(`tests/icon-grid.rs`, `tests/file-list.rs`, `tests/file-columns.rs`).

## Design

The views below are built as the design the owner approved for the family (W4.8 in the plan), and so is what follows
them (W4.12): the places, a folder's listing and the thumbnails as work, the notices on the program's connection and
the transfers in slices.

### What shapes it

- Files' views emit Files' own message (`Message::Window(id, Event)`, 62 events) and its column view reads the app's
  session and window. A view in Kit knows neither, so the views do not come over with their bodies as they are.
- A `shell` crate cannot depend on `cupertino-files`. What the desktop component (W6.10) and the dialogs component
  (W6.11) share with Files is in Kit before they start, and it starts no thread: in the shell host a thread is the
  host's to start, so that its panic is the component's.
- `kit::task` and its pools are in `cupertino-kit`, above the `cupertino-ui-*` crates, so nothing in the family
  starts a task. It gives blocking functions and futures, and the program runs them (`kit::task::io`,
  `Command::Perform`), as it runs a `filepicker::Request`.
- An application's message is `Clone + Send`, so it cannot carry an open folder or a watch. A model keeps the queue
  its work fills, and the message only says there is something to `poll`.

### The views

A view is a widget with public fields, generic over the program's message `M`, as every control is. It takes plain
values (the items, the selection, the scroll state, a function for an item's icon and one for the field a name is
edited in), never a model, a session or a window. The three report through one event type and one function, which
answers `None` for an event the program has no use for: nothing is sent for it. The toolkit's table and lists the
list and the columns are made of report `Select`, `Open`, `Sort`, `Toggle`, `Scroll` and a column's `Scroll` and
`Pick` through handlers that must give a message, so a list's or a column view's function gives one for each of
those.

```rust
/// What a file view reports. The program gives each view one function that turns an event into its message, or
/// into none.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewEvent {
    /// The selection a press, a key, the band or typing made.
    Select(Selection),
    /// Open item `index`: a double click or Return.
    Open(usize),
    /// A secondary press on item `index`, or on empty space.
    SecondaryPress(Option<usize>),
    /// Open the context menu at this point of the window.
    ContextMenu(Point),
    /// A press on items of the grid went far enough to drag them.
    DragOut(Grab),
    /// A press on rows of the list, or of the shown folder's column, went far enough to drag them.
    DragRows(RowGrab),
    /// The rubber band being drawn, in the view's content; `None` once it is let go.
    Band(Option<Rect>),
    /// The view scrolled: its scroll view, or a band held past its edge.
    Scroll(ScrollState),
    /// A press on the name of the only selected item had no second press: edit the name of item `index`.
    Rename(usize),
    /// Tab chose row `index` of the list.
    FocusRow(usize),
    /// A column header of the list was chosen.
    Sort((usize, SortOrder)),
    /// A folder's row of the list opens or closes.
    Toggle(TreeToggle),
    /// The column view's own events.
    Column(ColumnEvent),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColumnEvent {
    /// The column of this folder scrolled.
    Scroll(PathBuf, ScrollState),
    /// Row `index` of the column of `folder`, a folder other than the one shown, was chosen; `secondary` when a
    /// secondary press chose it, whose context menu follows.
    Pick { folder: PathBuf, index: usize, secondary: bool },
    /// Right (1) or Left (-1) in the column of the folder shown.
    Step(isize),
    /// A divider was dragged to make column `index` `width` wide; `all` with Option held.
    Resize { index: usize, width: f64, all: bool },
    /// The divider was let go.
    Resized,
}

/// The presses a list's rows and its frame share, or a column's: the program keeps one a window between views. A
/// clone is the same presses. Nothing of it is public.
#[derive(Clone, Default)]
pub struct Presses(/* private */);

/// The folder as a grid of icons with their names: the content of the program's scroll view. Its fields are the ones
/// Files' grid has, and `on_event`; its body differs only where it emits.
pub struct IconGrid<M> {
    /// Tells this view's keys from those of the program's other windows.
    pub window: u64,
    pub label: String,
    pub layout: Rc<IconLayout>,
    pub params: IconParams,
    pub items: Rc<[Entry]>,
    pub icon: ItemIcon, // Rc<dyn Fn(&Entry) -> Vec<ImageSource>>
    pub scroll: ScrollState,
    pub selection: Selection,
    /// The items a context menu is open on, which wear a ring.
    pub ringed: Selection,
    pub band: Option<Rect>,
    /// How far down its pane the first item starts at rest: what a toolbar row covers.
    pub lead: f64,
    /// What sits under item `index`'s icon, given `(index, selected)`: its `name`, or the field it is renamed in.
    pub name: Rc<dyn Fn(usize, bool) -> Element<M>>,
    pub names: Option<NameShape>,
    pub revision: u64,
    pub strip: bool,
    pub on_event: Rc<dyn Fn(ViewEvent) -> Option<M>>,
}

/// The folder as an outline table: a header, a row an item with its icon and name first, folders that open in place,
/// the band and the presses of its rows.
pub struct FileList<M> {
    pub window: u64,
    pub label: String,
    /// The rows' items in order: an open folder's items follow its row.
    pub items: Rc<[Entry]>,
    /// Each row's place in the outline; a row past its end is a top-level row without children.
    pub outline: Rc<[TreeItem]>,
    /// The columns, the name's first: a table's `Column`, a title and a width.
    pub columns: Vec<Column>,
    /// The text of an item in column `column`, from 1; the name's cell is the item's icon and name.
    pub text: Rc<dyn Fn(&Entry, usize) -> String>,
    pub sort: Option<(usize, SortOrder)>,
    pub icon: ItemIcon,
    pub multiple: bool,
    pub scroll: ScrollState,
    pub selection: Selection,
    pub ringed: Selection,
    pub band: Option<Rect>,
    /// From the pane's top to the header at rest.
    pub lead: f64,
    /// The row whose name is being edited, and the field that stands in its place; the field's messages are the
    /// program's own.
    pub name_field: Option<(usize, Rc<dyn Fn() -> Element<M>>)>,
    pub presses: Presses,
    pub on_event: Rc<dyn Fn(ViewEvent) -> Option<M>>,
}

/// A chain of folders side by side in a row that scrolls sideways, a divider after each column that resizes it.
pub struct FileColumns<M> {
    pub window: u64,
    /// The columns in order, each as wide as the program lays it out.
    pub columns: Vec<(FileColumn<M>, f64)>,
    /// The width of the chain's content, which is the columns' widths together or more: a chain scrolled to a whole
    /// column keeps room after its last one.
    pub content: f64,
    /// The chain's sideways scroll, reported as `ViewEvent::Scroll`; a column's own is `ColumnEvent::Scroll`.
    pub scroll: ScrollState,
    pub lead: f64,
    pub icon: ItemIcon,
    pub on_event: Rc<dyn Fn(ViewEvent) -> Option<M>>,
}

pub enum FileColumn<M> {
    /// The folder the program shows: its selection, its rename and its presses, as a list's rows have them.
    Shown {
        folder: PathBuf,
        label: String,
        items: Rc<[Entry]>,
        scroll: ScrollState,
        selection: Selection,
        name_field: Option<(usize, Rc<dyn Fn() -> Element<M>>)>,
        presses: Presses,
    },
    /// Another folder of the chain; the row of the chain's next folder is highlighted.
    Other { folder: PathBuf, label: String, items: Rc<[Entry]>, scroll: ScrollState, highlighted: Option<usize> },
    /// A folder whose items have not come yet.
    Empty { folder: PathBuf },
    /// What ends the chain, built by the program: the preview of the selected file. It has no divider.
    Preview(Element<M>),
}
```

`FileList` and `FileColumns` become elements with `Element::from`. Public beside them: `name` and `laid_out_name` (an
item's name as the grid draws and lays it out, which the desktop's items use), `NameShape`, `ItemIcon`, `Grab`,
`RowGrab`, and the keys `cell_key`, `cell_index`, `row_key` and `column_key`. The widgets the views are made of
(cells, name boxes, row cells, the band, the click and rename timing, the column frame and its divider) are private.

Files keeps what is its own around them: the scroll view and the kept layouts around the grid, the gallery, the drag
images and commands (`dragged`), the drop targets, the chain's listings, layout and navigation (`ColumnsView`, the
`columns` chain, whose `ChainLayout` gives the widths and the content width), the preview pane and window, the name
field. Its `Event` and its `update` do not change: one function maps the family's events onto the events it has.

```rust
// cupertino-files
fn view_events(id: u64) -> Rc<dyn Fn(ViewEvent) -> Option<Message>> {
    Rc::new(move |event| {
        let event = match event {
            ViewEvent::Select(selection) => Event::Select(selection),
            ViewEvent::Rename(index) => Event::RenameItem(index),
            ViewEvent::Column(ColumnEvent::Step(by)) => Event::ColumnStep(by),
            // one arm an event, each to the `Event` Files has for it
        };
        Some(Message::Window(id, event))
    })
}

// the chooser, once its rows are a `FileList` (W6.11): three of the messages it has
Rc::new(move |event| match event {
    ViewEvent::Select(rows) => Some(Message::Select(id, rows)),
    ViewEvent::Open(row) => Some(Message::Open(id, row)),
    ViewEvent::Scroll(scroll) => Some(Message::Scroll(id, scroll)),
    _ => None,
})
```

### Where Files' modules go

To the family goes what a view names, what the desktop component needs, and what those two stand on. What only Files
uses stays a module of Files (D32).

| Files' module | In `cupertino-ui-files` as | Because | Task |
|---|---|---|---|
| `entry`, `natural` | the same | the views' item and the order of names | W4.8 |
| `sort`, `mime`, `tags`, `applications`, `recents` | the same | the listing sorts, types and tags its items and lists the two virtual folders | W4.8 |
| `layout::icon`; `metrics::{icon_view, list_view, column_view, timing, selection, marquee, drag}` | the same | the views' layout and design values | W4.8 |
| `geom` | `Direction` alone | its `Point`, `Size` and `Rect` are `cupertino-geometry`'s, which the controls draw with | W4.8 |
| `rename`, `name_edit`, `open`, `operations`, `item_art` | the same | D8's operations as the blocking functions they are; the art of the common items | W4.8 |
| `kit::{grid, list, band, clicks}`; the widgets and assembly of `kit::columns`; the list's assembly of `kit::view`; `Grab` of `kit::dragged` | `grid`, `list`, `columns` | the views | W4.8 |
| `places` | the same, the user directories read through `cupertino-defaults` ([Places](#places-and-user-directories)) | Files' sidebar, the chooser's, the desktop's folder | W4.12 |
| `model` | the same, without threads ([Slow work](#slow-work)) | Files and the desktop list folders | W4.12 |
| `thumbnail_service`, `kit::item_icons` | the same, as jobs; `file_icons` comes down from `cupertino-kit` beside them | Files and the desktop draw the same icons and thumbnails | W4.12 |
| `notice` | the same, on the program's bus connection | Files and the desktop post failures | W4.12 |

Stay in Files: `browser`, `history`, `selection`, `typeselect`, `settings`, `layout::list`, `columns` (the chain),
`preview`, `format`, `group`, `window`, `window_prefs`, `tab_strip`, `search_field`, `spring`, `context_menu`,
`menu_spec`, `menu_bar`, `instance`, `udisks`, the rest of `metrics` and of `kit`. `udisks` is the sidebar's removable
media, which no other program lists. `desktop`, `kit::desktop` and `metrics::desktop` stay until W6.10 takes them to
`cupertino-shell-desktop`. There is no `trash` module to bring: `cupertino-trash` is a `system` crate, and the
family's operations and places call it by its name.

### Slow work

The family starts no thread, and its values on the UI thread touch no file: no read, no `stat`, no watch
registration. Work is one of three things: a blocking function the program runs with `kit::task::io`; a job a value
hands out (`take_work`, `take_job`, `take_slice`) that the program runs the same way and that leaves its result in
the value's queue; or a future that waits without touching a file, which the program runs with `Command::Perform` on
its UI loop (a folder's watch, the notices on the bus connection).

```rust
impl DirectoryModel {
    /// A model of `dir`. It touches no file: `notify` wakes the program whenever there is something to `poll`.
    pub fn open(dir: impl Into<PathBuf>, opts: ModelOptions, notify: Notify) -> DirectoryModel;
    /// The model's next blocking work; `None` while one of its works is out, and when none waits. A model runs one
    /// work at a time, in the order asked: a change is applied on top of the one before it. The first work checks
    /// that the folder is one, registers its watch, opens it and then lists it in batches (for Applications it
    /// walks the application folders and registers each); a later one reads the items a change named, or lists a
    /// virtual folder again.
    pub fn take_work(&mut self) -> Option<ModelWork>;
    /// The folder's watch, once; `None` without `opts.watch`. Its descriptor is made here and holds no folder until
    /// the first work registers one.
    pub fn take_watch(&mut self) -> Option<ModelWatch>;
}
impl ModelWork {
    /// Blocking. What it read is in the model's queue when it returns. It catches a panic of the work, so the panic
    /// is not the instance's: the first work then leaves `OpenFailed`, a later one `Error`.
    pub fn run(self);
}
impl ModelWatch {
    /// Waits on the descriptor, hands each burst of changed names to the model and wakes it. It touches no file. It
    /// ends as soon as its model is dropped, with no event needed, and closes the descriptor.
    pub fn follow(self) -> impl Future<Output = ()> + Send + 'static;
}

pub enum ModelDelta {
    /// The folder is a folder, is watched and can be read: the first update of a model that opens.
    Opened,
    /// The folder could not be opened (it is missing, is not a folder, gives no read permission, cannot be
    /// reached): the model's first and only update, with the error's kind and its text.
    OpenFailed { kind: io::ErrorKind, error: String },
    // the others as they are; `Error(String)` stays a read that fails once the folder is open
}

impl ThumbnailService {
    /// The next thumbnail to make, while fewer than `JOBS` (two, the count `WORKERS` holds) are being made.
    pub fn take_job(&self) -> Option<ThumbnailJob>;
}
impl ThumbnailJob {
    /// Blocking: reads and decodes. It catches a decoder's panic, as the worker does, and keeps it as the file's
    /// failure, so the panic is not the instance's.
    pub fn run(self);
}

impl Notices {
    /// Notices that are logged, not posted: the session has no bus.
    pub fn logged() -> Notices;
    /// Notices posted on `session` in order, and the future that posts them and hears their clicks.
    pub fn on(session: zbus::Connection) -> (Notices, impl Future<Output = ()> + Send + 'static);
    /// Ends once every notice posted so far is sent.
    pub fn sent(&self) -> impl Future<Output = ()> + Send + 'static;
}

/// A copy or a move that runs in slices, so it holds an I/O turn for `operations::SLICE` at a time.
pub struct Transfer(/* private */);
impl Transfer {
    pub fn into_folder(sources: Vec<PathBuf>, dest: PathBuf, action: DropAction) -> Transfer;
    pub fn duplicate(paths: Vec<PathBuf>) -> Transfer;
    /// The next slice; `None` while one is out and once the transfer has ended.
    pub fn take_slice(&mut self) -> Option<TransferSlice>;
    /// How far it has come, as of the last slice.
    pub fn progress(&self) -> Progress;
    /// Stops it: the next slice removes the item being copied and ends the transfer.
    pub fn cancel(&mut self);
    /// One result a source, in order, once it has ended.
    pub fn results(&mut self) -> Option<Vec<io::Result<PathBuf>>>;
}
impl TransferSlice {
    /// Blocking: copies until the first megabyte or item that ends past `SLICE`.
    pub fn run(self);
}
```

`open_recents` and `open_applications` have the same form; `poll`, `entries`, `set_sort`, `want`, `forget`, `get`,
`post`, `post_clickable` and `clicks` keep their signatures. `transfer`, `duplicate_with` and `drop_into` stay, as a
loop over a `Transfer`'s slices.

| Now, in Files | In the family | What changes for the person |
|---|---|---|
| `model`: a thread a folder that lists it; the `notify` crate's thread for changes; on the UI thread, `open` reads the folder's metadata and registers the watch (for Applications a walk of every application folder), and `poll` reads the items a change names; dropping a model waits for its thread | `take_work`, run as `kit::task::io`, one at a time a model; `ModelWatch::follow` waits on an inotify descriptor through the reactor the bus connection waits on, as the settings watch does | The window stays on the folder it shows, and answers, until the folder asked for opens ([below](#a-folder-that-does-not-open)). A change shows one turn of the loop later. Closing a window never waits for a listing. |
| `thumbnail_service`: two worker threads | `take_job`, run as `kit::task::io`, two at once | None at rest. A decoder's panic still marks the file as failed. |
| `notice`: one thread that posts with blocking calls, one that hears clicks; `flush` blocks the exit | one future on the UI loop, on the connection the program gives; `sent` instead of `flush` | Files exits when `sent` ends or after `EXIT_NOTICES`, without blocking. |
| `operations`: `transfer` copies a whole tree in one call, on its caller's thread | `Transfer`, a slice a task | None in Files until W7.1, whose transfer thread runs the same loop. |
| `open`, `places`, the other `operations`: blocking functions | as they are | None. |

The deadlines are named constants beside the work: 10 s for a model's work, 5 s for a thumbnail, 10 s for a slice;
`SLICE` is a tenth of a second.

A `Command::Perform` future lives until it ends or the application's executor is dropped, and a user has 128 inotify
instances by default. So `follow` does not wait for a file event to learn its model is gone: the model's drop closes
a channel the future waits on beside the descriptor.

#### A folder that does not open

`open` cannot fail, so the browser decides at the model's first update. It keeps the listing it shows until the model
of the folder asked for gives that update. `Opened` puts the model in its place, and only then does the history take
the folder and the window count as entered. `OpenFailed` drops the model and posts the notice a failed go, Back,
Forward or Parent posts now, with the error's text. A failed open therefore adds no history entry and shows no empty
folder, as now. A second request while one waits takes its place. A column of the column view and an open row of the
list wait the same way: the column stays empty and the row closed until their model opens.

A folder without read permission does not open, as now. Files watches every folder it lists, and registering the
watch fails with `EACCES` before anything is read (`start` in `model.rs`), so the window stays where it is with the
notice. In the family the first work reports it as `OpenFailed` with `PermissionDenied`; the notice then words the
kernel's error, where it words the watch crate's now. A model opened without a watch reports the same, where its
folder shows empty with an `Error` now: no caller of the family shows a folder it cannot read as an empty one.

#### Mounts that do not answer

Any step of a model's work can hang on a mount that does not answer: the open check, the watch registration, the
read of the folder, and the `stat` of each item, which follows symbolic links (`Entry::from_path`), so a folder on a
healthy disk hangs too when it holds a link into such a mount. A thumbnail's read and a slice hang the same way.

One instance runs at most `task::IO_SHARE` (four) I/O tasks at once, and Files is one instance for all its windows.
Each hung work holds a turn until the kernel gives up. With four held, Files lists nothing, makes no thumbnail, reads
no change and copies nothing, in any window, until one comes back; its windows still draw and answer. Now each
listing has a thread of its own, so other folders go on, and closing a window on a hung folder blocks the UI thread
instead.

What limits it:

- A model runs one work at a time, so a hung folder holds one turn however many changes it is told of.
- Files does not open a folder twice while it hangs. A model that is let go while its work runs or is overdue (a
  request replaced, a window or a tab closed, a column gone) is kept by its folder in the session, and the next
  request for that folder takes it instead of opening another. It leaves when its work ends.
- Kit reports each overdue work once (`toolkit.task.overdue`), with the instance and the call.

What remains: four folders that hang at once stop Files' I/O, and two windows that wait for the same hung folder hold
a turn each. A larger share for a program that is the only instance of its process is Kit's to give
(`task::IO_SHARE`, W4.5), not this family's.

#### Transfers

A copy is not one I/O task: it would hold a turn for minutes and pass any deadline its caller could name. A
`Transfer` copies in slices and gives its turn back between them, so listings and thumbnails go on beside it and a
stop takes effect at the next slice. Moving to the trash and emptying it are an item a task: `cupertino-trash`'s
functions take one item each. The desktop component (W6.10) is the first caller that runs a `Transfer` as tasks;
Files follows in W7.1.

Not in these tasks, because their threads are their callers': Files' transfer thread (`kit/transfer.rs`), the preview
window's read, the look-ups, the single-instance service, the places watch and `udisks` are Files' until W7.1; the
desktop's `desktop-files` and `trash-sizes` threads are W6.10's; the chooser's `read` and `arm` threads are W6.11's.

### The open and save panel

The panel keeps its rows in wave 4. The chooser draws a plain list of 24 pt rows in `dialog.rs`, which it shares
with the application chooser, the sharing picker and two consent dialogs, and which W6.4, W6.11 and W6.14 take apart;
the portal has no capture example. Rows that are a `FileList` are another look (a header, Files' row height and
selection, perhaps its columns) and another order (the picker lists folders first by lower-cased name, the family by
natural order with the sort the program gives), and both need the owner's eye on a sheet.

- Wave 4 shares what is the same already: the places (below) and the row icons (`file_icons`).
- W6.11, which moves the chooser into `cupertino-shell-dialogs` and gives it a capture example, gains a step: the
  panel is a view of `kit::files` (the sidebar of places, a `FileList`, the Save As field, the filters and choices,
  the buttons), built from the chooser's model and the picker, which lists the family's `Entry` in the family's
  order; `filepicker::Entry` and its reader go (the second pair of copies, which meets only then), and `filepicker`
  may join `cupertino-ui-files`. It is a look and behaviour change, shown on a review sheet of the panel before and
  after, open and save, light and dark, for the owner.
- Settings' chooser sheet (`Chooser` in `cupertino-settings/src/sheet.rs`), which draws rows of its own on the same
  `Picker`, moves onto that view in W7.2.
- What a single-selection list does with a band, and whether the panel's sidebar is Files' sidebar, are settled on
  that sheet.

### Places and user directories

`user-dirs.dirs` has four parsers: Files', `cupertino-ui-controls`' `filepicker::places` (which the portal's chooser
and its screenshot directory name through Kit), `cupertino-screenshot`'s and the dock's. The portal becomes a service
that never uses Kit (W6.11), so the one reader is below Kit, beside `config_home`:

```rust
// cupertino-defaults
pub mod user_dirs {
    /// The `XDG_<NAME>_DIR` assignments of a `user-dirs.dirs` text as `(NAME, path)`, `$HOME` expanded against `home`.
    pub fn parse(text: &str, home: &Path) -> Vec<(String, PathBuf)>;
    /// The assignments of the session's `user-dirs.dirs`: in `$XDG_CONFIG_HOME` when that is an absolute path, else
    /// in `.config` of `home`. A relative `XDG_CONFIG_HOME` is not valid and is passed over, as the base directory
    /// specification says. None when the file is missing; an error when it is there and cannot be read. It reads a
    /// file.
    pub fn read(home: &Path) -> io::Result<Vec<(String, PathBuf)>>;
}
```

Files' `places` stays, as the family's `places`: its `Place` (name, path, group, kind), `standard_places`,
`folder_icons`, `desktop_folder`, `session_places`. `filepicker::places` goes, with its `Place`, `places` and
`session_places`. The two lists differ and both stay, over one table of user directories: `standard_places` is Files'
sidebar, and `chooser_places(home, &assigned) -> Vec<Place>` is the chooser's (Home, then Desktop, Documents,
Downloads, Music, Pictures and Videos where they exist), with the names and symbols its rows have now.

| Caller | Effect |
|---|---|
| Files | A relative `XDG_CONFIG_HOME` is passed over for this file, where Files uses it now. A missing file assigns nothing and an unreadable one is an error at start, as now. |
| The portal's chooser | The same rows and the same `XDG_CONFIG_HOME` rule. A file that cannot be read is one line on standard error and the sidebar of a missing file; `filepicker::places` says nothing. |
| The portal's screenshot directory | Reads `cupertino_defaults::user_dirs`; the same line; `~/Pictures` when nothing is assigned or readable. |
| Settings' sheet | None: it names `filepicker::Picker`, not the places. |
| `cupertino-screenshot`, the dock | None now. Their copies go in W6.4 and W6.8, which own those crates. |

`config_home()` keeps its rule for the settings files, where a relative value is used; whether it follows the
specification too is not these tasks' to change.

### The two tasks

W4.8 changes no behaviour and no look. Everything that changes behaviour is W4.12, a task of its own after it and
before W6.10.

- **The same pixels.** `files_capture` captures every sheet with `--out` before and after a step, at 1× and 2×,
  light and dark, and the files are the same (`diff -r` is empty): `files/window` in the icon, list, column and
  gallery views, each at rest, with an item selected, with a name being edited (`--state rename`), with a ring under
  an open context menu (`--state ring`) and with a band held over items (`--state band`); `files/sidebar`,
  `files/preview`, `files/popover`, `files/tabs`; and `files/desktop`, the desktop's items over a flat fill.
- **What a capture cannot show.** The drag's image and the glass under the toolbar and the menus, which the
  compositor draws; anything that is time (the delay before a rename, the band following the pointer past an edge,
  type-to-select); where the keyboard goes.
- **Tests.** Each view step writes its view's tests in `tests/`, driven through the headless driver: the selection a
  press, a key, the band and typing make; open; the rename press and its delay; Tab by name; a drag starting; the
  secondary press and the menu's point; for the columns, pick, step and resize. They are the W4.T tests of the views.

#### W4.12 File services as work (L, behaviour change)

**Owns** `ui-files`; `files`; `kit` (`file_icons.rs`, the `files` and `filepicker` lists of `lib.rs`, the README);
`ui-controls` (`filepicker::places`); `defaults` (`user_dirs`); `portal` (where `dialog.rs` takes its places from,
`pictures` in `screenshot.rs`). **Touches** the manifests of `ui-files files kit portal`. **After** W4.8.

| # | Step | Size | Done when |
|---|---|---|---|
| 1 | **Places and user directories: one copy.** `cupertino_defaults::user_dirs`; Files' `places` is the family's, with `chooser_places`; `filepicker::places` goes; the portal's chooser and `pictures` use them. Changes: [Places](#places-and-user-directories). | S | `rg parse_user_dirs crates` finds only `cupertino-screenshot`; a test of `user_dirs` in `crates/cupertino-defaults/tests/` (a relative `XDG_CONFIG_HOME` among its rows); a test that `chooser_places` gives the rows the chooser has now; the portal's diff holds no drawing code; same pixels in Files. |
| 2 | **Listings as work.** `model` is the family's, in the form above, and the `directory` bench comes with it; the browser waits for a model's first update; the session keeps the models whose work runs. Changes: [Slow work](#slow-work). | L | no `std::thread` and no `notify` in the model; `tests/model.rs`: `Opened` then batches; `OpenFailed` with its kind for a missing folder, a file and a folder without read permission, with and without a watch; a created, renamed and removed item seen through the watch the first work registered; works one at a time and in order; `follow` ends when its model is dropped with no event. In Files: a failed go, Back, Forward and Parent leave the listing and the history as they are and post the notice; a folder asked for again while its work runs opens no second model. Same pixels. |
| 3 | **Thumbnails and item icons.** `thumbnail_service` and `item_icons` are the family's, as jobs; `file_icons.rs` comes down from `cupertino-kit`, whose `kit::file_icons` names the same items. | M | no `std::thread` in them; `tests/thumbnails.rs`: wanted first, two at once, the budget, a failure kept, a decoder's panic kept as a failure; same pixels, thumbnails included. |
| 4 | **Notices.** `notice` is the family's, on the program's connection. Until W7.1 Files makes its session connection before its loop starts and hands its async half to `Notices`. | S | no `std::thread` and no `zbus::blocking` in it; `tests/notices.rs` against a stand-in service on a peer connection: order kept, a click back with its tag and token, `sent` ends. |
| 5 | **Transfers in slices.** `operations::Transfer`; `transfer`, `duplicate_with` and `drop_into` loop over it. | M | `tests/transfer.rs`: a tree copied and moved slice by slice with its progress, a move across two file systems, a stop that removes the item being copied, a source that changed while it was copied left where it was, the results the blocking functions give now. |

### What the later tasks take

- **W6.10** (`cupertino-shell-desktop`, after W4.12): `desktop`, `kit::desktop` and `metrics::desktop` leave Files for
  the component, which names `kit::files` for its listing, item icons, names, rename, operations, open, notices and
  `places::desktop_folder`. It runs its file work as tasks, a `Transfer` slice by slice, the first caller to do so.
  It keeps its own item view: "on Kit's file views" reads "on `kit::files`".
- **W6.11** (`cupertino-shell-dialogs`, after W4.12): the panel step above; the chooser reads its places and its
  folders as tasks.
- **W7.1** (Files through Kit): Files' own threads become tasks, its transfers run slice by slice, `udisks` and the
  session connection go through `kit::bus::connect`, its `notify` watch of the places goes, its `clippy.toml`
  arrives.
- **W7.2**: Settings' chooser sheet is the panel of `kit::files`.
- **W6.4**, **W6.8**: `cupertino-screenshot` and the dock read `cupertino_defaults::user_dirs`.

### What W4.T tests

The `ui` part of W4.T starts once W4.8 is merged. The views' tests are there from W4.8's steps 4 to 6, and W4.12's
steps bring their own. In `cupertino-ui-files` W4.T writes what is left, in files W4.12 does not own: `layout::icon`,
natural order and sort, the types of `mime`, tags read and written, name checks and free names, `name_edit`, `open`
against a fixture of applications, and the operations that are not a transfer (a folder made, an alias, a rename).
Files' own modules are W7.T's.

### Decisions

The owner approved these with the design (the plan's decision log, 2026-11-18).

1. **The views' boundary.** The three views are widgets generic over the program's message. They report through
   `ViewEvent` and one function, which may answer with no message. The list and the columns come assembled; the
   chain's listings and layout, the preview pane and the name field stay with the program.
2. **A crate of its own.** The family is `cupertino-ui-files`, in `ui`, not a module of `cupertino-ui-controls`.
3. **What stays in Files.** Everything one program uses, `udisks` included.
4. **No threads.** The family starts none and touches no file on the UI thread, watch registration included. A model
   runs one work at a time. A folder that does not open, one without read permission among them, leaves the window
   where it is with the notice it gets now.
5. **Mounts that do not answer.** Files' I/O takes turns, four at once for all its windows, so four folders hung at
   once stop its listings, thumbnails and copies until one comes back; the windows still answer, and closing one does
   not block. Files never opens the same hung folder twice.
6. **Transfers in slices.** A copy or a move gives its I/O turn back every tenth of a second instead of holding it to
   the end.
7. **The panel's look.** Unchanged in wave 4. In W6.11 it becomes a view of `kit::files` on a `FileList`, on a review
   sheet, and Settings' sheet moves onto it in W7.2.
8. **Places.** One reader of the user directories, in `cupertino-defaults`. It passes over a relative
   `XDG_CONFIG_HOME`, as the specification says. It reports an unreadable file.
9. **Two tasks.** W4.8 changes no behaviour (six steps). W4.12 holds every behaviour change (five steps), after W4.8
   and before W6.10.
