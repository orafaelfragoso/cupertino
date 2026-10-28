# cupertino-files

File-manager core with no window-system or toolkit dependency, plus the file manager itself: the `cupertino-files`
binary (feature `app`) runs it as an application window on `cupertino-wayland-client` and `cupertino-protocol-app`,
drawn with `cupertino-ui-wayland`. The library implements the data, interaction logic and layout metrics a native file
manager (and a desktop-icon layer) needs.

## Usage

```sh
cargo test -p cupertino-files --features app                    # 140+ unit tests, 2 workflow tests, 3 session handler tests
cargo run -p cupertino-files --features app -- [--terminal PROGRAM] [FOLDER | file:///FOLDER | trash:///]  # the window (default $HOME), in a running session
cargo clippy -p cupertino-files --all-targets
cargo run -p cupertino-files --release --example files_samples # PNGs in output/ (needs CUPERTINO_ASSET_DIR)
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

The window lists the folder (default `$HOME`) in icon or list view with the asset directory's file icons (`folder`, the folder icons of the home folder and the user directories, `document.*
by type; a symbolic link gets the alias badge; `Renderer::new` and `FileIcons::new` fail when the directory lacks an icon or symbol a window can show, and nothing is drawn in its place), a sidebar of
standard places (drawn with the `symbol.* place glyphs, tinted with the accent colour: the tint and the 16 pt size are
approximate, the design's sidebar symbol colour is not known) and a 52 pt toolbar: back / forward and the folder's name, then the view-mode popup, Group By (disabled until), Tags, the Action (…) popup and the 112 × 22 pt search field (Action and search sizes, the others take the arrange popup's and the share button's sizes, the order, gaps and glyphs are approximate; items give way right to left of importance when the toolbar narrows). The title is 15 pt (the unified toolbar's Title3 style, its weight approximate). A 1 pt line runs under the strip and under the list header, as a screenshot shows (approximate; kept over 's reading that the automatic separator shows no line at rest); the column header is part of the strip and takes its fill (the resting colour ). The window uses the unified title bar style
(`TitlebarStyle::Unified`): its surface is the whole window, the sidebar runs the full height under the window control buttons over
the server's `Sidebar` material (a backdrop), and the server draws only the buttons. The toolbar strip moves the window
(`xdg_toplevel.move`) and a double click zooms it. Changes on disk appear live.

The sidebar lists the design's default Favorites, Recents, Applications, Desktop, Documents and Downloads (the nearby-sharing
item has no counterpart), those that exist, and the computer (the file system root, under the pretty host name of
`/etc/machine-info`, else the host name) and Trash under Locations (approximate, a screenshot); names are cut at
the tail. The home folder and the other user directories keep their folder icons. Recents is the virtual folder `recent:///: the existing files of
`$XDG_DATA_HOME/recently-used.xbel`, listed again whenever the file is rewritten, in the window's sort order. Applications is
the first `applications` folder of `$XDG_DATA_DIRS` (the system's desktop entries), a real folder rather than a merged
virtual one (approximate).

In the list every folder has a disclosure triangle: a click on it, Right and Left expand and collapse the folder in place, its
items one level (15 pt) deeper, read only when it expands and kept live. Left on an item inside an expanded folder
moves to that folder; collapsing moves a selection inside to the folder (both ). Another view or another folder shows
the folder collapsed.

Under the view the 28 pt path bar and the 20 pt status bar can be shown; both are hidden by default.
The path bar shows the folder chain from the root (or from Trash / Recents) with each folder's icon; a click on a
folder goes there. The status bar reads "N items, X available" (`statvfs` of the folder; format approximate from the
screenshot).

`data/applications/cupertino-files.desktop` names the application "Files" for the menu bar, which reads `Name= of
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
| double click (0.5 s), Return, Super+Down | open: a folder in this window, files with their default application (`mimeapps.list`, read at each open; `Terminal=true` applications under `--terminal PROGRAM` as `PROGRAM -e`) |
| Super+[ / Super+] or the toolbar arrows | back / forward |
| Super+Up | enclosing folder, with the folder just left selected |
| Super+1 / Super+2 or the view popup | icon / list view (the popup lists Columns and Gallery disabled) |
| Tags popup, Action (…) popup | toggle a tag colour on the selection; the selection's context menu, or the folder's without a selection |
| click the search field and type | show the folder's items whose names contain the text, flat; the field is a shared text field (`cupertino-text-field`: caret with blink, word and line moves, Shift selection, click, double and triple click, drag, copy, cut and paste, undo; at most 1 KiB); Escape clears, Return / Tab keep it; another folder ends it |
| Right / Left in the list, a click on a triangle | expand / collapse the focused folder; Left inside an expanded folder moves to it |
| Alt+Super+P / Super+/ | show or hide the path bar / the status bar |
| arrows, Home / End, Tab, typing; Shift / Ctrl click; Super+A; a list column header | selection, type-select, select all, sort (again: reverse) |
| right click | context menu (`menu_spec`): on an item the design's items menu, on empty space its background menu; rows without a backend are shown disabled (Policies). Hover, click, arrow keys, Return and Escape work; a submenu opens 0.2 s after the pointer rests on its row; holding Alt (and Shift+Alt, Ctrl) swaps the rows that have alternates |
| press on empty space and drag | rubber band (icon and list view): the items it touches are selected live; Shift or Ctrl toggles them against the selection kept; near the top or bottom edge (50 pt) the view scrolls up to 20 pt every 0.05 s |
| Super+Backspace | move the selection to the trash (`$XDG_DATA_HOME/Trash`, freedesktop Trash 1.0) |
| click on the name of the only selected item, or Rename | edit the name in place in a shared text field (`cupertino-text-field`: caret with blink, word and line moves, Shift selection, click, double and triple click, drag, copy, cut and paste, undo): the name without its extension is selected; Return commits, Escape cancels; a name that exists is refused (never replaced) and an invalid one keeps the editor open |
| press an item and move 5 pt | drag the selection onto a folder of the window or a place of the sidebar: moves there (Alt copies, Super moves, copy across volumes, a clash renames the arrival `name copy`); onto the trash place it is trashed; Escape cancels |

## Design

### Modules and API

| Module | Role | Main items |
|---|---|---|
| `model` | async directory model | `DirectoryModel::open(dir, ModelOptions)`, `poll` / `poll_timeout` → `Vec<ModelDelta>` (`Reloaded`, `LoadComplete`, `Inserted`, `Removed`, `Changed`, `Moved`, `DirectoryGone`), `enumerate_blocking`, `set_sort` |
| `natural`, `sort`, `group` | name collation (reference options 0x371: case-, width-insensitive, numeric, localized), ordering, sectioning | `NameKey`, `compare_names`, `SortSpec { key, ascending, folders_first }`, `sort_entries`, `insertion_index`, `GroupBy`, `group_entries`, `date_bucket`, `size_bucket` |
| `settings` | per-folder view state | `ViewSettings` (icon / list / column / gallery), `ViewStyle`, `default_columns`, `SettingsStore` (atomic JSON, keyed by device + inode), `FolderKey` |
| `selection` | selection model | `Selection::{mouse_down, mouse_up, click_empty, move_focus, rubber_band_*, on_insert, on_remove, on_move}` (the window's `Frame::hits_in` gives it the band's items), `Modifiers` |
| `rename`, `typeselect`, `spring` | interaction logic | `RenameTrigger`, `validate_name`, `needs_extension_warning`, `initial_selection`, `unique_name`; `TypeSelect::{type_text, tab}`; `SpringLoad`, `ticks_from_seconds` |
| `context_menu`, `menu_spec` | context menus | `menu_spec::{item_menu, background_menu, condense}` (the descriptor table), `common_apps`, `Menu::{open, set_mods, hover, poll, press, press_at, navigate, hit}` (levels, alternates, hover, 0.2 s submenu timer, no-wrap arrow keys), `Command` |
| `name_edit`, `operations` | rename in place and file changes | `NameEdit::{field, field_mut, commit}`; `rename_item` (no replace), `new_folder`, `drop_action`, `drop_into` (move or copy, `name copy` on a clash) |
| `trash` | freedesktop Trash 1.0 | `Trash::{trash, list, restore, delete, empty, purge_older_than}`; home and mount-point trash, `.trashinfo`, `directorysizes` |
| `thumbnail` | freedesktop thumbnail cache | `Thumbnailer::{lookup, lookup_at_least, create, get_or_create}`, `ThumbSize::for_icon`, `file_uri`, failure markers |
| `tags`, `mime` | metadata | `read_tags` / `write_tags` (`user.xdg.tags`), `TagColor` (seven label colours), `parse_reference_tags`; `detect`, `kind_group`, `kind_description` |
| `open` | opening files | `file_type` (MIME globs, no content sniffing), `open_command` (default application of `mimeapps.list` through `cupertino-apps`; `Terminal=true` under `--terminal`), `command_with`, `open_with` (Open With list: default first, rest by name), `OpenError` |
| `layout::icon` | icon view | `ideal_cell_size`, `max_title_lines`, `layout` (flow / justified, 32 pt section headers), `IconLayout::{hit, hits_in_rect, neighbour}`, `FreeGrid` |
| `layout::list` | list view | `row_height`, `layout` (columns, rows with level and expanded state, disclosure, indentation), `ListLayout::{row_at, visible_rows} |
| `layout::column`, `layout::gallery` | column and gallery views | `column::layout`, `gallery::strip_metrics`, `gallery::layout` |
| `preview` | quick-preview geometry | `ZoomTransition::{open, close, at}`, `resize_duration`, `panel_frame`, `fit_content` |
| `render` | CPU renderer | `Renderer::{icon_view, list_view}` (scrolled, visible items only), `icon_layout`, `list_layout`, `Palette::{builtin, from_theme}`, `truncate_middle`, `truncate_tail`, `wrap_label`, `available_size` |
| `history`, `places`, `recents` | navigation state | `History::{visit, back, forward}`; `standard_places` (Recents, Applications, Desktop, Documents, Downloads, the computer, Trash), `folder_icons`, `session_places` (`SessionPlaces`), `computer_name`, `applications_folder`; `recents::{parse_xbel, recent_entries}` (the bookmark file is read with `cupertino_apps::recent`, which `parse_xbel` and `XBEL_NAME` re-export) |
| `browser` | one window's folder | `Browser::{open, go, back, forward, parent, poll, sort_by, press, release, move_focus, select_all, type_text, tab, activation, outline, expand, collapse, toggle_expanded, disclose_focus, collapse_all, available_space}: model, history, the list's outline of expanded folders, selection by path across reloads, nearest-ancestor fallback for a deleted folder |
| `window` | window layout and painting | `WindowLayout` (toolbar, sidebar, browser, path and status bars), `Bars`, `path_chain`, `path_segments`, `status_text`, `Frame::{hit, reveal, scroll_by}`, `WindowPainter`, `ViewMode`, `metrics` |
| `app`, `main` (feature `app`) | the binary | toplevel, input, `open` (default application, one activation token per file), asset file icons (`cupertino-icons::document`), inotify wake-ups through the connection's `Waker`, session appearance |

### Measured defaults

Values live in `metrics.rs`.

| Area | Value |
|---|---|
| Icon view | 64 pt icons (16–512), grid spacing 54 (1–100), 12 pt text (10–16), label below; cell = icon + 12.5 % padding, label width interpolated by the spacing, 1–2 lines |
| List view | 16 pt icons (16–32), 13 pt text, row = max(icon, font line) + 4 with no intercell spacing, a 20 pt pitch, 15 pt indentation; kinds are the design's type descriptions, capitalised |
| Column view | 245 pt columns (min 100), 312 pt preview column |
| Chrome | sidebar 200 pt default, 128 pt minimum, rows 24/28/32 pt; unified toolbar 52 pt; status bar 20 pt, path bar 28 pt |
| Timing | spring-loading delay 0.5 s; sidebar toggle 0.3 s, slow motion ×10; type-select reset after max(2 s, 2 × key-repeat delay) |
| Selection look | icon bubble: the icon enlarged by its longest side / 16, that radius, quaternary label fill, white 50 % border of min(2, trunc(padding)), the same in an inactive window; the label pill is the accent (grey when inactive). Rubber band: quaternary label fill, 1 pt tertiary label border, square corners |
| Context menu | item 22 pt, separator 11 pt, 5 pt edge padding, 6 pt corner radius, 4 pt highlight inset 5 pt, 13 pt text; title at 15 pt, 14 pt trailing padding; submenu 5 pt over its parent, first row level with the parent's, after 0.2 s; arrow keys stop at the ends. Entry order from the design's descriptor table |
| Names | compare case-insensitively, numerically, width-insensitively, diacritics significant |

### Policies

* The context menus are the design's descriptor table walked in order: New Folder with Selection, Open
  (Alt: Open in New Window), Open With (Alt: Always Open With), Show Original (links), Put Back (in the trash), Move to
  Trash (Alt: Delete Immediately…), Get Info, Rename, Compress, Duplicate, Make Alias, Quick Preview, Copy, Share, the tag row
  and Quick Actions; and for empty space New Folder, Paste Item, Get Info, the view row, Use Groups, Group By, Sort By,
  Clean Up By and Show View Options. Separators are condensed as the design does. Rows the design hides for contexts this window
  lacks (volumes, desktop, cloud, discs, packages, servers, sidebar) are left out. Enabled: Open, Open in New Window (a
  window of this program), Open With, Show Original, Put Back, Move to Trash, Rename (one item), Duplicate (`name copy`),
  Make Alias (a symbolic link `name alias`), New Folder, New Folder with Selection, the tag dots (the `user.xdg.tags`
  attribute), Sort By (the choice in use is checked) and the two usable view modes. Disabled, with their reference titles:
  Delete Immediately…, Get Info, Show Inspector, Compress, Quick Preview, Slideshow, Copy, Copy as Pathname, Paste Item and its
  alternates, Share, Quick Actions, Empty Trash, Other…, Duplicate Exactly, Use Groups, the Group By choices, Snap to Grid,
  Date Added, Clean Up By, Show View Options and the Columns and Gallery segments. The menu is kept inside the window, has no item blink and no safe-triangle
  tracking; the menu width has a 150 pt minimum that the design applies only to type-select menus, and the
  arrow column is 16 pt wide (approximate). The dot size of the tag row, the view row's size and the check mark are
  approximate.
* A rename that changes the extension is not confirmed (the design asks; there is no dialog here). The name editor,
  the drop outline and the new folder's default name numbering are approximate.
* The collation fold table, date and size group boundaries, thumbnail-size choice, quick-preview easing/velocity and
  disclosure width are approximate and labelled so in the source.
* The renderer draws; input handling lives in `browser` and `app`. There is no animation clock yet.
* Only `SettingsStore`, `Trash`, `Thumbnailer` and the tag functions write files, into caller-supplied locations (tests use temporary directories).

**Text.** The `ui` font is a variable font whose optical size follows the point size, so every `TextRenderer` owner calls `set_scale` with the output scale before it draws and measures (`frame`, `paint` and `paint_menu` set it from their `scale`; the context menu is laid out in points): text is laid out in points at the same optical size it is drawn with, at every output scale.

## Open issues

* The toolbar has no share, preview or path controls and no sidebar toggle: their sizes are as designed but nothing
  in the application stands behind them; items that do not fit are dropped, not moved into an overflow menu. Group By waits
  for grouping. The Tags popup is a menu of the seven colours, not the design's tag editor. The search filters the
  folder shown by name (the design searches the computer by default); its field has no clear button. The toolbar's item order, the title's position and
  type, the strip's separator and the sidebar's top inset are approximate. The toolbar's 8 pt side insets are the leading and
  trailing fields of the unified metrics record; fields 4 to 12 of the record (item viewer layout) are not
  separated yet.

* Per-folder view settings are not saved.
* No hidden-files toggle. Alt+Right (expand every folder inside) is not built, and the expanded folders are not remembered
  per window or folder. The triangle is a drawn chevron 12 pt wide at the row's left edge (approximate).
* The path and status bar toggles are not saved (the design restores them with the window) and have no menu.
  The status bar shows no selection count, no zoom slider and no progress; the path bar's shrinking rule, text size and
  spacing are approximate, and the root segment wears the computer's icon instead of a volume's.
* Recents is listed in the window's sort order, not by last use, and opening a file does not add it to the list. Applications
  shows the desktop entry files of one folder; opening one does not launch the application.
* The rubber band of the list view has no reference
  it draws the icon view's band and selects the rows its span crosses (approximate).
* Drag stays inside the window: dragging items to another application, or receiving a drag from one, needs the client
  library to start and receive `wl_data_device` drags (it exposes neither the origin surface nor the data device), and the
  `text/uri-list` of the items (`thumbnail::file_uri`) is not offered yet. No drag image is drawn, and a folder does not
  spring open under a drag (`spring::SpringLoad` is not wired).
* The trash place appears in the sidebar only at the next start after the first item was trashed.
* The lock badge is not drawn, because entries carry no permission bits.
* Errors go to stderr only: a file without an application is reported there, not in a dialog.
* Opening several files opens each without confirmation; a mixed selection enters the first folder only (approximate).
