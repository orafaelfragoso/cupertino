# cupertino-files

File-manager core with no window-system or toolkit dependency, plus the file manager itself: the `cupertino-files`
binary (feature `app`) runs it as a CupertinoKit application (`kit`). The library implements the data, interaction
logic and layout metrics a native file manager (and a desktop-icon layer) needs.

## Usage

```sh
cargo test -p cupertino-files --features app                    # 210+ unit tests; the Kit window run headless: 38 window, 16 accessibility, 16 context menu, 7 damage, 5 search, 5 item icon, 7 drag, 9 drop, 12 spring-loaded folder, 9 file operation and 4 type-to-select and band tests; 19 removable media tests (a fake udisks2 on a private bus), 11 single-instance tests (a private bus), 2 workflow, 3 session handler and 2 error notice tests
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
process: File › New Window and Open in New Window open another beside it, and a later launch opens its window there
too (below).

**One process per session** (`instance`). Files keeps running while a file operation runs with no window open, so a
second process would not see its operations, its progress window or its pasteboard. At startup Files claims the session
bus name `org.cupertino.Files` (without queueing) and serves `org.freedesktop.Application` at `/org/cupertino/Files`.
A launch that finds the name taken (the dock, the launcher, a terminal, `xdg-open` of a folder) hands its request to
the running Files and exits with success: a folder argument becomes `Open` (a relative path made absolute where it was
launched, sent as a `file://` URI), no argument becomes `Activate`, and the launch's `XDG_ACTIVATION_TOKEN` (else
`DESKTOP_STARTUP_ID`) goes along as `activation-token` and `desktop-startup-id`. Files takes the token out of its
environment at startup, so no program it starts inherits it, and raises its own first window with it.

* `Activate` brings the most recently opened browser window forward, or opens one on the home folder when none is
  open. `Open` opens a window on each `file://` (local) or `trash:///` URI; one URI of any other kind (another host, a
  path without a scheme, a file in the trash) refuses the whole call with `InvalidArgs`. `ActivateAction` takes
  `show-progress` (the progress window, when it lists something, as Window › Show Progress) and `new-window` (a window
  on the home folder), with no parameter.
* The window a call opens or brings forward is raised with the call's token once it has been shown
  (`WindowRequest::Activate`); with several folders, the last window opened is raised. A forwarded `--terminal` is
  ignored: the running Files keeps its own.
* Only an unsandboxed process of the same user may call: the caller is named by the bus daemon's credentials (its
  `ProcessFD`, else its `ProcessID`) and refused when it runs in a Flatpak (a `/.flatpak-info` in its root), in a Snap
  (an AppArmor label starting `snap.`), as another user, or when its root cannot be checked. A launch with a token
  hands it only to an owner that runs the same program; another owner (an install at another path, or a program that
  took the name) gets the request without the token, and the launch says so on stderr.
* The window's thread never waits on the bus: a call reaches it through a channel (at most 16 waiting; more are refused
  with `LimitsExceeded`) and the bus thread waits for its answer. The service's connection reads no signals (the name
  is claimed with a plain `RequestName` call), so the credentials call made for each incoming call cannot stall behind a
  full signal queue.
* A launch waits 3 s for an answer (`FORWARD_TIMEOUT`, as long as the dock waits for one). A running Files that does not
  answer in time and still owns the name fails the launch with a message, so a second Files never starts beside a hung
  one; one that left lets the launch claim the name and start. As it exits, Files refuses waiting and later calls
  (`org.freedesktop.DBus.Error.Disconnected`) and gives the name up before it waits for its notices, so a launch racing
  the exit claims the name and starts (it keeps trying for 5 s). Without a session bus Files runs alone and says so.
* Decision: the desktop file stays `cupertino-files.desktop` and does not set `DBusActivatable`, so pinned dock items
  and the default file manager keep their ID; a launch reaches the running Files through the hand-over above. The
  Desktop Entry specification wants the desktop file ID to equal the bus name for D-Bus activation, and a bus name needs
  two elements, so `cupertino-files` cannot be one. To adopt it later: rename the desktop file to
  `org.cupertino.Files.desktop` with `DBusActivatable=true`, ship `org.cupertino.Files.service` (D-Bus session
  services, `Exec` the program) so the bus can start Files, set the windows' application id (`kit::APP_ID`) to the same
  ID, and migrate the dock's pinned items, `mimeapps.list` defaults and the session's default file manager from the old
  ID. The dock then calls `Activate` itself, with its token, instead of starting the program.

