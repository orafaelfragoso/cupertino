# Changelog

Notable changes to Cupertino, newest first. The project is pre-1.0: interfaces and configuration files can change between candidates.

## Unreleased

Changes on `main` since `v0.1.0-rc4`.

### Input

- Apps such as games and virtual machines can lock or confine the pointer, also when they draw in a subsurface. A notice shows while an app holds the pointer, and `Super+Escape` releases it for the whole app.
- An app can ask to receive the system shortcuts. A release chord that no configuration can remove gives them back, and a notice stays visible while the app holds them.
- Two modifier profiles, Cupertino and classic PC. System shortcuts match the physical key, with a table for each profile.
- Under the classic PC profile, tapping `Super` alone opens the launcher (a gesture or a mouse-keys key cancels the tap), `Super+Down` restores a tiled or zoomed window before it minimizes, `Ctrl+Tab` moves between key views in text views, the menu bar shows PC key names, and `Ctrl` is the command key in Settings and in text fields.

### Text

- A character the first font lacks is drawn from another loaded font, one character at a time.
- Emoji written with the emoji presentation selector are drawn from a colour font, and colour glyphs are cached once per size.

### Menus

- The system menu has its items, live shortcuts, Recent Items and About This Computer. Lock Screen is always in it, and it closes when the session agent goes away ([#43](https://github.com/orafaelfragoso/cupertino/issues/43)).
- Session options the session agent doesn't offer are shown disabled. If they can't be read, every option stays offered, and a failed later read keeps the previous ones ([#42](https://github.com/orafaelfragoso/cupertino/issues/42)).
- An app's exported menus are merged with the system menus, and an app without menus gets standard fallback menus. Edit menu items go to the key window.
- Files and Settings publish their own menus.
- Menus follow the focused surface only. GTK menus follow their app's menu and action changes live and keep a literal `&`, and a menu that failed to load is loaded again. The app menu services are offered only while a menu bar runs.
- Help searches every enabled menu item, and `Shift+Super+/` opens it with the search field focused.
- Holding `Alt` in the Window menu offers Minimize All, Zoom All, Arrange in Front and Close All, and the menu bar offers Force Quit for the current app.
- Menu shortcuts are drawn as symbols the embedded font has, Escape included.
- The menu bar text stays readable over the wallpaper under it, the built-in procedural wallpaper included.

### Settings, Files and theme

- An About This Computer window in Settings. Long values wrap, and each display shows its name, diagonal, mode and scale.
- Settings keeps running on a compositor without window activation.
- Files opens files through the shared app launcher and file typing.
- Files and Settings redraw only what changed, including Files' content rows, drop outline and selection rectangle ([#40](https://github.com/orafaelfragoso/cupertino/issues/40)).
- Files highlights a sidebar place as a drop target, with an outline when Increase Contrast is on.
- Neutral palettes reach 4.5:1 for text and 3:1 for borders and the focus ring. Switches use the accent of the buttons beside them, scroll bars stay visible on text backgrounds, and high-contrast buttons with a fill always draw their border.
- Reduce Transparency uses the opaque neutral palette.
- Choosing the theme that is already set changes nothing.
- Cupertino no longer writes GTK and Qt themes or reads the GTK icon-theme setting.

### Shell and session

- X11 apps run through xwayland-satellite, started on first use and supervised. A quick exit backs off before the next start. Toolkits are asked to prefer Wayland, and logging out removes only the hints Cupertino set.
- A session started from the display manager is supervised, and a crash returns to the login screen. The session sets up its activation environment and its own session target, and restarts back off.
- Starting from a text console checks the hardware first, writes a log file and runs under a watchdog script.
- Explicit synchronization on the text-console backend, with bounded waits.
- After a VT switch, a frame lost during the switch is dropped, and the lock screen counts as shown only once a frame is on screen.
- Each monitor is paced to its own refresh rate and redraws on its own.
- When idle, the compositor sleeps until its next real deadline instead of waking every 4 ms, and child processes wake it only when they exit.
- The CPU renderer resizes a window and opens the window switcher within one frame at 4K.
- A failed frame keeps apps paced. A lost rendering context ends the session with an error that names it.
- A buffer that is already ready is used without a wait or a log line.
- A new window takes the keyboard only by the focus policy, never while you type or while the session is locked. Windows move and resize only from a real press on themselves. A process's right to focus its first window ends when its last connection closes ([#20](https://github.com/orafaelfragoso/cupertino/issues/20), [#44](https://github.com/orafaelfragoso/cupertino/issues/44)).
- An app can bring itself forward only while it has focus or an activation token. Otherwise its dock tile bounces.
- A window whose app stops answering shows the busy cursor, and the dock marks it Not Responding. Force Quit is `Super+Alt+Escape` ([#20](https://github.com/orafaelfragoso/cupertino/issues/20)).
- A window that can't zoom or has a fixed size refuses full screen, the shortcut always leaves full screen, and View > Exit Full Screen works.
- Apps can disable their minimize and zoom buttons. The buttons dim and the matching menu items are greyed out.
- Logging out reaches the login process over the server's private connection. A log out that reaches nobody shows a notice, and Lock is disabled when there is no login process.
- The launcher and the login screen honour Reduce Motion. A failed password keeps its outline until you type again.
- A machine-wide accessibility file, `/etc/cupertino/accessibility.conf`, is read by every session and by the login screen. An administrator at the console can set it through the session manager.
- Animated theme cursors play, and stop when the pointer leaves, hides or changes shape, theme or seat.
- A rotated monitor shows its wallpaper upright, and its screenshots and screen captures come out upright.
- Screenshots are written in one step and never replace an existing file, and a failed write, for example on a full disk, leaves no partial file.
- The network daemon starts without rfkill access and reports the real error. The Bluetooth daemon keeps running on a machine without Bluetooth.
- When every `wayland-N` socket name is taken, the server uses a name of its own.

### Security

- Shell roles are bound to the process the session started, granted only by the root-owned system configuration, and checked on every request, also after the holder has exited.
- Session-bus services check their callers. On a bus older than dbus 1.15 the caller is identified by its process id, and the log says so ([#51](https://github.com/orafaelfragoso/cupertino/issues/51)).
- Sandboxed apps get no clipboard control, output management, layer shell or window list. While the session is locked, clipboard changes are refused.
- The notification centre refuses sandboxed and unnamed callers.
- Locking the screen stops screen captures, and only a process with the capture role may capture.
- The login process clears the memory it frees. The login and lock screens can't be dumped, and a login screen killed in the middle of an attempt ends that attempt.
- A lock request reaches the lock screen even when another process holds the login bus name.
- Force Quit acts only on the process the kernel named when it connected.
- The server checks that each announced menu belongs to the window's app, and only the verified menu bar may read or use an app's menus.
- About and Settings hand a launch only to their own program, and claim their bus name again if its owner left.
- Size limits before data is used: a keymap larger than its file is refused, text from apps for the dock, window manager and menu bar is cut to 1024 bytes, menu announcements are capped, and cursor theme files and glyph images are checked before anything is allocated.
- At most 64 backdrops per surface and 128 per app ([#48](https://github.com/orafaelfragoso/cupertino/issues/48)).

### CupertinoKit

- CupertinoKit, the toolkit for apps, and everything it depends on are MPL-2.0, and a check keeps it so. The desktop stays GPL-3.0-or-later.
- `cupertino-wayland-client` talks to the compositor over standard Wayland protocols only, with text input, presentation feedback, popups that can move, and theme cursors when a compositor has no cursor-shape support.
- Cupertino's own shell processes and apps run on that client.
- The toolkit has a runtime, a Wayland window system, two-pass layout (stacks, grids, scrolling, right-to-left, virtual lists), a CPU renderer, text shaping and paragraph layout, and theme tokens with light, dark, neutral and high-contrast palettes.
- Controls: Toggle, Button, TextField, SecureField and Text. The Counter and Forms examples run on Cupertino and on a second compositor.
- Hidden and sensitive text stays protected: secure text buffers and input hints are kept out of logs and recordings.
- Apps start in the session's light or dark appearance and follow it, and a switch redraws every part of the window.
- The text caret blinks and stops after 10 seconds idle, and double and triple click select text.
- A held key press is matched by the physical key, and leaving a window cancels it.
- Clipboard reads don't block the app: each waits up to a second for more data and at most 10 seconds in all.
- Drag and drop (the transport only so far).
- Screen readers see the app's accessibility tree, including live regions that announce changes. Password text never leaves the app.
- Standard menus, with a menu model that apps export to the menu bar.
- Colour roles for scroll bars, dialog backdrops, links and page dots, and surface materials.
- A window can leave out its plain background, so a menu or popover can paint its own shape.
- A headless test window system that records and replays input, with protected text left out.

### Packaging and release

- The Adwaita icon theme is a required dependency.
- The default asset pack uses the built-in cursors.
- Package licence fields and the shipped `THIRD-PARTY-LICENSES` cover every linked licence.
- Debian packages ship stripped binaries.
- An upgrade test installs one version and then the next for deb and rpm.
- All three formats recommend xwayland-satellite and Xwayland. Debian does not package xwayland-satellite.

### Tests and CI

- Golden image tests of headless scenes in a pinned renderer image, with input replay. The login screen, launcher, power panel and overview have goldens, the CPU renderer has its own, and `ci/goldens.sh --update` checks each test's result.
- A registry of every Wayland global, D-Bus interface and shipped binary, checked at run time and in CI.
- Crate manifests declare each crate's layer, and one check enforces the layer rules.
- CI also runs the tests in the package build environment ([#41](https://github.com/orafaelfragoso/cupertino/issues/41)).
- Every shipped binary is started in the tests, and the server starts headless in CI.
- A test that needs EGL or D-Bus fails instead of skipping when the backend is missing.
- Tests use private D-Bus daemons and never change the process environment.
- A render timing matrix covers the window switcher and window resizing.
- Timing-sensitive tests count work instead of measuring time.
- A debug-only test clock for the time shown in the login screen, dock and launcher.
- The vendored xkbcommon-rs tests build again.

### Documentation

- The contributor agreement and how to accept it.
- X11 support and its limits.
- The local changes to the vendored xkbcommon-rs.

## v0.1.0-rc4 — 2026-10-27

Tagged on `da978f9`. This entry covers the changes since `v0.1.0-rc3`.

### Menus

- A system menu at the left end of the menu bar holds the session actions, and app shortcuts are drawn as symbols ([#19](https://github.com/orafaelfragoso/cupertino/issues/19)).

### Files

- Files search uses the shared text field. While a search runs the field is wider, and an empty result shows No Results ([#39](https://github.com/orafaelfragoso/cupertino/issues/39)).
- The launcher copies through the app clipboard.

### Shell and session

- Sleep and a power dialog with Restart, Sleep, Cancel and Shut Down. The log-out shortcut asks first. While the login process restarts after a crash, the shortcut ends the server without asking ([#19](https://github.com/orafaelfragoso/cupertino/issues/19)).
- Live reload reads `/etc/cupertino/compositor.json` under the user's own file ([#33](https://github.com/orafaelfragoso/cupertino/issues/33)).

### Tests and CI

- The Settings popup tests pass in the package build.
- `ci/replace.py` fails when a pattern matches nothing, and writes every file or none.

### Documentation

- The release-candidate hardware checklist covers suspend, the lid, monitor hotplug and the wallpaper.

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
- The lock agent is started in the installed and TTY sessions.

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

- Public README, CONTRIBUTING (CLA process; the CLA text link is still to be set), SECURITY, Code of Conduct, and a docs book covering architecture, building and running, asset packs and trying Cupertino.
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
sudo pacman -U cupertino-0*.pkg.tar.zst cupertino-data-*.pkg.tar.zst
```

Both packages are replaced together. `/etc/cupertino/compositor.json` is a package config file, and files under `~/.config/cupertino/` are read as before.
