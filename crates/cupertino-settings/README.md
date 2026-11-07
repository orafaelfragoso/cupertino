# cupertino-settings

The settings application of the Cupertino desktop. The library holds the pane table, the sidebar order, the navigation
history, the search, the panes and their controls; the `cupertino-settings` process (feature `app`) draws one window with
CupertinoKit (`shell`): the sidebar beside the pane under a back and forward toolbar with the search field.

```
cargo test -p cupertino-settings --features app        # needs CUPERTINO_ASSET_DIR; bus tests need dbus-daemon
cargo run -p cupertino-settings --features app -- [--pane PANE-ID]
cargo clippy -p cupertino-settings --all-targets --all-features
```

One process per session: it owns `org.cupertino.Settings1` (`/org/cupertino/Settings1`, `ShowPane(s pane_id)`, an
unknown id is `InvalidArgs`) on the session bus; a second start calls `ShowPane` with its pane and exits 0 (§8).

Pane ids: `wifi`, `bluetooth`, `network`, `battery`, `accessibility`, `appearance`, `control-panel`, `desktop-dock`,
`displays`, `sound`, `focus`, `lock-screen`, `keyboard`, `mouse`, `trackpad`. Without `--pane` the window opens on the first
sidebar row (approximate). The desktop entry is `data/applications/cupertino-settings.desktop` (`Name=Settings`).