The window is in the full-height-sidebar style (Kit's `SidebarWindow`, `DecorationMode::Sidebar`): the sidebar of places
from the window's top edge to its bottom edge, the window control buttons in its top, then a navigation stack whose
floating toolbar holds the back / forward group, the folder's name (Kit's `ToolbarTitle`, cut with an ellipsis; in the column view the folder selected in the last column, as the window's title is), the
view-mode choice (icons, list and columns, and gallery disabled), the View menu button, the group of share (disabled),
Tags (its own drawn tag, as a host symbol theme may have none) and Action (each opens below its button: the view modes, the tags popover of the selection (below), else a menu of
the tags unavailable, the selection's context menu or the folder's) and the search field, which rests as a round button, over the folder in the icon view (`kit::grid`, laid out by `layout::icon` from a 10 pt leading and 8 pt
top inset; a name is centred and wraps to two lines, a longer one keeping its first line and cut in the middle of the
rest with an ellipsis, so its end and extension stay) or the list view (`kit::list`: the toolkit's outline table, Name, Date Modified,
Size, Kind, each cut with an ellipsis in its cell; a type the kind table does not list is described from its MIME
subtype (`mime::kind_description`), in capitals only when it is an abbreviation of up to four letters and digits, else
in words with the first capitalised and, in a subtype of several words, each of up to three kept as an abbreviation
("MS WMV movie"); a header click sorts, again reverses; a folder's disclosure triangle, Left and Right open and close
it in place) or the column view (below). The toolbar and the top of the sidebar are the window's drag region (Kit's `NavigationStack` and
`SidebarWindow`): a drag where no control lies moves the window and a double click there zooms it; back / forward, the
toolbar's buttons and the search field keep their
presses, and from the keyboard the compositor's Zoom shortcut (Super+Ctrl+Z by default) and the menu bar's Window menu
zoom, minimize and tile it. Under the folder the path bar and the status bar can be shown from the View menu; both
are hidden by default. Each window keeps its view mode and bars while it is open, and the last choice made in any
window is kept in `$XDG_CONFIG_HOME/cupertino/files/window.json` (`window_prefs`): a window opened later, in this launch
or the next, starts with it (the icon view without the bars at first). The path bar names the folder chain from the root (or from Trash / Recents), each
folder with its icon, and a click on one, or Space or Return on it, goes there (each folder is a Tab stop, and the bar
reads as the group "Path"); the status bar reads "N items, X available" (`statvfs` of the
folder, read again whenever the listing changes and after a transfer). Changes on disk appear live. The sidebar shows
the theme's sidebar material where the compositor can draw it, and its opaque fill without desktop effects or under
Reduce Transparency (Kit's sidebar panel glass).

**Column view** (`columns`, `kit::columns`). The folders from the one the window opened on down to the selection stand
side by side in a chain that scrolls sideways, each a list of 22 pt rows with the item's 16 pt icon, its name and a
chevron on folders; a folder's row stays highlighted in grey (in the accent while its column has the keyboard) and the
column after it lists it. The chain scrolls to keep its last column whole in view. Up and Down move in the column shown,
Right goes into the selected folder (its first item selected) and Left back to the column before; typed letters select
in the column shown, a click on a row of another column shows that column's folder, and Return, double click, drops,
the context menus and rename work as in the other views. A selected item that is not a folder is previewed in a 240 pt
column at the end (`cupertino-ui-controls`' `FilePreview`): a picture whole, the start of a text file, else its large
icon, then its name, kind and size, its dates, and a More… button (a ringed ellipsis over its label) that opens the
item's menu. The divider after a
column drags it wider or narrower (100–1000 pt; with Option held, every column), and the width it leaves is kept with
the window's view state once it is let go (`window_prefs`, `column_width`). A row's selection sits 10 pt in from its
column's sides, as the grey highlight of a folder of the chain does, and a click on another column moves the keyboard
there without a focus ring. Each column is a list named for its folder, and the preview a
group named "Preview of <name>".

**Preview window** (`preview`, `kit::peek`). Space on the items, or File › Quick Preview (Primary+Y), opens the
selection in a floating window of its own: a toolbar with a close and a full screen button, the item's name and, with
several items, "n of N", then an "Open with <application>" button (the default application, else "Open"); under it a
picture shows whole, the start of a text file (at most 64 KiB, cut at a character, read on a thread of its own and
dropped when the item shown has changed since; a pipe or a device is never opened) in the pack's fixed-pitch face, and
anything else its large icon with kind, size and dates. With several items selected the arrows go from one to the
next; with one, they move the window's selection and the window follows. Space or Escape closes it. It grows from its
centre as it fades in, over a time that grows with its size (at most 0.35 s), and shrinks back as it closes; with
Reduce Motion it only fades, over 0.2 s. While a name is being
typed to select an item, Space goes on to the name.

**Tabs** (`tab_strip`, `kit::tabs`). Primary+T or File › New Tab opens a tab on the folder shown, after the one shown;
the tab bar shows while there are two or more tabs, or always with View › Show Tab Bar. The tabs share one long rounded
track; the tab shown has a fill and rim of its own, the others only their title, and a tab's close button shows under
the pointer. A click shows a tab, a tab dragged along the bar moves, and the bar's trailing + opens one. Each tab keeps
its own folder, history, selection, view, scroll position and search. Primary+W closes the tab shown (Shift+Primary+W
the window), Window › Show Next Tab and Show Previous Tab (Ctrl+Tab, Shift+Ctrl+Tab) go round them, and the last tab
closed closes the window. The bar reads as a list of tabs, each with its place and whether it is shown.

**Tags** (`tags`, `kit::tag_editor`). With items selected, the toolbar's Tags button opens a popover titled "Tags for
<name>": a field, then the seven colours and every other tag in use, each with its dot, checked where every selected
item has it. A row puts its tag on every selected item or takes it off, and a name typed in the field and Return puts a
tag of that name on them (commas part several, as the attribute keeps them); typing narrows the rows. Tags are kept
in the `user.xdg.tags` extended attribute, a symbolic link's on the item it points to (the system keeps no user
attributes on a link), and every listing reads them on its own thread. The sidebar lists under Tags the colours and names in use, and one chosen shows the folder's items
that carry it.

**Pointer and keyboard in the views.** A press on empty space and a drag draw a rubber band (`kit::band`): the items it
touches replace the selection, or toggle against it with Shift or Primary held at the press; near the top or bottom
edge the view scrolls under the band. In the list a press on a row's blank part (anywhere but its icon and the text of its name) selects the
row as a click, and draws a band once it moves; a press on the name drags the row. A click on the name of the only selected item edits the name after the
double-click interval, unless a second click (which opens), a scroll or a key comes first (`kit::clicks`). Ctrl with a
press opens the context menu; Tab and Shift+Tab on an item select the next and previous item by name (with nothing
selected, from the first or last by name, or the one after it when the keyboard is already on it), and past the last or
before the first item the keyboard goes on to the next or previous control. In the list, Tab on a column header goes to
the next header and from the last into the rows; the table hands Files only its rows' keys (`TableView::on_row_key`).

**Item images** (`kit::item_icons`). Each item in the icon view, the list view and a drag image wears the icon of its
type from the asset packs and the host icon theme, through the toolkit's `cupertino_kit::file_icons::FileIcons`: the
icon of the type's kind or of its nearest ancestor's (PDF, image, audio, video, archive, text, program; an office
document packed in a zip is not an archive), else the generic document icon, else the packs' placeholder. Folders, text files, other documents and archives that
no asset pack draws itself wear Files' own art (`item_art`: a folder, a page with a folded corner, with lines of text for
text files, a zipper for archives), drawn in code at 1× and 2×; a pack's own icon for them wins. The home
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

