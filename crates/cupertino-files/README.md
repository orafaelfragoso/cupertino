# cupertino-files

File-manager core with no window-system or toolkit dependency, plus the file manager itself: the `cupertino-files`
binary (feature `app`) runs it as a CupertinoKit application (`kit`). The library implements the data, interaction
logic and layout metrics a native file manager (and a desktop-icon layer) needs.

## Usage

```sh
cargo test -p cupertino-files --features app                    # 170+ unit tests; the Kit window run headless: 19 window, 14 context menu, 5 search, 6 drag and 9 drop tests; 15 removable media tests (a fake udisks2 on a private bus), 2 workflow, 3 session handler and 2 error notice tests
cargo run -p cupertino-files --features app -- [--terminal PROGRAM] [FOLDER | file:///FOLDER | trash:///]  # the window (default $HOME), in a running session
cargo clippy -p cupertino-files --all-targets
ci/capture.sh files/window --view list   # a window or its sidebar on a sample home folder (crates/cupertino-capture)
ci/capture.sh files/sidebar --devices removable --scale 2   # the sidebar with a mounted, a busy and an unmounted device
cargo bench -p cupertino-files --bench directory                # 50k entries
```

Library example:

```rust
use cupertino_files::{DirectoryModel, ModelOptions, ModelDelta};
use cupertino_files::layout::icon::{self, FontMetrics, IconParams, SectionSpec};

let mut model = DirectoryModel::open("/home/user/Pictures", ModelOptions::default())?;
loop {
    for delta in model.poll_timeout(std::time::Duration::from_millis(16)) {
        if let ModelDelta::LoadComplete { count } = delta { println!("{count} items"); }
    }
    let p = IconParams::new(64, 54, 12, true, false, FontMetrics::estimate(12.0));
    let layout = icon::layout(&p, 1200.0, &[SectionSpec { title: None, count: model.len() }], icon::Flow::Leading);
    // draw layout.cells[i].{frame, icon_frame, label_frame}; model.entries()[i] is the item
    # break;
}
# Ok::<(), std::io::Error>(())
```

### The window

`main` reads the session (XDG directories, the sidebar's places, the MIME database, the recents file, the session bus
for notifications, the system bus for removable media) and runs `kit::run`. Every window is a window of that one
process: File › New Window and Open in New Window open another beside it.

The window is a split view: the sidebar of places, then a navigation stack whose toolbar holds back / forward, the
folder's name (Section text, cut with an ellipsis), the View, Tags and Action pop-up buttons (`kit::toolbar`; each opens
its menu below it: the view modes, the tags of the selection, the selection's context menu or the folder's) and the
search field, over the folder in the icon view (`kit::grid`, laid out by `layout::icon`; a name wraps to the lines its cell
holds and the last ends in an ellipsis) or the list view (`kit::list`: the toolkit's outline table, Name, Date Modified,
Size, Kind, each cut with an ellipsis in its cell; a type the kind table does not list is described from its MIME
subtype (`mime::kind_description`), in capitals only when it is an abbreviation of up to four letters and digits, else
in words with the first capitalised and, in a subtype of several words, each of up to three kept as an abbreviation
("MS WMV movie"); a header click sorts, again reverses; a folder's disclosure triangle, Left and Right open and close
it in place). The toolbar is the window's drag region (Kit's
`NavigationStack`, under the server's title bar): a drag where no control lies moves the window and a double click there
zooms it, as the old window's unified bar did; back / forward, the pop-up buttons and the search field keep their
presses, and from the keyboard the compositor's Zoom shortcut (Super+Ctrl+Z by default) and the menu bar's Window menu
zoom, minimize and tile it. A window opens in the icon view. Under the folder the path bar and the status bar can be
shown; both are hidden by default. The path bar names the folder chain from the root (or from Trash / Recents), each
folder with its icon, and a click on one, or Space or Return on it, goes there (each folder is a Tab stop, and the bar
reads as the group "Path"); the status bar reads "N items, X available" (`statvfs` of the
folder, read again whenever the listing changes and after a transfer). Changes on disk appear live. The sidebar shows
the theme's sidebar material where the compositor can draw it, and its opaque fill without desktop effects or under
Reduce Transparency (`kit::sidebar`).