| Module | Contents |
|---|---|
| `pane` | The panes of the minimum set (§3): title, `symbol.* and enclosure colour (Battery's colour approximate), how they apply, and the authored section titles and index words of the search |
| `sidebar` | The groups in the design order, the sortable group by the localized standard compare (collation subset of approximate) |
| `history` | `HistoryManager`: array and index, push drops the forward part, no cap |
| `search` | Word-prefix matching, case- and width-insensitive with diacritics significant; ranking exact title > title prefix > section > word |
| `controls` | What each control holds (switch, segmented, pop-up, slider with or without stops, level slider, buttons, text, picture grid, preview) and the value it commits; sliders commit on release (§7.2), a level slider writes at most every 16.7 ms while it moves |
| `mapping` | Tracking stops, Dock size, magnification (from just above the tile size to 128 px, kept in place when the size changes), key repeat rate and delay positions with the walk |
| `settings` | `dock.conf`, `compositor.json`, `menubar.conf`, `keyboard.conf` and `power.conf` through `cupertino-defaults` (atomic writes that keep comments, pins and unknown keys), `accessibility.conf` read only (the server writes it) over the machine's `/etc/cupertino/accessibility.conf`, key by key, `data-control.conf` (the server reads it; a refused file is never written over), the values of unset keys |
| `clipboard` | Clipboard Access: `data-control.conf` as the Privacy pane shows it (entries, the lines that allow nothing, a refused file), each entry's state from the server's own test `Entry::allows_file`, the installed applications with the canonical program their `Exec` runs (past `env`, through `PATH`), and why one cannot be allowed (sandboxed, runs scripts, Wayland proxy, already allowed, can't be read) |
| `panes` | Each pane's sections and rows, the key or call behind every control (ST-09), and what a committed value writes or calls. The Accessibility pane's Screen Reader section (after Display and Login Screen, so the rows above it do not move whether or not a reader is installed): the Screen reader switch shows `screen_reader` as the server holds it and sends `SetAccessibility("screen_reader", "on"|"off")`, so the server starts or stops the session's reader; its second line names Super+Alt+S, and without an installed reader (`Paths::screen_reader`, `cupertino_defaults::accessibility::installed_screen_reader`; Settings never starts it) the switch is disabled with "No screen reader is installed." The Login Screen section (after Display) shows only when `CanSetSystemAccessibility` said yes at this opening; its Use Mine sends the allow-listed keys (`SYSTEM_KEYS`) with the values the Display rows show, its Login screen now row reads `/etc/cupertino/accessibility.conf` with the readers' rules (Default settings without a file or with a refused one), and its footer says that people who changed an accessibility setting keep their own |
| `live` | The readings of the live services the panes show: unknown before the first read, or the reason they are unavailable; `display_name` cleans every name a radio or device supplies (control and bidirectional characters dropped, 128 characters at most) where the services read it |
| `sheet` | The Wi-Fi passphrase, the Bluetooth Forget and Clipboard Access Remove confirmations, the pairing prompts, the folder, picture and program choosers and the application chooser of Clipboard Access, whose Return never allows (wording and metrics approximate) |
| `services` (feature `app`) | `Network1`, `Bluetooth1`, `Power1`, UPower and `LoginManager1` (`CanSetSystemAccessibility` at each opening of the Accessibility pane, `SetSystemAccessibility` on a thread of its own; a refusal is shown by its name, never sessiond's text) on the system bus, `NotificationCenter1` on the session bus, the default PipeWire sink through `cupertino-pipewire` (`wpctl`), else the mixer through `cupertino-alsa`; read every refresh, never cached; a missing name gives the pane's reason; `detect` picks the Cupertino daemon, else NetworkManager or BlueZ, each refresh, and the panes none of them backs (`Backends::hidden`) leave the sidebar and the search |
| `agent` (feature `app`) | The pairing agent `org.cupertino.Bluetooth1.Agent` at `/org/cupertino/Settings1/Agent`, registered while the Bluetooth pane is shown; each question waits for its sheet; a call from anyone but the current owner of `org.cupertino.Bluetooth1` is `Rejected` |
| `nm` (feature `app`) | The Wi-Fi and Network panes on NetworkManager when `org.cupertino.Network1` is not on the bus: devices, access points (one row per SSID, the strongest signal; an SSID is keyed by its bytes, `nm::ssid_key`), saved connections; Join activates a saved connection, else adds one (`wpa-psk` or `sae`) |
| `bluez` (feature `app`) | The Bluetooth pane on BlueZ when `org.cupertino.Bluetooth1` is not on the bus: the first adapter and its devices (the id is the object path; a call to any other path is refused), a paired device is trusted; the agent as `org.bluez.Agent1` (`KeyboardDisplay`, the default agent), answering only the owner of `org.bluez` |
| `instance` (feature `app`) | `org.cupertino.Settings1`: claim the name, or hand the pane to the owner; any session client may call `ShowPane` (the launchers of §8 are several processes), and it only switches the pane |
| `shell` (feature `app`) | The window on CupertinoKit: default size `(ceil(715 f), 625)`, at least 400 high, the sidebar 215 wide at the start; the pane's sections as Kit controls (a row's label and second line shrink to wrap when the row is too narrow for them and its control), its sheets as dialogs, its menus on the desktop's menu bar; the icons it requires (`shell::required_icons`, the panes' symbols and the chrome's), each missing one named once at start |

## Open issues

- Every pane has content. Every file the panes write is applied at once by its process, so no pane shows the
  restart footer. The live panes are read again every second (approximate) and the Wi-Fi pane scans on open and every
  10 s (approximate); property change signals are not subscribed to, so a change by another client shows within a
  second.
- Displays shows the backlight only: `cupertino-wayland-client` has no output-management client, so modes, scale and
  rotation are not built.
- The pane open and close hooks (scan, discovery, agent registration and unregistration) run in the window process and are
  not covered by a test; `tests/services.rs` drives the `Services` calls they make.
- `ShowPane` switches the pane of the running window; `ShowPaneWithToken` also raises it. On a compositor without the
  activation protocol the window and More Info show unraised.
- Bus, watcher and worker threads hand events to the window through a queue of 64. Workers wait for room; a pane request
  or a pairing prompt arriving at a full queue is dropped, and a dropped prompt refuses the pairing.
- Not built yet: keyboard steps of the sliders, the focus ring animation, the overlay scroller, tooltips, undo,
  the Mouse and Trackpad rows' device predicates (both rows always show until the device monitor is wired, §3).
- The width cannot be fixed at the server: `ToplevelConfig` carries a minimum size only, so the window can be widened.
- The interface width factor f is 1 (approximate).
- Symbols: the process requires `symbol.accessibility`, `symbol.appearance`, `symbol.lock-screen`, `symbol.keyboard`,
  `symbol.mouse`, `symbol.trackpad`, `symbol.moon.fill` and `symbol.xmark.circle` (gap G8). The design's `appearance` and
  `dots.below.lock.fill` are not in the system glyph catalogs; `circle.lefthalf.filled` and `lock.display` stand in for
  them (approximate).
- A click outside the results popup on another client or the desktop ends the search (the field clears) and reaches what it hit; a click in this window keeps the popup's grab and reaches the window. Whether the design needs a second click is unknown.
- Choosing a search result selects the pane but does not yet scroll to its section or flash it (§5).

## Tests

| Test | Proves |
|---|---|
| unit tests in `src/` | ST-01 (size), ST-03 (order, sortable group, mock full set), ST-05 (history), ST-06 (chords, through the Kit shell headless), ST-07 (matching, ranking, every section findable), ST-20 (a pack without the required symbols still starts: `shell::required_icons` names them), the `--pane` argument |
| `src/shell/tests/layout.rs` | Section layout, headless at 1, 1.25, 1.5 and 2x: a footnote wider than its section (long translated-style text) wraps to as many lines as it needs at 320, 500 and 900 pt, stays inside the section's width, starts a row gap below the last row, and the next section starts one section gap below its last line; a narrower section gives it more lines. A row whose label and second line are wider than its section (long translated-style text) wraps them and keeps its switch inside the section, a row gap after the text, with the next row below every line. The Mouse pane with two mice from a fake session on a private bus: the resolution footnote is one line in a 1200 pt window and two at the default and the least width (where the sidebar folds away), and no text or control is cut off at the window's edge or overlaps another. With `CUPERTINO_LOOK_DIR` set, they write captures (`settings-long-footnote-<width>`, `settings-long-row-<width>`, `settings-mouse-resolution`) |
| `src/shell/mod.rs` tests | The Kit window headless: navigation, search, menus and commits; the Screen reader switch, clicked twice against a fake session object on a private bus (also the window's own session connection, never the real session), calls `SetAccessibility("screen_reader", "on")` then `"off"` and shows what the session then holds; every pane leaves nothing a screen reader cannot announce |
| `tests/start-process.rs` | ST-20 for the binary: without any asset pack, or with a pack without `symbol.trackpad`, it reports what is missing and goes on to the display it needs |
| `tests/files.rs` | ST-15, ST-16 from the panes: a `dock.conf` write keeps pins, comments, corner and gesture lines and unknown keys; a `compositor.json` write keeps the key bindings, the shell list and unknown keys; a write that cannot be made is an error and leaves the file |
| unit tests of `mapping`, `panes`, `controls` | The Login Screen section (present only when allowed, "Saving…" ignoring presses until the reply, the state row and "Default settings" without a file, the one-line footer), ST-14, ST-14a, ST-17 (the restart footer), ST-18 for the hot corners and the sleep timers (a value outside the list shows as an extra item and is kept), the level slider's write throttle, the sheets |
| `tests/clipboard.rs` | Clipboard Access: rows named by their application or file, Remove only after confirmation, a system program stored by path and a user's one pinned at its canonical path, Changed and Allow Again, Not found, the chooser's reasons with the arrows skipping disabled rows and Return allowing nothing, a script refused, a line that allows nothing removed alone, a refused file left byte for byte |
| `tests/services.rs` | Against fake daemons on a private bus: the Login Screen section offered only when sessiond says yes (not on no, a failure or no sessiond), Use Mine sending only the allow-listed keys with the person's values and each refusal getting its sentence (any other failure the plain unavailable text); ST-10 (the Wi-Fi switch writes `WirelessEnabled`, the shown value is the daemon's, a saved network joins with an empty passphrase once, a refused write keeps the daemon's value and gives the error), ST-11 (paired and nearby groups, agent registration, a pairing answered and refused through the agent, Remove only after confirmation, `AgentExists` without a dialog), ST-13 (Focus rows are `FocusLifetimes` in order and call the centre), ST-19 (no daemon: the switch row disabled with the reason) |
| `tests/instance.rs` | ST-24: `ShowPane` reaches the owner, an unknown id is `InvalidArgs`, a second owner is refused, and a second start of the binary hands its pane over and exits 0 without a display, but its activation token only to its own program; About has its own owner, takes a launch from its own program only, and a launch refuses to hand its token to an About of another program |
| `tests/about-window.rs` | About on CupertinoKit, headless: the title, app id and fixed size; every reading read in order with a repeated label read once, More Info a button, nothing a screen reader cannot announce; labels end at the centre and no row overlaps the next; the system mark drawn in the label colour over the window background in light and dark at 2x |