The sidebar lists the design's default Favorites, headed by the home folder (a house and the user's name), then
Applications, Desktop, Documents and Downloads (the nearby-sharing item has no counterpart), those that exist; the
computer (the file system root, under the pretty host name of `/etc/machine-info`, else the host name), removable media
and Trash under Locations; and under a Tags heading, always there, each tag a listed item carries (above). Names are cut at the tail. The sidebar
takes the toolkit's neutral look for lists of places (`SidebarLook::Neutral`): the selected row in a neutral tint with
its label semibold, the Favorites' symbols in the accent colour and the others' in the secondary label colour; in a
window that is not the one in use every symbol, the selected one too, is the secondary label colour, while an open
context menu keeps the window in use. The places
are live (`kit::places`): a change of the user directories or their configuration shows at once, and Trash comes with
the first item trashed and leaves when the trash's folder goes. Recents (Go › Recents, without keys, as
Shift+Primary+F is the system's full-screen shortcut; Go › Home is Shift+Primary+H) is the virtual folder `recent:///`, not a sidebar row: the existing files of
`$XDG_DATA_HOME/recently-used.xbel`, listed again whenever the file is rewritten, in the window's sort order.
Applications is the first `applications` folder of `$XDG_DATA_DIRS`, a real folder rather than a merged virtual one
(approximate).