**Pointer and keyboard in the views.** A press on empty space and a drag draw a rubber band (`kit::band`): the items it
touches replace the selection, or toggle against it with Shift or Primary held at the press; near the top or bottom
edge the view scrolls under the band. A click on the name of the only selected item edits the name after the
double-click interval, unless a second click (which opens), a scroll or a key comes first (`kit::clicks`). Ctrl with a
press opens the context menu; Tab and Shift+Tab on an item select the next and previous item by name (with nothing
selected, from the first or last by name, or the one after it when the keyboard is already on it), and past the last or
before the first item the keyboard goes on to the next or previous control. In the list, Tab on a column header goes to
the next header and from the last into the rows; the table hands Files only its rows' keys (`TableView::on_row_key`).

**Item images** (`kit::item_icons`). Each item in the icon view, the list view and a drag image wears the icon of its
type from the asset packs and the host icon theme, through the toolkit's `cupertino_kit::file_icons::FileIcons`: the
icon of the type's kind or of its nearest ancestor's (PDF, image, audio, video, archive, text, program; an office
document packed in a zip is not an archive), else the generic document icon, else the packs' placeholder. The home
folder and the user directories wear their own folder icons (`places::folder_icons`), and a symbolic link the alias
badge in its bottom-left corner. Each icon is drawn at 1× and 2× and keeps its image for the life of the process.
A PNG, JPEG or WebP picture shows its thumbnail once it is made (a symbolic link keeps its type's icon and badge).
`thumbnail_service::ThumbnailService` makes thumbnails on two worker threads, never on the UI thread: after each
message a window says which items are in view or within 200 pt of it (`view::shown`), and the service drops what no
window wants any more before a worker starts it. A worker takes the thumbnail from the freedesktop cache
(`$XDG_CACHE_HOME/thumbnails`, `Thumbnailer::from_env("cupertino-files")`, the rules of `cupertino-thumbnails`: valid
for the file's modification time and size, a failure remembered under `fail/cupertino-files/`, nothing under the cache
itself thumbnailed) or creates it there, then decodes it at 2 pixels a point (128 for the icon view's 64 pt, 32 for
the list's 16 pt). Beyond those in view, at most 32 MiB of decoded thumbnails stay in memory, the least recently drawn
going first; a window showing more keeps them all rather than making them again. A decoder that panics fails its
file, not the worker, and quitting does not wait for a decode that hangs (a stalled network mount). When a picture changes on disk, the watcher's new modification time and size make it wanted
again; the old thumbnail shows until the new one is made, and a file that failed is tried again once it changes.
`Start::with_file_icons` and `Start::with_thumbnails` give a window the session's icons and cache (`main.rs` does);
without them every icon is the built-in placeholder and no thumbnail is made, which is what the tests use unless they
pass a fixture pack and a temporary cache.

The sidebar lists the design's default Favorites, Recents, Applications, Desktop, Documents and Downloads (the
nearby-sharing item has no counterpart), those that exist, and the computer (the file system root, under the pretty
host name of `/etc/machine-info`, else the host name) and Trash under Locations; names are cut at the tail. The places
are live (`kit::places`): a change of the user directories or their configuration shows at once, and Trash comes with
the first item trashed and leaves when the trash's folder goes. Recents is the virtual folder `recent:///`: the existing files of
`$XDG_DATA_HOME/recently-used.xbel`, listed again whenever the file is rewritten, in the window's sort order.
Applications is the first `applications` folder of `$XDG_DATA_DIRS`, a real folder rather than a merged virtual one
(approximate).

**Search.** Typing in the field shows the folder's items whose names contain the text, flat; Escape clears it and
another folder ends it. A search that matches nothing shows "No Results" centred over the view, below the list's header
(Title text: the toolkit has no 17 pt regular role, so the size and weight are approximate); an empty folder without a
search shows nothing. The field (`search_field`) rests at 112 pt; while it has the keyboard or holds text it opens to
2.5 times that, never more than half the toolbar and never over the back and forward buttons or the 60 pt the title
keeps, over 0.2 s with an ease-out, or at once with Reduce Motion. A narrower toolbar narrows an open field at once.

**Context menus.** A secondary press on an item selects it (inside a selection it keeps the selection), on empty space
it clears the selection; the Menu key and Shift+F10 open the menu of the selection, or of the folder without one. The
rows are `menu_spec`'s, walked in order (`kit::context`): rows without a backend are shown disabled, the tag row is a
Tags submenu of the seven colours (checked where every selected item has the tag), the view row is "as Icons", "as
List" and the two views that are not built, disabled. Toolkit menus cannot swap rows while a modifier is held, so an
alternate that does something (Open in New Window, Move Item Here) is a row of its own after the row it belongs to.
The menus track the pointer and keys as every menu does (`cupertino-ui-controls`, `cupertino-menu-model`'s
`tracking`): the secondary press that opens the menu, dragged onto a row, chooses it on release, and a quick click
leaves the menu open; a submenu opens after a rest on its row and stays open while the pointer heads into it (Sort By
from the folder menu, Tags from the item menu), until the pointer rests or crawls; typed letters find a row, Return
chooses it and Escape closes one level, then the menu, with the focus back where it was.

**Copy and paste.** Besides dropping (below), moving or copying into a folder goes through the application's own
pasteboard, the keyboard's and assistive technology's way in: Copy (the item menu, Edit › Copy, Primary+C) keeps the selection's paths; Paste Item
(the folder menu, Edit › Paste Item, Primary+V) copies them into the folder shown and Move Item Here (Alt+Primary+V)
moves them there and empties the pasteboard. A name that is taken gives the arrival a free one (`name copy`), a folder is
never pasted into itself, and what arrived is selected. Both run beside the window (`kit::transfer`): a row under the
folder names what it copies or moves where, with a bar of the bytes done, and its Stop button ends the transfer, removing what was half
written and leaving the rest where it was; closing the window stops its transfers the same way and waits for them. An item moved out of the trash takes its `.trashinfo` record with it; a copy
leaves the record with the trashed item. The pasteboard is not the session's clipboard: other
applications neither see it nor fill it.

**Dragging out.** Both views drag items out of the window (`kit::dragged`, hooked into `kit::grid`'s and `kit::list`'s
press, motion and release and `kit`'s `Event::DragOut` and `Event::DragRows`): a press on an item that moves 5 pt offers the dragged files as a
`text/uri-list` (copy or move; the pressed item first, then the rest of the selection in the view's order) and the image
follows the pointer, which holds it where the press grabbed the cell. One item shows its icon (or thumbnail) and its
selected name as the grid draws them; several show the pressed item in front of the icons of up to two more, each 4 pt
up and to the trailing side, with the count in a badge on the farthest icon's corner. A row of the list shows its name
cell as the list draws it (icon and name, mirrored right to left), with the count in a badge after the name for several
rows. The image is drawn at 0.8 opacity
(`dragged::OPACITY`), at the window's scale rounded up. A press on an item of a selection keeps the selection, and its
release without a drag selects that item alone. The window does nothing when the drag ends (the receiving application
copies or moves; a move is not undone here), a cancelled or refused drag takes the image away at once with no return to
the item, and the selection stays.

**Dropping in.** The window takes a `text/uri-list` of local files dragged from any application, this one included
(`kit::drops`, two `DropTarget`s): on a folder of the list or the icon grid, on a place of the sidebar, and anywhere
else in the folder area on the folder shown (over a file too). The Trash place, and the folder area of the trash's own
window, move the items to the trash, and only for a source that lets them go; Recents and the toolbar take nothing. The
place under the drag is highlighted with the accent at 0.3 opacity in rounded corners, with a 2 pt text-colour outline
under Increase Contrast; a background drop highlights the whole folder area. With nothing held the items move within a
device and are copied across devices, Option copies and the primary modifier moves (`operations::drop_action`), and
the drop goes through `operations::drop_into`, as Paste Item does: a taken name gives the arrival a free one, a folder
never goes into itself, the failures are one notice, and what arrives in the folder shown is selected. For the window's
own drag the device is known while the drag moves, so the pointer shows the action; another application's files are
known only at the drop, so with nothing held the drag shows a move and the drop copies what lies on another device. The
source hears the drop is finished only after the files are in place, and with what was done: a move only when every
item left its place, a copy when every item was copied, and otherwise nothing (an item that failed, or one dropped on
the folder already holding it), so a source never removes items Files did not take. A list is taken whole or not at
all: one with a URI of another scheme or host, a relative path, a `.` or `..` step or a NUL is refused. A drag of the window's own items has nowhere to go
on one of the dragged folders or in the folder holding them all. Folders do not spring open under a drag.

**Failures.** A failure of something a person did (open, a new folder, duplicate, make alias, show original, put back,
tags, rename, a name that can't be used, copy, paste, move to the trash) is one transient notification from "Files" on
the session bus (`org.freedesktop.Notifications`), posted in order from a thread of its own (`notice`). It names one
item, or counts several, and its body is the first error, with control and bidirectional formatting characters removed,
cut at 256 characters and escaped, because the body is markup. Without a session bus, or when a post fails, that notice
is written to stderr instead, with the reason. Failures that only explain the window (an entry that can't be read, the
free space, the default applications) stay on stderr. The wording is approximate.

**Accessibility** (`tests/kit-accessibility.rs`). Every control reads its role, name and state and does its default
action when assistive technology asks: the places and their eject buttons, back and forward, the View, Tags and Action
pop-up buttons (each says whether its menu is open), the search field, the path bar's folders, the icon view's cells, the list's rows (selected, open or
closed, with their level and position; Click selects a row alone, Expand and Collapse open and close a folder), the
column headers (sorted up or down), the transfer row and its Stop button, the name editor and the context menu. Icons
are decorative, an item reads by its name and never by its path, and the rubber band is not read. Tab goes from the
sidebar to the toolbar, the list's column headers in the order shown, the folder's items, the path bar's folders and the
sidebar's separator, then around, and Shift+Tab visits the same stops in the opposite order. The
keyboard comes back to the button or item that opened a menu, and to the item a rename commits or cancels; a folder
closed over the row the keyboard is on leaves it on the folder. A polite live region (`kit::announce`) reads a
transfer starting and ending ("Copied “a.txt” to “Documents”", "Stopped copying …"), a search's result count and the
selection's count once it holds more than one item, without taking the keyboard; a single item selected is read by the
focus that comes to it and a failure by its notification. Each message is a node of its own, so the same words said
again (a second copy of the same file) are heard again; a rubber band says its count once it is let go, and a
transfer's progress is not announced. A selected row's text, its secondary columns included, is
drawn in the selected control text colour on the accent. The focus ring shows on the toolbar buttons, the path bar's
folders and the rows under Increase Contrast. Over AT-SPI the list is a multiselectable table whose rows carry their
selected state. The tests listen through `cupertino-ui-test`'s stand-in screen reader (`AssistiveTechnology`), and every
settle of the test window runs the shared accessibility audit over each window open, with nothing left out.

**Removable media** come from udisks2 on the system bus (`udisks`): after Trash, Locations lists every file system of a
drive that udisks neither hides (`HintIgnore`) nor counts as the system's (`HintSystem`), with usage `filesystem`, and
every locked encrypted container. A block without a drive, such as a loop device, is left out. A row is named by
`HintName`, then `IdLabel`, then the drive's vendor and model, then "<size> Volume". A row wears the symbol of its
drive's medium (`places::Medium`, from udisks' `Media` and `ConnectionBus`): `symbol.memory-card` for a flash card
(a `flash_*` medium or an SD bus), `symbol.flash-drive` for a USB stick (`thumb`, or `flash` on USB), else
`symbol.externaldrive`; none wears the computer's. It lists again whenever udisks reports a change or comes to the
bus. Every call runs off the window's thread.
* **Mount:** a click on a row that is not mounted mounts it and takes the window that asked to the mount point.
  Each partition of a drive is a row of its own.
* **Unlock:** a click on a locked container opens a sheet that asks for its passphrase in a secure field; Unlock (or
  Return) unlocks and mounts what it holds, Cancel closes the sheet without a call. The passphrase is kept in zeroed
  memory, is never in the accessibility tree, and every copy of it is cut from a service's error before the error is
  shown or written.
* **Eject:** a mounted row has an eject button (`symbol.eject`), and File › Eject (Primary+E) acts on the file system of
  the sidebar row the keyboard is on, else on the one the folder shown is on, never on a drive that also holds the
  home folder. Ejecting first takes the window home, then unmounts every mounted file system of the drive in
  order, then ejects the drive, else powers it off. It stops at the first unmount that fails.
* **Read-only media** are described so and take nothing new: New Folder, Paste Item, Rename and the other changes are
  disabled there, and no drop lands there. A file system mounted read-only on a writable medium (the mount's
  `ST_RDONLY`) is taken the same way.
* **Busy and failures:** while a call runs the row is busy and takes no other call; it turns an activity indicator
  where its eject button was. A failure is one transient
  notification; when a file system is busy it names the programs that keep it open (read from `/proc`). A polkit prompt
  the person dismissed is not reported.
* **Permission:** no call sets `auth.no_user_interaction`, so polkit may ask through the session's authentication agent,
  and Files never asks for a password of its own.
* **A device pulled out** leaves the sidebar, and a window on it goes home.

`data/applications/cupertino-files.desktop` names the application "Files" for the menu bar, which reads `Name=` of
`<app_id>.desktop` from the data directories (without it the bar shows the capitalised app id); install it to
`$XDG_DATA_DIRS/applications` or put `data` on `XDG_DATA_DIRS` in a nested session. Its `Exec` passes `%U`: a path, a local
`file://` URI or `trash:///`, which opens the home trash's `files` folder (created with `info` when nothing was trashed yet;
other `trash:` paths are refused) under the title "Trash". `data/applications/cupertino-mimeapps.list` makes the entry the
default of `inode/directory` and `x-scheme-handler/trash` for the desktop `Cupertino` only (the server sets
`XDG_CURRENT_DESKTOP=Cupertino` for its clients): in that session it comes before the host's `mimeapps.list` of the same or a
later data directory, other desktops are not affected, and a default the user set in `~/.config/mimeapps.list` still wins
(`tests/session-handlers.rs`). The entry's `Icon=application.file-manager` names the asset directory's file manager icon,
which the dock tile, the switcher and the launcher draw (`cupertino-icons`).

| Input | Action |
|---|---|
| double click, Return, Primary+Down, Primary+O | open: a folder in this window, files with their default application (`mimeapps.list`, read at each open; `Terminal=true` applications under `--terminal PROGRAM` as `PROGRAM -e`), each launch with an activation token naming the application |
| Primary+[ / Primary+] or the toolbar arrows | back / forward |
| Primary+Up | enclosing folder, with the folder just left selected |
| Primary+1 / Primary+2 | icon / list view |
| Alt+Primary+P / Primary+/ | show or hide the path bar / the status bar |
| arrows, Home / End, Page Up / Page Down, typing; Shift / Primary click; Primary+A; a list column header | selection, type-select, select all, sort (again: reverse) |
| press on empty space and drag (Shift / Primary: toggle) | rubber band (above) |
| Tab / Shift+Tab | on an item: the next / previous item by name; past the last / first, the next / previous control (a list column header: the next / previous header or control) |
| a folder's triangle, Right / Left in the list | open / close the folder in place |
| click the name of the only selected item and wait | rename (above) |
| Primary+Backspace | move the selection to the trash (`$XDG_DATA_HOME/Trash`, freedesktop Trash 1.0); Put Back in the trash |
| Shift+Primary+N, Ctrl+Primary+N, Primary+D, Ctrl+Primary+A, Primary+R | new folder (its name is edited at once), new folder with the selection, duplicate, make alias, show original |
| Rename in a menu | edit the name in place: the name without its extension is selected; Return commits, Escape cancels; a name that exists is refused (never replaced) and an invalid one keeps the editor open |
| Primary+C, Primary+V, Alt+Primary+V | copy, paste item, move item here (above) |
| right press, Ctrl with a press, Menu, Shift+F10 | context menu (above) |
| the toolbar's View, Tags and Action buttons | the view modes, the tags, the context menu, below the button |
| click a folder of the path bar, or Space / Return on it | go there |
| Escape | end a rename without renaming, else clear the search |
| click a removable row / its eject button, File › Eject, Primary+E | mount or unlock it and go there / eject its drive (above) |
| drag the separator right of the sidebar | resize the sidebar |
| press an item of either view and move 5 pt | drag the items out of the window (above) |

## Design

### Modules and API

| Module | Role | Main items |
|---|---|---|
| `model` | async directory model | `DirectoryModel::open(dir, ModelOptions)`, `poll` / `poll_timeout` → `Vec<ModelDelta>` (`Reloaded`, `LoadComplete`, `Inserted`, `Removed`, `Changed`, `Moved`, `DirectoryGone`), `enumerate_blocking`, `set_sort` |
| `natural`, `sort`, `group` | name collation (case-, width-insensitive, numeric, localized), ordering, the date buckets dates read in | `NameKey`, `compare_names`, `SortSpec { key, ascending, folders_first }`, `sort_entries`, `insertion_index`, `date_bucket` |
| `settings` | per-folder view state | `ViewSettings` (icon / list / column / gallery), `ViewStyle`, `default_columns`, `SettingsStore` (atomic JSON, keyed by device + inode), `FolderKey` |
| `selection` | selection model | `Selection::{mouse_down, mouse_up, click_empty, move_focus, on_insert, on_remove, on_move}`, `Modifiers` |
| `rename`, `typeselect` | interaction logic | `RenameTrigger`, `validate_name`, `needs_extension_warning`, `initial_selection`, `unique_name`; `TypeSelect::{type_text, tab}` |
| `context_menu`, `menu_spec` | context menus | `menu_spec::{item_menu, background_menu, condense}` (the rows in order), `MenuItem`, `Command`, `common_apps` (the Open With list of a selection) |
| `menu_bar`, `search_field` | the menu bar and the search field | `menu_bar(MenuState)`, `MenuAction`, `chords`, `enabled`; `SearchWidth::{want, tick, follow_toolbar, width}`, `open_width` |
| `name_edit`, `operations` | rename in place and file changes | `NameEdit::{field, field_mut, commit}`; `rename_item` (no replace), `new_folder`, `drop_action`, `drop_into` (move or copy, `name copy` on a clash), `transfer` (the same for a list of items, with `Progress` and a `Cancel`) |
| `trash` | freedesktop Trash 1.0 | `Trash::{trash, list, restore, delete, empty, purge_older_than}`; home and mount-point trash, `.trashinfo`, `directorysizes`; `trashed_item`, `forget` (an item moved out of a trash) |
| `thumbnail` | freedesktop thumbnail cache | `Thumbnailer::{lookup, lookup_at_least, create, get_or_create}`, `ThumbSize::for_icon`, `file_uri`, failure markers |
| `tags`, `mime` | metadata | `read_tags` / `write_tags` (`user.xdg.tags`), `TagColor` (seven label colours), `parse_reference_tags`; `detect`, `kind_group`, `kind_description` |
| `open` | opening files | `open_command` (default application of `mimeapps.list` through `cupertino-apps`; `Terminal=true` under `--terminal`), `command_with`, `open_with` (Open With list: default first, rest by name), `OpenError` |
| `layout::icon` | icon view | `ideal_cell_size`, `max_title_lines`, `layout` (flow / justified, 32 pt section headers), `IconLayout::{hit, hits_in_rect, neighbour}`, `FreeGrid` |
| `layout::list` | list view | `row_height`, `layout` (columns, rows with level and expanded state, disclosure, indentation), `ListLayout::{row_at, visible_rows}` |
| `history`, `places`, `recents` | navigation state | `History::{visit, back, forward}`; `standard_places` (Recents, Applications, Desktop, Documents, Downloads, the computer, Trash), `folder_icons`, `session_places` (`SessionPlaces`), `computer_name`, `applications_folder`; `recents::{parse_xbel, recent_entries}` (the bookmark file is read with `cupertino_apps::recent`, which `parse_xbel` and `XBEL_NAME` re-export) |
| `browser` | one window's folder | `Browser::{open, go, back, forward, parent, poll, sort_by, press, release, move_focus, select_all, type_text, activation, outline, expand, collapse, collapse_all, available_space}`: model, history, the list's outline of expanded folders, selection by path across reloads, nearest-ancestor fallback for a deleted folder |
| `window`, `format` | the window's wording and metrics | `metrics`, `ViewMode`, `path_chain`, `status_text`; `human_size`, `available_size`, `relative_date` |
| `udisks` (feature `app`) | removable media | `devices` (the listed file systems and locked containers of udisks' managed objects), `Volumes::{watch, poll, mount, unlock, eject}` → `Report`, `Media` (the listed devices and the busy ones), `Passphrase`, `holders`, `notice` (a failure's notification text) |
| `notice` (feature `app`) | failures as notifications | `failure`, `unopened`, `bad_name` → `Notice`; `Notices::{start, post}`; `shown`, `escape` |
| `kit` (feature `app`) | the window on CupertinoKit; driven headless through `cupertino-ui-test` | `prepare(Start::open(folder, places, types, recents, notices, applications, terminal))`, with `with_file_icons`, `with_thumbnails` and `with_places_source`, then `run` or a `TestDriver<Files>`; `grid` (the icon view), `list` (the list view's presses and band), `band` (the rubber band), `clicks` (double-click and click-to-rename), `toolbar` (pop-up buttons, path bar folders), `sidebar` (material, keyboard watch), `places` (live places), `transfer` (paste and move off the window's thread), `context` (context menus), `search` (the field's slot and "No Results"), `dragged` (drag images), `drops` (drops onto folders, places and the folder shown), `item_icons` (items' icons and thumbnails) |
| `thumbnail_service` | thumbnails of the items in view | `ThumbnailService::{new, want, forget, get, queued, memory}`, `Stamp`, `has_thumbnail`, `MEMORY_BUDGET`, `WORKERS` |
| `main` (feature `app`) | the binary | arguments, the session's places, types, associations and buses, then `kit::run` |

### Measured defaults

Values live in `metrics.rs`.

| Area | Value |
|---|---|
| Icon view | 64 pt icons (16–512), grid spacing 54 (1–100), 12 pt text (10–16), label below; cell = icon + 12.5 % padding, label width interpolated by the spacing, 1–2 lines |
| List view | 16 pt icons (16–32), 13 pt text, row = max(icon, font line) + 4 with no intercell spacing, a 20 pt pitch, 15 pt indentation; kinds are the design's type descriptions, capitalised |
| Column view | 245 pt columns (min 100), 312 pt preview column |
| Chrome | sidebar 200 pt default, 128 pt minimum, rows 24/28/32 pt; unified toolbar 52 pt; status bar 20 pt, path bar 28 pt; search field 112 pt at rest |
| Timing | sidebar toggle 0.3 s, slow motion ×10; type-select reset after max(2 s, 2 × key-repeat delay); search field 0.2 s |
| Selection look | icon bubble: the icon enlarged by its longest side / 16, that radius, quaternary label fill, white 50 % border of min(2, trunc(padding)), the same in an inactive window; the label pill is the accent (grey when inactive) |
| Names | compare case-insensitively, numerically, width-insensitively, diacritics significant |

### Policies

* The context menus are `menu_spec`'s rows walked in order: New Folder with Selection, Open (Alt: Open in New Window),
  Open With (Alt: Always Open With), Show Original (links), Put Back (in the trash), Move to Trash (Alt: Delete
  Immediately…), Get Info, Rename, Compress, Duplicate, Make Alias, Quick Preview, Copy, Share, the tag row and Quick
  Actions; and for empty space New Folder, Paste Item (Alt: Move Item Here), Get Info, the view row, Use Groups, Group By,
  Sort By, Clean Up By and Show View Options. Separators are condensed. Rows for contexts this window lacks (volumes,
  desktop, cloud, discs, packages, servers, sidebar) are left out. Enabled: Open, Open in New Window, Open With, Show
  Original, Put Back, Move to Trash, Rename (one item), Duplicate (`name copy`), Make Alias (a symbolic link
  `name alias`), Copy, Paste Item, Move Item Here, New Folder, New Folder with Selection, the tags, Sort By (the choice in
  use is checked) and the two usable view modes. Disabled: Delete Immediately…, Get Info, Show Inspector, Compress, Quick
  Preview, Slideshow, Copy as Pathname, Paste Item Exactly, Share, Quick Actions, Empty Trash, Other…, Duplicate Exactly,
  Use Groups, the Group By choices, Snap to Grid, Date Added, Clean Up By, Show View Options and the Columns and Gallery
  views.
* A rename that changes the extension is not confirmed (the design asks; there is no dialog here). The name editor and
  the new folder's default name numbering are approximate.
* The collation fold table, date bucket boundaries, thumbnail-size choice and disclosure width are approximate and
  labelled so in the source.
* Only `SettingsStore`, `Trash`, `Thumbnailer`, `operations` and the tag functions write files, into caller-supplied
  locations (tests use temporary directories).

### Frame time

`tests/kit-damage.rs` pins what each interaction repaints, from the damage of each presented frame: an arrow key
repaints the row the selection left and the row it reached, the caret's blink repaints only the search field, the
search field's animation repaints only the toolbar, a hover repaints no row, a wheel step repaints only the listing,
and a window kept up to date from damage alone has, after each of these, the pixels of the window drawn whole (at
scale 1.5), give or take the few levels by which the renderer's partial redraw blends an edge.

`tests/kit-repaint.rs` holds `repaint_budget_report`, `#[ignore]`d because it measures wall time. In a generated folder of
1500 files and with the window filling a 3840×2160 display at scales 2, 1.5 and 1, it times every frame of opening the
folder, arrow-key selection among the rows in view and past them, hover, wheel scrolling, the search field opening
and closing, the caret's blink, typing a search and typing a new name, and gives p50, p95, p99, maximum, the frames
over 8 ms and the damaged share of the window. It uses the application's renderer and node limit and keeps no copy of
the frames. Run it in the release profile:

```sh
cargo test --release -p cupertino-files --features app --test kit-repaint -- --ignored --nocapture --exact repaint_budget_report
```

`CUPERTINO_REPAINT_DIR=DIR` also writes `files-<scale>x.md` and `files-<scale>x.csv` (one line per frame) there, and
`CUPERTINO_REPAINT_SCALES=2,1` measures other scales. Under `ci/worktree-build.py`, which keys its target on the
`CUPERTINO_*` variables, build with `--no-run` and run the test binary it names under `python3 ci/worktree-build.py
locked -- env CUPERTINO_REPAINT_DIR=DIR BINARY --ignored --nocapture --exact repaint_budget_report`.
[What runs on CupertinoKit](../../docs/src/apps-on-kit.md#frame-time) has the last results.

## Open issues

* Scrolling the listing, with the wheel or with an arrow key past the last row in view, repaints the whole listing on
  the CPU: 60–70 ms a frame at 3840×2160, over the 8 ms budget. Opening a folder and the key of a search whose results
  fill the listing take as long. At scale 1 the search field's animation and typing a new name take 8–10 ms. See Frame
  time.
* A folder does not spring open under a drag, and a drop from another application shows a move while it is held even
  when it will copy across devices. The drag badge holds three digits.
* Thumbnails are made for PNG, JPEG and WebP pictures only (what `cupertino-thumbnails` decodes): no external
  thumbnailers, so videos, PDFs and other documents show their type's icon. Thumbnails are decoded for a 2× display;
  a 1× display draws them shrunk. The sidebar keeps the design's template symbols, as the places are not files.
* The toolbar has no Group By popup (the window lists without sections), no share, preview or path controls and no
  sidebar toggle. The search filters the folder shown by name (the design searches the computer by default); its field
  has no clear button.
* The path bar does not shorten the names of the folders above the one shown when it runs out of room.
* The free space is read again when the listing changes and after a transfer, not when another program writes
  elsewhere on the file system.
* The folder icons of the user directories are read when the application starts; a user directory moved later keeps
  the plain folder icon until Files starts again.
* The pasteboard is the application's own, not the session's clipboard.
* Per-folder view settings, the sidebar width and the bars' visibility are not saved.
* Recents is listed in the window's sort order, not by last use, and opening a file does not add it to the list.
  Applications shows the desktop entry files of one folder; opening one does not launch the application.
* The lock badge is not drawn, because entries carry no permission bits.
* Over AT-SPI the list's table offers no Selection interface (the accessibility bridge gives one only to lists and
  trees of items), so a screen reader reads the selection row by row.
* When the node the keyboard is on goes away (the row of the folder it opens, the list swapped for the icon grid, an
  item moved to the trash), the keyboard falls to its nearest surviving ancestor (the table's rows, the items' frame),
  and assistive technology is told the focus is on the nearest node of that path it can see, such as the table, until
  Tab or a click moves it on.
* Removable media: disc images and network mounts are not listed.
* Opening several files opens each without confirmation; a mixed selection enters the first folder only (approximate).
  Open in New Window opens as many windows as there are selected folders.
* The window does not hear the window states the compositor sets: after the compositor takes a window out of full
  screen, or refuses it, the menu still offers Exit Full Screen, and the next choice asks for the state the window is
  already in.
* The icon view (`kit::grid`) scrolls back to the top when the view changes, so the selection can be out of sight, and
  each rebuild lays out every item of the folder.
