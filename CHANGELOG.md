# Changelog

Notable changes to Cupertino, newest first. The project is pre-1.0: interfaces and configuration files can change between candidates.

## v0.1.0-rc3 — 2026-10-27

Tagged on `c47d32a`. This entry covers the changes since the initial import (`cf9f5af`).

### Input

- Pointer acceleration is designed on physical speed and can be turned off, so a mouse moves predictably at every speed ([#1](https://github.com/orafaelfragoso/cupertino/issues/1)).
- One wheel notch scrolls three lines ([#2](https://github.com/orafaelfragoso/cupertino/issues/2)).
- The natural-scrolling setting applies live to wheels and touchpads ([#3](https://github.com/orafaelfragoso/cupertino/issues/3)).
- Settings > Mouse shows a resolution row per connected mouse, read from the session, with a typed value in a sheet.
- A press outside a popup chain dismisses it and is consumed; popup subsurfaces take input and belong to the chain; a session lock ends the grab ([#5](https://github.com/orafaelfragoso/cupertino/issues/5)).

### Text

- One shared single-line text field provides a caret, selection, clipboard, undo and a per-field length limit. Settings search, the launcher and Files rename use it ([#16](https://github.com/orafaelfragoso/cupertino/issues/16), [#4](https://github.com/orafaelfragoso/cupertino/issues/4)).
- Pasted and composed text is sanitised; a paste is capped at 1 MiB and served off the event loop.

### Menus

- Open menus in Settings and Files work from the keyboard: arrow keys, Return, Space, Escape, `Super+.` and type-to-select ([#18](https://github.com/orafaelfragoso/cupertino/issues/18)).
- Settings closes an open menu on an outside press, a pane change or an appearance change ([#5](https://github.com/orafaelfragoso/cupertino/issues/5)).

### Settings

- A Wallpaper pane with a grid of the packs' and added folders' pictures, a Current preview, and an Add Folder and Add Photo chooser with folder and picture icons, hover and wheel scrolling ([#7](https://github.com/orafaelfragoso/cupertino/issues/7)).
- Image wallpapers are chosen per appearance and decoded once on a worker; the procedural fallback is unchanged, and a decoded picture is released when no output wants it.
- A picture's title and photographer come from the pack's `credits.txt`.
- Folder and picture listings run on threads and show Loading; one listing per folder runs at a time, and a listing that cannot start releases its folder.
- Relative wallpaper paths are refused, the credits file is capped, and thumbnails have size and picture limits; the shared thumbnail cache is used by Files and Settings.
- The content and toolbar background matches Files ([#8](https://github.com/orafaelfragoso/cupertino/issues/8)).
- Failed bus and file operations show plain text, with the system error in the log; search matches every word in any order; accessibility controls show the values the server holds.

### Shell and session

- An installed session starts the full shell: `/etc/cupertino/compositor.json` supplies the process list, under the user's own `compositor.json` when there is one ([#17](https://github.com/orafaelfragoso/cupertino/issues/17)).
- `run-cupertino.sh` starts the compositor and every shell process from a text console, with a fresh session bus and a log at `~/cupertino-tty.log`.
- `capture.sh` renders the desktop in a fixture home and never starts the login agent or touches the host session ([#10](https://github.com/orafaelfragoso/cupertino/issues/10)).
- The lock agent is part of the installed and TTY shell lists, so locking works there.

### Packaging and release

- One manifest builds Arch, Debian and RPM packages. `cupertino-data` carries the default asset pack, and desktop entries ship for Cupertino's own apps.
- `/etc/cupertino/compositor.json` is installed as a config file in all three formats.
- `v<version>-rc<N>` tags build a draft prerelease with packages, a source archive and `SHA256SUMS`.
- Arch packages build without debug information so the check build fits a hosted runner.

### Tests and CI

- The suite runs under nextest with a two-minute limit per test, and the hanging menu bar D-Bus test is fixed at its cause ([#11](https://github.com/orafaelfragoso/cupertino/issues/11)).
- Keyboard-map tests use pinned keymap data instead of the host's ([#14](https://github.com/orafaelfragoso/cupertino/issues/14)).
- The session settings setter test no longer caches a failed first read ([#15](https://github.com/orafaelfragoso/cupertino/issues/15)).
- CI builds the docs book with mdBook 0.5.4 and checks its internal links with lychee 0.24.2 ([#12](https://github.com/orafaelfragoso/cupertino/issues/12)).
- The test build carries no debug information, keeping the CI job inside the runner's disk.

### Documentation

- Public README, CONTRIBUTING (with the CLA), SECURITY, Code of Conduct, and a docs book covering architecture, building and running, asset packs and trying Cupertino.
- Bug report and feature request forms.
- A release-candidate hardware checklist.

### Known missing

Open p0 and p1 issues at this tag: [#6](https://github.com/orafaelfragoso/cupertino/issues/6), [#19](https://github.com/orafaelfragoso/cupertino/issues/19), [#20](https://github.com/orafaelfragoso/cupertino/issues/20), [#21](https://github.com/orafaelfragoso/cupertino/issues/21), [#22](https://github.com/orafaelfragoso/cupertino/issues/22), [#23](https://github.com/orafaelfragoso/cupertino/issues/23), [#24](https://github.com/orafaelfragoso/cupertino/issues/24), [#25](https://github.com/orafaelfragoso/cupertino/issues/25), [#26](https://github.com/orafaelfragoso/cupertino/issues/26), [#27](https://github.com/orafaelfragoso/cupertino/issues/27), [#28](https://github.com/orafaelfragoso/cupertino/issues/28). The release-candidate tracking issue is [#13](https://github.com/orafaelfragoso/cupertino/issues/13).

### Install or upgrade

From a checkout, on a text console (`Ctrl+Alt+F3`):

```bash
./run-cupertino.sh
```

For an installed session, download the Arch packages `cupertino` and `cupertino-data` from the `v0.1.0-rc3` release and install both from the same candidate:

```bash
sudo pacman -U cupertino-*.pkg.tar.zst cupertino-data-*.pkg.tar.zst
```

Both packages are replaced together. `/etc/cupertino/compositor.json` is a package config file, and files under `~/.config/cupertino/` are read as before.