**Search.** Typing in the field shows the folder's items whose names contain the text, or that carry a tag of that
whole name (in any case), flat; Escape clears it and
another folder ends it. A search that matches nothing shows "No Results" centred over the view, below the list's header
(Title text: the toolkit has no 17 pt regular role, so the size and weight are approximate); an empty folder without a
search shows nothing. The field (`search_field`) rests as a 36 pt round button that a click opens; while it has the keyboard or holds
text it is open, 280 pt wide, never more than half the toolbar and never over the back and forward buttons, the 60 pt
the title keeps or the controls after the title, over 0.2 s with an ease-out, or at once with Reduce Motion. A narrower toolbar narrows an open field at once.

**Context menus.** A secondary press on an item selects it (inside a selection it keeps the selection), on empty space
it clears the selection; the Menu key and Shift+F10 open the menu of the selection, or of the folder without one. The
rows are `menu_spec`'s, walked in order (`kit::context`): rows without a backend are shown disabled, the tag row is a
Tags submenu of the seven colours (checked where every selected item has the tag), the view modes are the folder menu's
Show As submenu (Icons, List, Columns, and Gallery, which is not built, disabled; the current one checked), the same rows
as the toolbar's view popup (`menu_spec::view_menu`); Quick Preview opens the preview window. Toolkit menus cannot swap
rows while a modifier is held, so an alternate that does something (Open in New Window, Move Item Here) is a row of its
own after the row it belongs to. No row of a context menu or of the toolbar's Action and More popups shows keys: the
keys are the menu bar's (View's ⌘1 to ⌘3 among them), so a menu is as wide as its longest label plus its insets.
The menus track the pointer and keys as every menu does (`cupertino-ui-controls`, `cupertino-menu-model`'s
`tracking`): the secondary press that opens the menu, dragged onto a row, chooses it on release, and a quick click
leaves the menu open; a submenu opens after a rest on its row and stays open while the pointer heads into it (Sort By
or Show As from the folder menu, Tags from the item menu), until the pointer rests or crawls, and Right opens the
focused row's submenu with its first row focused; typed letters find a row, Return
chooses it and Escape closes one level, then the menu, with the focus back where it was.

**Copy and paste.** Besides dropping (below), moving or copying into a folder goes through the application's own
pasteboard, the keyboard's and assistive technology's way in: Copy (the item menu, Edit › Copy, Primary+C) keeps the selection's paths; Paste Item
(the folder menu, Edit › Paste Item, Primary+V) copies them into the folder shown and Move Item Here (Alt+Primary+V)
moves them there and empties the pasteboard. A name that is taken gives the arrival a free one (`name copy`), a folder is
never pasted into itself, and what arrived is selected. Both are file operations of the process (below): a row under
the folder names what it copies or moves where, with a bar of the bytes done, and its Stop button ends the transfer,
removing what was half written and leaving the rest where it was. An item moved out of the trash takes its `.trashinfo`
record with it; a copy leaves the record with the trashed item. The pasteboard is not the session's clipboard: other
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
copies or moves; a move is not undone here) and the selection stays. A drag released where nothing takes it, refused, or
cancelled with Escape does not drop its image: the compositor, which draws every drag's image, flies it back to where
the press was and then removes it, or fades it where it was let go when the window it came from is gone or hidden
(`cupertino-server`, `drag_return.rs`). A cancelled drag also takes the window back from the folders it sprang open
(below), so the item is where the image lands.

