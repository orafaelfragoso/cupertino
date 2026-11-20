# cupertino-shell-desktop

The desktop items of the Cupertino desktop: the items of the Desktop folder as icons on the main display, over the
wallpaper and under every window. It is a component of the shell host (`cupertino-shell`, `docs/architecture/shell-host.md`):
`Desktop` implements `cupertino_kit_shell::Component` (`ComponentName::DESKTOP`), and the host runs it on a Kit instance
and a thread of its own, with its own Wayland and session bus connections, when the server's `components` list names
`desktop`. It owns no bus name.

## Contents

| Module | Contents |
|---|---|
| `arrangement` | Where the items sit without a toolkit: the `Grid` that starts at the top right corner below the menu bar's room and fills each column downwards before the next one to its left; `arrange` (each item in its kept cell, the rest in the first free ones, the overflow piled on the last), `dropped` (a drag moves the items as many cells as the held one, each to the nearest free cell), `cleaned` (Clean Up), `neighbour` (the arrow keys), `item_frames` (backdrop, icon and name in a cell) and `DesktopPrefs`, what is kept between sessions in `$XDG_CONFIG_HOME/cupertino/files/desktop.json` (`location`, the file the desktop kept when it was part of Files). |
| `metrics` | The measured sizes: icon, grid pitches, backdrop, name capsule and its shadow, the dragged items' opacity. |
| `Desktop` (`component`) | The component: one `Layer::Bottom` layer surface on every output (namespace `cupertino-desktop`, anchored to all four edges, exclusive zone 0 so the server configures it to the area the menu bar, the dock and other surfaces leave, `Event::Configured`, and the grid lays the items out within it; drawn headless by `prepare`, with no server, the grid keeps the menu bar's room itself), persistent so the server holds its picture while the component restarts (D19), taking the keyboard once a press lands on it. The items show on the main display (the first output the server lists); the others show the empty desktop that takes presses and the context menu. |
| `view` | What each desktop draws and what its presses mean: the icons (thumbnails for pictures), the selected icon's backdrop and the accent capsule of a selected name, the rubber band (fading in and out with the catalogue's `Timing::DESKTOP_BAND_FADE_IN` and `DESKTOP_BAND_FADE_OUT`), the dragged items, the rename field; click, Shift / Primary click, double click, the band, a drag of the selection, a secondary press for the context menu. |

`prepare` gives a driver of the headless window system the desktop of a folder on one screen of a given size, without
the shell's roles, a kept arrangement or a session bus (the capture uses it).

## What it does

- The listing and its watch are Kit's file family (`DirectoryModel`), the icons and thumbnails its `ItemIcons`; a file
  made, renamed or removed in the Desktop folder by anyone shows at once.
- The context menu on items: Open, Open With (the applications that open the first selected item, read on the I/O
  pool), Move to Trash, Rename, Duplicate, Make Alias, Copy. On the desktop: New Folder (made where the menu opened,
  then renamed), Paste, Change Wallpaper… (`cupertino-settings --pane wallpaper`), Sort By (None, Name, Kind, Date
  Modified, Size), Clean Up.
- Keys, once a press gave the desktop the keyboard: Return or Enter renames the only selected item, Primary+O and
  Primary+Down open the selection, Primary+Backspace moves it to the trash, Primary+D duplicates it, Primary+C and
  Primary+V copy and paste, Primary+Shift+N makes a folder, Primary+A selects every item, the arrows move the
  selection, Escape clears it (or ends the rename without renaming).
- A folder opens in a window of the file manager (`cupertino-files PATH`), a file with its default application; each
  launch asks for an activation token naming the application and starts it through the host (`host::spawn`).
- Files dropped from another application on the main display are copied into the Desktop folder; items dropped on a
  folder of the desktop move into it.
- Every file access runs on Kit's I/O pool (`cupertino_kit::task::io`): the start's reads (the folder, made when
  missing, the file types, the icon packs, the kept arrangement), what opens an item, renames, new folders, aliases,
  the trash (the sizes of trashed folders recorded afterwards) and the arrangement written after each change, one write
  at a time. Copies and moves are `kit::files::operations::Transfer`s run one slice at a time, each slice an I/O task.
  Failures are notices posted on the component's session bus connection (`Notices::on`), on standard error without one.
  The component starts no thread of its own and never blocks its loop.

## What a restart loses

A failed component is started again alone by its host (D16). While it is down the server keeps each desktop's last
picture (the persistent layer surface, D19); the new instance takes its place with its first frame. It shows no window
until its start's reads came back and the folder's first listing is in (or the folder cannot be listed), and it is not
`ready` before (`Program::showing`, `shell-host.md#holes-and-holds`), so the held picture stays until a first frame that has the items;
the server's 10 s end the hold if the listing takes longer. Kept across a restart: the items and their cells (`desktop.json`, written after each
change) and the files themselves. It loses:

- the selection, the rubber band and a drag under way;
- a name being edited, which keeps its old name;
- an open context menu;
- what Copy put aside for Paste on the desktop (the clipboard text it set stays with the server);
- a copy or move under way: the slice that was running ends, the items not reached are not copied or moved;
- an arrangement change whose write had not ended, and notices not yet posted.

## Captures

`examples/desktop_capture.rs` draws the items headless over a flat fill, at rest and with `--state select`, `rename` or
`drag` acting on `notes.txt`:

```sh
cargo run -p cupertino-shell-desktop --example desktop_capture -- desktop/items --state rename --appearance both --scale 2 --out target/captures
```

## Tests

`tests/arrangement.rs` covers the grid and the arrangement. The component itself is tested against the real server in
`crates/cupertino-session-tests/tests/shell-desktop.rs`: panics at paint, dispatch and in a helper, 50 restarts with the
dock drawing and the desktop's picture held, the restart-leak bound, and a file created in the Desktop folder showing,
renamed and moved to the trash through its item.
