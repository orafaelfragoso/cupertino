# cupertino-settings

The settings application of the Cupertino desktop. The library holds the pane table, the sidebar order, the navigation
history, the search, the panes and their controls; the `cupertino-settings` process (feature `app`) draws one window with
CupertinoKit (`shell`) in the full-height-sidebar style (Kit's `SidebarWindow`): the sidebar, with the window control
buttons at its top and below them a search field (the large control size, 28 pt, a light capsule on the sidebar's
material) over rows of coloured tiles, beside the page under a floating toolbar
of back and forward buttons. A page's title shares the toolbar row, after the buttons; only a pane that gathers
sub-pages (Accessibility, Privacy & Security) opens instead with a header card (its tile, title and a one-line
description) and no title in the toolbar. Its sections are rounded groups of full-width rows; some sections are pages of
their own, opened from a row with a chevron and gone back from with the toolbar's history, their title in the toolbar
after the buttons. The page fills the pane on the window background (its groups keep their own fill) and scrolls under
the toolbar row, starting below it: what passes under the row shows through its glass, veiled by the scroll edge
(Kit's `NavigationStack` with `under_toolbar`). The top of the
sidebar and the toolbar are the window's drag region: a drag where no control lies moves the window and a double click
there zooms it, while the buttons and the search field keep their presses. From the keyboard, the compositor's Zoom shortcut (Super+Ctrl+Z by default) and the menu bar's Window menu
zoom, minimize and tile the window.

```
cargo test -p cupertino-settings --features app        # needs CUPERTINO_ASSET_DIR; bus tests need dbus-daemon
cargo run -p cupertino-settings --features app -- [--pane PANE-ID]
cargo clippy -p cupertino-settings --all-targets --all-features
```

One process per session: it owns `org.cupertino.Settings1` (`/org/cupertino/Settings1`, `ShowPane(s pane_id)`, an
unknown id is `InvalidArgs`) on the session bus; a second start calls `ShowPane` with its pane and exits 0 (§8).

Pane ids: `wifi`, `bluetooth`, `network`, `battery`, `accessibility`, `appearance`, `control-panel`, `desktop-dock`,
`displays`, `notifications`, `sound`, `focus`, `lock-screen`, `keyboard`, `mouse`, `trackpad`. Without `--pane` the window opens on the first
sidebar row (approximate). The desktop entry is `data/applications/cupertino-settings.desktop` (`Name=Settings`).

| Module | Contents |
|---|---|
| `pane` | The panes of the minimum set (§3): title, `symbol.* and enclosure colour (Battery's colour approximate), how they apply, and the authored section titles and index words of the search |
| `sidebar` | The groups in the design order, the sortable group by the localized standard compare (collation subset of approximate) |
| `history` | `HistoryManager`: array and index, push drops the forward part, no cap |
| `search` | Word-prefix matching, case- and width-insensitive with diacritics significant; ranking exact title > title prefix > section > word |
| `controls` | What each control holds (switch, segmented, pop-up, slider with or without stops, level slider, buttons, text, picture grid, preview) and the value it commits; sliders commit on release (§7.2), a level slider writes at most every 16.7 ms while it moves |
| `mapping` | Tracking stops, Dock size, magnification (from just above the tile size to 128 px, kept in place when the size changes), key repeat rate and delay positions with the walk |
| `settings` | `dock.conf`, `compositor.json`, `menubar.conf`, `keyboard.conf` and `power.conf` through `cupertino-defaults` (atomic writes that keep comments, pins and unknown keys), `accessibility.conf` read only (the server writes it) over the machine's `/etc/cupertino/accessibility.conf`, key by key, `data-control.conf` (the server reads it; a refused file is never written over), `screen-sharing.conf` (the portal writes and reads it; Settings changes it only by forgetting one grant under the file's lock, never by writing its list back; a refused file is never written over), `notifications.conf` (the notification centre reads it and follows its changes), the values of unset keys |
| `screen_sharing` | Screen Sharing in the Privacy pane: each display an app may share again without asking (`screen-sharing.conf`), named and drawn by the app's desktop entry, with Forget (`Effect::ForgetGrant`, `Settings::forget_grant`), which reads the file again under its lock (`ScreenSharingConf::update`) and takes out only that grant, so one the portal added since the pane opened stays |
| `clipboard` | Clipboard Access: `data-control.conf` as the Privacy pane shows it (entries, the lines that allow nothing, a refused file), each entry's state from the server's own test `Entry::allows_file`, the installed applications with the canonical program their `Exec` runs (past `env`, through `PATH`), and why one cannot be allowed (sandboxed, runs scripts, Wayland proxy, already allowed, can't be read) |
| `panes` | Each pane's sections and rows, the key or call behind every control (ST-09), and what a committed value writes or calls. The Appearance pane: Appearance (Light, Dark) and Show scroll bars (Automatically, When scrolling, Always: `show_scroll_bars` of `compositor.json`; automatically keeps them shown while a mouse is connected). The Trackpad pane's Pointer acceleration switch writes `trackpad_acceleration` (off: a constant gain by the tracking speed), as the Mouse pane's does `mouse_acceleration`. Accessibility's Pointer page has Pointer outline color and Pointer fill color pop-ups (white around black by default, eight named colours; a colour of the file that is none of them is listed after them as `#rrggbbaa`, chosen, and choosing it changes nothing), writing `cursor_outline` and `cursor_fill`. The Accessibility pane's Display section ends with a Steady text cursor switch (after the colour filter rows, so they keep their places), which shows `steady_caret` as the server holds it and sends `SetAccessibility("steady_caret", "on"|"off")`. The Accessibility pane's Screen Reader section (after Display and Login Screen, so the rows above it do not move whether or not a reader is installed): the Screen reader switch shows `screen_reader` as the server holds it and sends `SetAccessibility("screen_reader", "on"|"off")`, so the server starts or stops the session's reader; its second line names Super+Alt+S, and without an installed reader (`Paths::screen_reader`, `cupertino_defaults::accessibility::installed_screen_reader`; Settings never starts it) the switch is disabled with "No screen reader is installed." The Login Screen section (after Display) shows only when `CanSetSystemAccessibility` said yes at this opening; its Use Mine sends the allow-listed keys (`SYSTEM_KEYS`) with the values the Display rows show, its Login screen now row reads `/etc/cupertino/accessibility.conf` with the readers' rules (Default settings without a file or with a refused one), and its footer says that people who changed an accessibility setting keep their own. The Notifications pane (`notifications.conf`; layout Original): Lock Screen with Show on lock screen (on/off) and Show previews (Always, When Unlocked, Never) for every application, then Application Notifications, a section per application with the same two choices as pop-ups whose first item, Default, removes the application's key so it follows the settings above. The applications listed (`notification_apps`, at most 64, by name) are those the file holds a choice of and those whose notifications the centre holds now (`NotificationCenter1.Applications`, read every refresh while the pane shows); one that never sent a notification is not listed until it does. An application is named by its desktop entry, else by the name the centre gave. The Displays pane: a section per display with Desktop Size first, a pop-up of the display's sizes (`cupertino_displays::catalogue` for its mode, physical size and connector) from bigger text to more room, labelled with their points as the display shows them (sides swapped on a display turned by 90 or 270 degrees), the default marked Default and the ends Bigger and More Room; a size that is none of them is listed after them, chosen, and choosing it changes nothing; a display without sizes (its physical size unknown, or a built-in panel already at the smallest size) has no such row. A choice sends `SetDisplayScale` with its scale for the display's connector (`output_config`); a display that names no connector is refused |
| `live` | The readings of the live services the panes show: unknown before the first read, or the reason they are unavailable; `display_name` cleans every name a radio or device supplies (control and bidirectional characters dropped, 128 characters at most) where the services read it |
| `sheet` | The Wi-Fi passphrase, the Bluetooth Forget and Clipboard Access Remove confirmations, the pairing prompts, the folder, picture and program choosers and the application chooser of Clipboard Access, whose Return never allows (wording and metrics approximate) |
| `services` (feature `app`) | `Network1`, `Bluetooth1`, `Power1`, UPower and `LoginManager1` (`CanSetSystemAccessibility` at each opening of the Accessibility pane, `SetSystemAccessibility` on a thread of its own; a refusal is shown by its name, never sessiond's text) on the system bus, `NotificationCenter1` on the session bus (Do Not Disturb and its durations, and `Applications` for the Notifications pane), the default PipeWire sink through `cupertino-pipewire` (`wpctl`), else the mixer through `cupertino-alsa`; read every refresh, never cached; a missing name gives the pane's reason; `detect` picks the Cupertino daemon, else NetworkManager or BlueZ, each refresh, and the panes none of them backs (`Backends::hidden`) leave the sidebar and the search |
| `output_config` (feature `app`) | Setting a display's scale over `zwlr_output_management_unstable_v1`: one short connection reads the heads, applies a configuration that names every head as it is and changes only the scale of the display asked for, and waits for `succeeded`, `failed` or `cancelled` |
| `agent` (feature `app`) | The pairing agent `org.cupertino.Bluetooth1.Agent` at `/org/cupertino/Settings1/Agent`, registered while the Bluetooth pane is shown; each question waits for its sheet; a call from anyone but the current owner of `org.cupertino.Bluetooth1` is `Rejected` |
| `nm` (feature `app`) | The Wi-Fi and Network panes on NetworkManager when `org.cupertino.Network1` is not on the bus: devices, access points (one row per SSID, the strongest signal; an SSID is keyed by its bytes, `nm::ssid_key`), saved connections; Join activates a saved connection, else adds one (`wpa-psk` or `sae`) |
| `bluez` (feature `app`) | The Bluetooth pane on BlueZ when `org.cupertino.Bluetooth1` is not on the bus: the first adapter and its devices (the id is the object path; a call to any other path is refused), a paired device is trusted; the agent as `org.bluez.Agent1` (`KeyboardDisplay`, the default agent), answering only the owner of `org.bluez` |
| `instance` (feature `app`) | `org.cupertino.Settings1`: claim the name, or hand the pane to the owner; any session client may call `ShowPane` (the launchers of §8 are several processes), and it only switches the pane |
| `pages` | The pages of the window: each pane's, and the sub-pages a pane opens from a navigation row (Accessibility's Display, Login Screen, Screen Reader, Pointer and Keyboard; Desktop & Dock's Hot Corners; Privacy & Security's Clipboard Access and Screen Sharing). A sub-page holds the pane's section of its title and the headerless sections after it; the pane's page shows a row for each sub-page where the first of them stood. The panes with a header card (`has_header_card`) and each one's one-line description for it; the page a search result opens |
| `shell` (feature `app`) | The window on CupertinoKit: default size `(ceil(739 f), 625)`, at least 400 high, the sidebar 232 wide at the start, its search field filtering its rows while a query is typed (the page lists the results; choosing one ends the search); the page's sections as Kit grouped-form controls (switches at the toggle's small size, `Toggle::small`), a label repeated across sections (an application's pop-ups on the Notifications pane) named with its section's header for assistive technology, (a row's label and second line shrink to wrap when the row is too narrow for them and its control) in a scroll view whose scroll bar stays shown while the pane is longer than the window, so a cut-off pane says so (unless the session's scroll bar style says otherwise), its sheets as dialogs (a chooser's list longer than nine rows scrolls, so the sheet fits the window at its least height), its menus on the desktop's menu bar; the icons it requires (`shell::required_icons`, the panes' symbols and the chrome's), each missing one named once at start |

## Open issues

- Every pane has content. Every file the panes write is applied at once by its process, so no pane shows the
  restart footer. The live panes are read again every second (approximate) and the Wi-Fi pane scans on open and every
  10 s (approximate); property change signals are not subscribed to, so a change by another client shows within a
  second.
- Displays sets only the size a display looks like: modes, refresh rate, rotation and arrangement are shown but not
  set, and the choice applies at once, without a confirmation that reverts it.
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
- Symbols: every tile (the sidebar's, a header card's, a sub-page row's) wears a filled symbol (`symbol.wifi.fill`,
  `symbol.trackpad.fill` and so on), so the process requires each pane's `.fill` name, `symbol.moon.fill` and
  `symbol.xmark.circle` (gap G8). The design's `appearance` and
  `dots.below.lock.fill` are not in the system glyph catalogs; `circle.lefthalf.filled` and `lock.display` stand in for
  them (approximate).
- A click outside the results popup on another client or the desktop ends the search (the field clears) and reaches what it hit; a click in this window keeps the popup's grab and reaches the window. Whether the design needs a second click is unknown.
- Choosing a search result opens the page that holds its section but does not yet scroll to it or flash it (§5).
- Decision: on a sub-page, clicking the selected pane's sidebar row goes back to that pane's main page, pushed on the
  history (Back returns to the sub-page); on the main page itself the row does nothing and the page keeps its scroll
  position. Whether the design does the same is not known.
- Decision: two applications of one name on the Notifications pane keep their headers as they are; their pop-ups'
  accessible names (the label and the header) are numbered in the order they show, which for one name is by
  application id, as "Show previews, Mail (1)" and "Show previews, Mail (2)": the numbers do not change when one of
  them gets a choice of its own or the notification centre lists them in another order.

## Tests

| Test | Proves |
|---|---|
| unit tests in `src/` | ST-01 (size), ST-03 (order, sortable group, mock full set), ST-05 (history), ST-06 (chords, through the Kit shell headless), ST-07 (matching, ranking, every section findable), ST-20 (a pack without the required symbols still starts: `shell::required_icons` names them), the `--pane` argument |
| `src/shell/tests/layout.rs` | Section layout, headless at 1, 1.25, 1.5 and 2x: a footnote wider than its section (long translated-style text) wraps to as many lines as it needs at 320, 500 and 900 pt, stays inside the section's width, starts a footer gap below its group, and the next section's title starts a section gap and its own space above below the footnote's last line; a narrower section gives it more lines. A row whose label and second line are wider than its section (long translated-style text) wraps them and keeps its switch inside the section, a row gap after the text, with the next row below every line. The Mouse pane with two mice from a fake session on a private bus: the resolution footnote is one line in a 1200 pt window and two at the default and the least width (where the sidebar folds away), and no text or control is cut off at the window's edge or overlaps another. With `CUPERTINO_LOOK_DIR` set, they write captures (`settings-long-footnote-<width>`, `settings-long-row-<width>`, `settings-mouse-resolution`). The Notifications pane with two applications from a fake notification centre on a private bus: every header, label and pop-up stays inside the window and none overlaps another at every width and scale (capture `settings-notifications`), and each application's pop-ups read a name of their own with the application's name; two applications of one name get pop-up names numbered apart (`shell::form` tests) |
| `src/shell/mod.rs` tests | The Kit window headless: navigation, search, menus and commits; the Screen reader switch, clicked twice against a fake session object on a private bus (also the window's own session connection, never the real session), calls `SetAccessibility("screen_reader", "on")` then `"off"` and shows what the session then holds; every pane leaves nothing a screen reader cannot announce; a navigation row opens its sub-page and Back and Forward go through the history; the selected pane's sidebar row goes back from a sub-page to the pane and does nothing on the pane itself; the Desktop & Dock rows' switches are 36 by 16 and their rows keep their height; a typed search keeps in the sidebar only the rows of the panes it finds, in order, and clicking one of them opens that pane and ends the search |
| `tests/start-process.rs` | ST-20 for the binary: without any asset pack, or with a pack without `symbol.trackpad`, it reports what is missing and goes on to the display it needs |
| `tests/files.rs` | ST-15, ST-16 from the panes: a `dock.conf` write keeps pins, comments, corner and gesture lines and unknown keys; a `compositor.json` write keeps the key bindings, the shell list and unknown keys; a write that cannot be made is an error and leaves the file; the Notifications pane writes the default and each application's choice into `notifications.conf`, keeps comments and unknown keys, and Default removes the application's key |
| unit tests of `mapping`, `panes`, `controls` | The Login Screen section (present only when allowed, "Saving…" ignoring presses until the reply, the state row and "Default settings" without a file, the one-line footer), ST-14, ST-14a, ST-17 (the restart footer), ST-18 for the hot corners and the sleep timers (a value outside the list shows as an extra item and is kept), the level slider's write throttle, the sheets |
| `tests/clipboard.rs` | Clipboard Access: rows named by their application or file, Remove only after confirmation, a system program stored by path and a user's one pinned at its canonical path, Changed and Allow Again, Not found, the chooser's reasons with the arrows skipping disabled rows and Return allowing nothing, a script refused, a line that allows nothing removed alone, a refused file left byte for byte |
| `tests/services.rs` | Against fake daemons on a private bus: the Login Screen section offered only when sessiond says yes (not on no, a failure or no sessiond), Use Mine sending only the allow-listed keys with the person's values and each refusal getting its sentence (any other failure the plain unavailable text); ST-10 (the Wi-Fi switch writes `WirelessEnabled`, the shown value is the daemon's, a saved network joins with an empty passphrase once, a refused write keeps the daemon's value and gives the error), ST-11 (paired and nearby groups, agent registration, a pairing answered and refused through the agent, Remove only after confirmation, `AgentExists` without a dialog), ST-13 (Focus rows are `FocusLifetimes` in order and call the centre), the Notifications pane lists the applications of the centre's `Applications` (none without a centre), ST-19 (no daemon: the switch row disabled with the reason) |
| `tests/instance.rs` | ST-24: `ShowPane` reaches the owner, an unknown id is `InvalidArgs`, a second owner is refused, and a second start of the binary hands its pane over and exits 0 without a display, but its activation token only to its own program; About has its own owner, takes a launch from its own program only, and a launch refuses to hand its token to an About of another program |
| `tests/about-window.rs` | About on CupertinoKit, headless: the title, app id and fixed size; every reading read in order with a repeated label read once, More Info a button, nothing a screen reader cannot announce; labels end at the centre and no row overlaps the next; the system mark drawn in the label colour over the window background in light and dark at 2x |
| `src/shell/tests/repaint.rs` | What an interaction repaints, from the damage of each presented frame (never its wall time): the Automatically hide switch repaints only itself while its knob slides, the Size slider only itself while it is dragged, and switching panes repaints in the sidebar only the rows the selection left and reached. `repaint_budget_report` (ignored) times them; see Frame time |

## Frame time

`shell::tests::repaint::repaint_budget_report` is `#[ignore]`d because it measures wall time. With the window filling
a 3840×2160 display at scales 2, 1.5 and 1, it times every frame of switching panes, toggling a switch, dragging a
slider, scrolling a pane (at the window's default size, the only size at which a pane is longer than the window), the
caret's blink and typing a search, and gives p50, p95, p99, maximum, the frames over 8 ms and the damaged share of the
window. It uses the application's renderer and node limit and keeps no copy of the frames. Run it in the release
profile:

```sh
cargo test --release -p cupertino-settings --features app --lib -- --ignored --nocapture \
  shell::tests::repaint::repaint_budget_report
```

`CUPERTINO_REPAINT_DIR=DIR` also writes `settings-<scale>x.md` and `settings-<scale>x.csv` (one line per frame) there,
and `CUPERTINO_REPAINT_SCALES=2,1` measures other scales. Under `ci/worktree-build.py`, which keys its target on the
`CUPERTINO_*` variables, build with `--no-run` and run the test binary it names under `python3 ci/worktree-build.py
locked -- env CUPERTINO_REPAINT_DIR=DIR BINARY --ignored --nocapture shell::tests::repaint::repaint_budget_report`.
[What runs on CupertinoKit](../../docs/src/apps-on-kit.md#frame-time) has the last results.