**Dropping in.** The window takes a `text/uri-list` of local files dragged from any application, this one included
(`kit::drops`, two `DropTarget`s): on a folder of the list or the icon grid, on a place of the sidebar, and anywhere
else in the folder area on the folder shown (over a file too). The Trash place, and the folder area of the trash's own
window, move the items to the trash, and only for a source that lets them go; Recents and the toolbar take nothing. The
place under the drag is highlighted with the accent at 0.3 opacity in rounded corners, with a 2 pt text-colour outline
under Increase Contrast; a background drop highlights the whole folder area. With nothing held the items move within a
device and are copied across devices, Option copies and the primary modifier moves (`operations::drop_action`), and
the drop is a file operation, as Paste Item is: a taken name gives the arrival a free one, a folder never goes into
itself, a failure is listed in the progress window, and what arrives in the folder shown is selected. For the window's
own drag the device is known while the drag moves, so the pointer shows the action; another application's files are
known only at the drop, so with nothing held the drag shows a move and the drop copies what lies on another device. The
drop stays open while the operation runs (`Command::HoldDrop`), so the source hears it is finished only after the files
are in place, and with what was done: a move only when every
item left its place, a copy when every item was copied, and otherwise nothing (an item that failed, or one dropped on
the folder already holding it), so a source never removes items Files did not take. Holding the offer open is what the
drag-and-drop protocol asks of a target whose work ends later: the source has seen the drop performed and may free its
data (a move's originals, or the temporary files of an archive) only once it hears the drop finished, and the
compositor sets no time limit. Two things end a held drop early, as a cancel the source hears at once, while the
operation goes on: closing the window it was dropped on (its offer goes with its surface), and more than eight drops
held at once (the oldest is let go). A cancel makes a source keep its originals; a source that then frees temporary
files can make that copy fail, and the failure is listed. A list is taken whole or not at
all: one with a URI of another scheme or host, a relative path, a `.` or `..` step or a NUL is refused. A drag of the window's own items has nowhere to go
on one of the dragged folders or in the folder holding them all. The folders of the path bar take drops as the folder
area does when it shows them (a third `DropTarget`).

**Spring-loaded folders** (`spring`, `kit::spring`). A drag of files from any application, this one included, held
over a folder of the list or the grid, a folder place of the sidebar or a folder of the path bar for
`spring::SPRING_DELAY` (0.668 s) opens that folder in the same window; for the last `spring::SPRING_FLASH` (0.1 s) the
folder flashes, drawn as selected (the path bar's folder as the folder shown). Moving onto another folder starts the
wait again, and moving off every folder stops it; the folder shown, a file, a place that takes no drop (Recents, a
read-only medium, a folder of the dragged items) and the Trash open nothing. The drag goes on in the folder that opened,
so it can spring further down. The window remembers its folder, history, expanded folders, search, selection, view and
scroll position from before the drag first opened a folder: the drag leaving the window (`DropTarget::on_leave`), released where nothing in
the window takes it, or cancelled (Escape, or a refusal of the window's own drag) takes the window back there, the
history as it was (the folders sprung through are not in it), the search with its text in the field, the folders
expanded again as the listing reaches them, and the selection and scroll position once they are read again; a drop keeps the folder the drag opened, also while its file operation holds the drop open and when the
source later hears the held drop cancelled. The timers are `Command::After` messages of the dwell's
generation, so one of a dwell that ended does nothing.

**File operations** (`kit::jobs`, `kit::transfer`, `kit::progress`). Paste Item, Move Item Here, Duplicate, New Folder
with Selection and every drop, from Files or another application, run on a worker thread of their own; the window's
thread never copies or moves an item. An operation belongs to the process, not to a window: the window that started it
shows its row while it is open, and closing that window leaves it running. Files exits once no browser window is open,
no operation runs and the progress window lists nothing (after the notices it posted are sent), so a copy is never cut
off by a closed window. Each operation runs once; its outcome is handed over once.

* **The progress window** ("File Operations", 420 pt wide, a row of about 76 pt per operation up to four, then its list
  scrolls) lists every running operation: a folder symbol, what it does ("Copying “big.bin” to “Notes”"), a bar of the
  bytes done (of the items for a move to the trash), its Stop button and how far it came ("1.0 MB of 4.2 MB"). It opens
  by itself once an operation has run for one second (`PROGRESS_WINDOW_DELAY`), so a quick one never shows it, and closes
  by itself once it lists nothing. Window › Show Progress (enabled while it has something to list) and a click on a
  window's progress row show it again. Its Close button only hides it: nothing stops.
* **Stop**, in a window's row or the progress window, cancels that operation: the item being copied is removed again,
  the items before it stay where they arrived, the items after it stay where they were, and a drop's source keeps what
  it has. A stop is not a failure.
* **A failure** is listed in the progress window, which opens (again, if it was hidden): what was done ("Could not
  duplicate “secret.txt”", "Moved 2 of 3 items to “Documents”") and the first error as a sentence. Closing the window
  forgets it.
* **An end nobody saw**: when an operation ends while the progress window is hidden and no browser window is open, it is
  posted as a notification ("Copied “big.bin” to “Notes”", with the first error as its body); the window that started
  it, if open, announces the end instead.
* The window that started an operation, if still open, selects what arrived when it shows the folder it went to, edits
  the new folder's name after New Folder with Selection, and reads its free space again.
* Decisions: a move to the trash from a menu or Primary+Backspace stays on the window's thread (it renames, and is not a
  transfer), but a trashed folder's `directorysizes` line, which sums every file in it, is written from a thread of its
  own (`Trash::trash_unsized` and `PendingSize`; a folder put back first gets no line); a drop on the Trash is an operation like any drop. A failed operation is shown in the progress window and
  is no longer a notification while a window is open. The delay, the window's title and sizes and its wording are
  approximate; Stop is a text button, and the progress window gives no time estimate.

**Failures.** A failure of something a person did (open, a new folder, make alias, show original, put back, tags,
rename, a name that can't be used, copy and paste of text, move to the trash, a drop that cannot be taken) is one transient notification from "Files" on
the session bus (`org.freedesktop.Notifications`), posted in order from a thread of its own (`notice`). It names one
item, or counts several, and its body is the first error, with control and bidirectional formatting characters removed,
cut at 256 characters and escaped, because the body is markup. Without a session bus, or when a post fails, that notice
is written to stderr instead, with the reason. Failures that only explain the window (an entry that can't be read, the
free space, the default applications) stay on stderr. The wording is approximate.

**Accessibility** (`tests/kit-accessibility.rs`). Every control reads its role, name and state and does its default
action when assistive technology asks: the places and their eject buttons, back and forward, the view modes (radio buttons, the
shown one on), the View, Tags and Action pop-up buttons (each says whether its menu is open), share (disabled), the search field, the path bar's folders, the icon view's cells, the column view's lists and preview, the tab bar's tabs, the tags popover's rows (check boxes), the preview window's toolbar, the list's rows (selected, open or
closed, with their level and position; Click selects a row alone, Expand and Collapse open and close a folder), the
column headers (sorted up or down), the transfer row and its Stop button, the progress window's rows and Stop buttons, the name editor and the context menu. Icons
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
bus; a change only asks for a read on a thread of its own (at most one waiting), so the signal streams never fill and a flood
of signals another user sends the process costs one read, never the connection. Every call runs off the window's thread.
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
| Primary+1 / Primary+2 / Primary+3 | icon / list / column view |
| Right / Left in the column view | into the selected folder / back to the column before |
| Space, Primary+Y; arrows, Space, Escape in the preview window | open the preview window; go through the items, close it |
| Primary+T, Primary+W, Shift+Primary+W; Ctrl+Tab / Shift+Ctrl+Tab | new tab, close the tab shown, close the window; the next / previous tab |
| drag a tab along the tab bar; drag a column's divider (Option: every column) | move the tab; resize the column |
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
| Window › Show Progress, a click on a window's progress row | show the progress window (above); its Close button hides it, Stop stops one operation |
| right press, Ctrl with a press, Menu, Shift+F10 | context menu (above) |
| the toolbar's View, Tags and Action buttons | the view modes, the tags popover (without a selection the tags menu), the context menu, below the button |
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
| `rename`, `typeselect`, `spring` | interaction logic | `RenameTrigger`, `validate_name`, `needs_extension_warning`, `initial_selection`, `unique_name`; `TypeSelect::{type_text, tab}`; `SpringLoad::{hover, due, cancel, flashing}` (a drag's dwell over folders, with `SPRING_DELAY` and `SPRING_FLASH`) |
| `context_menu`, `menu_spec` | context menus | `menu_spec::{item_menu, background_menu, condense}` (the rows in order), `MenuItem`, `Command`, `common_apps` (the Open With list of a selection) |
| `menu_bar`, `search_field` | the menu bar and the search field | `menu_bar(MenuState)`, `MenuAction` (`chord`: its keys), `glyphs`, `chords`, `enabled`; `SearchWidth::{want, tick, follow_toolbar, width}`, `open_width` |
| `name_edit`, `operations` | rename in place and file changes | `NameEdit::{field, field_mut, commit}`; `rename_item` (no replace), `new_folder`, `drop_action`, `drop_into` (move or copy, `name copy` on a clash), `transfer` (the same for a list of items, with `Progress` and a `Cancel`), `duplicate_with` (Duplicate with them), `stopped` |
| `trash` | freedesktop Trash 1.0 | `Trash::{trash, trash_unsized, list, restore, delete, empty, purge_older_than}`, `PendingSize`; home and mount-point trash, `.trashinfo`, `directorysizes`; `trashed_item`, `forget` (an item moved out of a trash) |
| `thumbnail` | freedesktop thumbnail cache | `Thumbnailer::{lookup, lookup_at_least, create, get_or_create}`, `ThumbSize::for_icon`, `file_uri`, failure markers |
| `tags`, `mime` | metadata | `read_tags` / `write_tags` (`user.xdg.tags`), `TagColor` (seven label colours), `parse_reference_tags`; `detect`, `kind_group`, `kind_description` |
| `open` | opening files | `open_command` (default application of `mimeapps.list` through `cupertino-apps`; `Terminal=true` under `--terminal`), `command_with`, `open_with` (Open With list: default first, rest by name), `OpenError` |
| `layout::icon` | icon view | `ideal_cell_size`, `max_title_lines`, `layout` (flow / justified, 32 pt section headers), `IconLayout::{hit, hits_in_rect, neighbour}`, `FreeGrid` |
| `layout::list` | list view | `row_height`, `layout` (columns, rows with level and expanded state, disclosure, indentation), `ListLayout::{row_at, visible_rows}` |
| `history`, `places`, `recents` | navigation state | `History::{visit, back, forward}`; `standard_places` (the home folder, Applications, Desktop, Documents, Downloads, the computer, Trash), `recents_place`, `folder_icons`, `session_places` (`SessionPlaces`), `computer_name`, `applications_folder`; `recents::{parse_xbel, recent_entries}` (the bookmark file is read with `cupertino_apps::recent`, which `parse_xbel` and `XBEL_NAME` re-export) |
| `browser` | one window's folder | `Browser::{open, go, back, forward, parent, poll, sort_by, press, release, move_focus, select_all, type_text, activation, outline, expand, collapse, collapse_all, available_space, visit, return_to, is_loading}`: model, history (a `Visit` to go back to, with its selection), the list's outline of expanded folders, selection by path across reloads, nearest-ancestor fallback for a deleted folder |
| `item_art` | Files' own item images | `Art::{for_icon}` (folder, text, document, archive), `draw(art, px, seed)` (premultiplied RGBA) |
| `window_prefs` | the view and bars kept between windows | `WindowPrefs::{parse, text, load, save}`, `location`, `column_width` |
| `columns` | the column view's chain | `ColumnChain::{follow, folders, width, resize, widths}`, `layout` → `ChainLayout` (where each column sits and the scroll that keeps the last whole), `DEFAULT_WIDTH`, `MIN_WIDTH`, `MAX_WIDTH`, `PREVIEW_WIDTH`, `ROW_HEIGHT` |
| `preview` | what a preview shows | `kind` → `PreviewKind`, `read_text` / `text_of` (`TEXT_LIMIT`), `summary`, `details`, `Browse` (the items gone through, `step`), `Motion` (the window's open and close, `at` → `MotionFrame`), `WINDOW_SIZE` |
| `tab_strip` | a window's tabs | `TabStrip::{open, close, select, cycle, move_to, order, active_id}`, `tab_frames`, `drop_place` |
| `window`, `format` | the window's wording and metrics | `metrics`, `ViewMode`, `path_chain`, `status_text`; `human_size`, `available_size`, `relative_date` |
| `udisks` (feature `app`) | removable media | `devices` (the listed file systems and locked containers of udisks' managed objects), `Volumes::{watch, poll, mount, unlock, eject}` → `Report`, `Media` (the listed devices and the busy ones), `Passphrase`, `holders`, `notice` (a failure's notification text) |
| `notice` (feature `app`) | failures as notifications | `failure`, `unopened`, `bad_name`, `finished` (an operation that ended unseen) → `Notice`; `Notices::{start, post, flush}`; `shown`, `escape`, `explained` (an error as a sentence for a window) |
| `kit` (feature `app`) | the window on CupertinoKit; driven headless through `cupertino-ui-test` | `prepare(Start::open(folder, places, types, recents, notices, applications, terminal))`, with `with_file_icons`, `with_thumbnails`, `with_places_source`, `with_instance` and `with_activation_token`, then `run` or a `TestDriver<Files>`; `grid` (the icon view), `list` (the list view's presses and band), `band` (the rubber band), `clicks` (double-click and click-to-rename), `toolbar` (pop-up buttons, path bar folders), `sidebar` (material, keyboard watch), `places` (live places), `transfer` (the worker of a file operation: `Work` → `Outcome`), `jobs` (the file operations of the process and when the progress window shows; `PROGRESS_WINDOW_DELAY`, `Start::with_progress_delay`), `progress` (the progress window, and the click on a window's row that shows it), `remote` (what the calls of later launches and of the desktop do), `context` (context menus), `search` (the field's slot and "No Results"), `dragged` (drag images), `drops` (drops onto folders, places, the path bar and the folder shown), `spring` (spring-loaded folders and the way back), `item_icons` (items' icons, thumbnails and previews), `columns` (the column view), `peek` (the preview window), `tabs` (the tab bar and a tab's parked state), `tag_editor` (the tags popover) |
| `thumbnail_service` | thumbnails of the items in view | `ThumbnailService::{new, want, forget, get, queued, memory}`, `Stamp`, `has_thumbnail`, `MEMORY_BUDGET`, `WORKERS` |
| `instance` (feature `app`) | one process per session | `Service::serve` (the name and `org.freedesktop.Application`; `take` the waiting `Call`s), `serve_or_forward` → `Launch::{Serve, Forwarded}`, `Target::{of_argument, of_uri, uri, folder}`, `Request`, `Action`, `take_activation_token`, `FORWARD_TIMEOUT`, `BUS_NAME`, `OBJECT_PATH` |
| `main` (feature `app`) | the binary | arguments, the hand-over to a running Files, the session's places, types, associations and buses, then `kit::run` |

### Measured defaults

Values live in `metrics.rs`.

| Area | Value |
|---|---|
| Icon view | 64 pt icons (16–512), grid spacing 54 (1–100), 12 pt text (10–16), label below; cell = icon + 12.5 % padding, label width interpolated by the spacing, 1–2 lines |
| List view | 16 pt icons (16–32), 13 pt text, row = max(icon, font line) + 4 with no intercell spacing, a 20 pt pitch, 15 pt indentation; kinds are the design's type descriptions, capitalised |
| Column view | 245 pt columns (100–1000), 22 pt rows, a 240 pt preview column (the 312 pt template width is not used) |
| Chrome | sidebar 223 pt default, 128 pt minimum, rows 24/28/32 pt; unified toolbar 52 pt; status bar 20 pt, path bar 28 pt; search field 112 pt at rest |
| Timing | sidebar toggle 0.3 s, slow motion ×10; type-select reset after max(2 s, 2 × key-repeat delay); search field 0.2 s; progress window after 1 s (approximate) |
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
  use is checked), Quick Preview and the three usable view modes. Disabled: Delete Immediately…, Get Info, Show
  Inspector, Compress, Slideshow, Copy as Pathname, Paste Item Exactly, Share, Quick Actions, Empty Trash, Other…, Duplicate Exactly,
  Use Groups, the Group By choices, Snap to Grid, Date Added, Clean Up By, Show View Options and the Gallery view.
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
* A drop from another application shows a move while it is held even when it will copy across devices. The drag badge
  holds three digits.
* A drag that ends without a drop flies back to the point it was pressed at, kept relative to its window: the compositor
  knows the window, not the item, so after the item is removed (or scrolled away by something other than a spring-loaded
  folder, which the window returns from) the image still lands where the item was. It fades in place only when the
  window is gone or hidden.
* Thumbnails are made for PNG, JPEG and WebP pictures only (what `cupertino-thumbnails` decodes): no external
  thumbnailers, so videos, PDFs and other documents show their type's icon. Thumbnails are decoded for a 2× display;
  a 1× display draws them shrunk. The sidebar keeps the design's template symbols, as the places are not files.
* The column view's selection is inset 4 pt in its column, as the list's is, where the design insets it 10 pt. Its text
  preview uses the asset pack's fixed-pitch face, and the UI face when the pack has none. The preview window decodes
  PNG, JPEG and WebP pictures; PDFs and other documents show their icon and details.
* The tags popover has no Show All Tags… row, and tags cannot be renamed, recoloured or removed from the sidebar.
* The toolbar has no Group By popup (the window lists without sections), no share, preview or path controls and no
  sidebar toggle. The search filters the folder shown by name (the design searches the computer by default); its field
  has no clear button.
* The path bar does not shorten the names of the folders above the one shown when it runs out of room.
* The free space is read again when the listing changes and after a transfer, not when another program writes
  elsewhere on the file system.
* With no browser window open and the progress window hidden, nothing of Files is on screen while an operation runs:
  launching Files again opens a browser window, whose Window › Show Progress shows it, and the desktop may call the
  `show-progress` action, but the dock has no menu item for it. The progress window gives no time estimate, and an
  operation's icon is a symbol, not the folder's own icon.
* The folder icons of the user directories are read when the application starts; a user directory moved later keeps
  the plain folder icon until Files starts again.
* The pasteboard is the application's own, not the session's clipboard.
* Per-folder view settings and the sidebar width are not saved; the view mode and the bars are kept for the next window,
  not per folder.
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
