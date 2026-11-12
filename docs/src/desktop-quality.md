# Desktop quality and open issues

Updated 2026-11-12. This is the installed-session quality backlog for the desktop experience Cupertino targets. These issues block calling the affected interactions finished, even where their implementation or automated tests already exist. See the [roadmap](roadmap.md#desktop-quality-acceptance) for the acceptance gate.

## Evidence and status

The review used the maintainer's recording `2026-11-10 00-31-22.mp4` (3:52.53, 1920×1080 at 60 fps) of an Arch Linux Cupertino session. The installed runtime and compositor reported base commit `0582a36c`; the portal had the screenshare hotfix installed separately. Timestamps below are positions in that recording, not wall-clock times. The recording and extracted screenshots are local review artifacts, not files shipped with this book.

DQ-30 comes from a separate maintainer report and live installed-session inspection on the same date, with runtime base commit `0582a36c`; it is not a finding from the recording.

DQ-31 is a separate maintainer test on the same date: ChatGPT Desktop lacked Cupertino's titlebar and could not be moved by dragging through XWayland. A subsequent native Wayland launch sent a valid move request, and the maintainer confirmed that dragging works. The application explicitly requests client-side decorations. On 2026-11-11 the maintainer decided that such applications keep their own titlebar, which closes DQ-31; the XWayland move failure continues as DQ-33.

DQ-32 comes from a subsequent maintainer report and a live D-Bus trace: selecting Open ChatGPT in the menu-bar status icon sent an obsolete menu item ID. A second ChatGPT icon during the Wayland test belonged to its separate test instance and disappeared when that instance ended; duplicate icons are not established as a separate defect.

The tables below keep the 32 entries as first recorded (DQ-01 to DQ-30, DQ-32 and DQ-33); DQ-31 is closed under [Closed Electron titlebar policy](#closed-electron-titlebar-policy). Their status at `b030dced` is in [Status at b030dced, 2026-11-12](#status-at-b030dced-2026-11-12): 31 remain open, 2 of them fixed in code and awaiting the installed-session check, and DQ-09 is resolved by design. **Observed** means a visible symptom was found in the recording; **reported** means the maintainer reported it and a focused reproduction remains necessary; **follow-up** means source or running-service inspection adds evidence. A cursor shape does not prove a click reached another window. Without physical input timestamps the video cannot establish repeat latency, right-button delivery or submenu latency. It also cannot establish search matching correctness, accessibility compliance or which material shader ran.

**P1** affects task completion, input reliability, readable controls or core desktop behavior. **P2** affects presentation, consistency or discoverability. These priorities are the next-work recommendation, not a claim that other release criteria are satisfied. The original recording is compressed and downscaled relative to its captured 4K source; fine seams and physical cursor size need verification at native scale.

## Status at b030dced, 2026-11-12

Each item was checked again at `b030dced`, 263 commits after the recording's base: in headless sessions at scales 1 to 2 in light and dark, against the focused tests and by reading the source. **Fixed** means the fix is in code and passes headless; it stays open until the installed-session check this page requires. **Partial** means part of the acceptance is met. **Needs installed-session check** means the code and headless checks pass and only the installed session can settle the item. Each open item has a GitHub issue with its remaining acceptance criteria and live-check steps.

| ID | Verdict at `b030dced` | Fixing commits | Issue |
|---|---|---|---|
| DQ-01 | Open. Dragging the divider past half the content's minimum width (Settings 320 pt, Files 316 pt) still collapses the content pane. | none | [#162](https://github.com/orafaelfragoso/cupertino/issues/162) |
| DQ-02 | Fixed, awaiting installed-session check. No column of the desktop shows at 1.25, 1.5 or 1.75 in light or dark; in dark a 1–2 device-pixel divider strip stays untinted. | `2c6d4a03`, `311b70b9`, `14be165a` | [#163](https://github.com/orafaelfragoso/cupertino/issues/163) |
| DQ-03 | Partial. Inside a menu the arrow stays and presses reach no window under it; the resize cursor still shows at a window edge just outside an open menu, and scroll routing is unverified. | `638505af`, `76769bc1`, `46a4898e` | [#164](https://github.com/orafaelfragoso/cupertino/issues/164) |
| DQ-04 | Partial. Menus on desktop items and space, Files items, space and sidebar, Shift+F10 and dock tiles; none on text fields, the file manager's dock tile offers only Open, and the dock's menu is styled differently. | several | [#165](https://github.com/orafaelfragoso/cupertino/issues/165) |
| DQ-05 | Needs installed-session check. The mapping is correct; a stored delay under 250 ms is shown at the Long end. | — | [#166](https://github.com/orafaelfragoso/cupertino/issues/166) |
| DQ-06 | Open. A drag followed by a quick press maximizes server-decorated windows, native and X11; the double-click count ignores movement. Kit toolbars are not affected. | none | [#167](https://github.com/orafaelfragoso/cupertino/issues/167) |
| DQ-07 | Needs installed-session check. The arrow is about 11 × 17 pt at scales 1, 1.5 and 2; the hardware cursor, the size setting and other toolkits are unchecked. | — | [#168](https://github.com/orafaelfragoso/cupertino/issues/168) |
| DQ-08 | Open. One 0.35 s delay is in code and measures 0.32–0.40 s headless, but submenus are still reported slow on the installed session; the cause is under investigation. | `c8266bcd`, `51c98d0e` | [#169](https://github.com/orafaelfragoso/cupertino/issues/169) |
| DQ-09 | Resolved by design. Menus may cover the dock; they stay within the display and flip or slide at its edges. | — | — |
| DQ-10 | Partial. Glass on sidebars, one-panel windows, menus, popovers, the Control Center, Notification Center, the launcher, the dock and the system prompts, with solid fallbacks; no four-mode sweep, and the power dialog and input-source HUD are unchecked. | `1efc5cbc`, `836afbf0`, `f9e86462`, `eb7d50e6`, `14be165a`, `2e277422`, `1a60b3be`, `be1e18b0`, `aaf024e5`, `ae509ae0`, `e90223eb`, `b25e7d7b`, `9c792d46`, `fcd2015e` | [#170](https://github.com/orafaelfragoso/cupertino/issues/170) |
| DQ-11 | Partial. Switches, sliders, segmented controls, glass buttons and popovers animate and respect Reduce Motion; no per-component feedback specification or frame-budget run. | `b419d423`, `7db00843` | [#171](https://github.com/orafaelfragoso/cupertino/issues/171) |
| DQ-12 | Partial. Stack, labels and the dock's menu are glass with solid fallbacks; the settled stack overlaps the bar by about 3 pt and has no pointer. | `1a60b3be`, `aaf024e5`, `e8562e44` | [#172](https://github.com/orafaelfragoso/cupertino/issues/172) |
| DQ-13 | Fixed, awaiting installed-session check. Overlay bars 6 pt at rest and 10 pt hovered or dragged; always-shown bars a 17 pt gutter with an 11 pt thumb, at scales 2 and 1.5. | `63839fc2`, `e0cce0c8`, `53a7858e`, `8b525c29` | [#173](https://github.com/orafaelfragoso/cupertino/issues/173) |
| DQ-14 | Open. Sidebar rows are cut hard 12 pt under Search and slices stay visible. | none | [#174](https://github.com/orafaelfragoso/cupertino/issues/174) |
| DQ-15 | Open. Labels about 1.6–2.2:1 and tracks about 1.1:1 over a bright wallpaper; Increase Contrast and Reduce Transparency are readable. | none | [#175](https://github.com/orafaelfragoso/cupertino/issues/175) |
| DQ-16 | Open. The Control Center reads only Cupertino's network and Bluetooth daemons; the NetworkManager and BlueZ fallbacks exist only in Settings. | none | [#176](https://github.com/orafaelfragoso/cupertino/issues/176) |
| DQ-17 | Partial. Edit Controls opens Settings' Control Panel pane, which switches four menu bar items; the Control Center's controls can't be edited. | none | [#177](https://github.com/orafaelfragoso/cupertino/issues/177) |
| DQ-18 | Open. "Notification Centre" remains in the notifications view and in Settings. | none | [#178](https://github.com/orafaelfragoso/cupertino/issues/178) |
| DQ-19 | Open. The centre has no outside-press catcher. | none | [#179](https://github.com/orafaelfragoso/cupertino/issues/179) |
| DQ-20 | Partial. The dock stays shown with the pointer in the gap; the tile's label and click are lost there. | `bbbd25b3` | [#180](https://github.com/orafaelfragoso/cupertino/issues/180) |
| DQ-21 | Partial. Width, focus, typing, results, No Results and Escape work; no scope indication or loading state, and the title collides. | `b0e1cc37`, `c204746d`, `b0ab3b8f` | [#181](https://github.com/orafaelfragoso/cupertino/issues/181) |
| DQ-22 | Open. The toolbar title wraps; even "Documents" wraps in a small window. | none | [#182](https://github.com/orafaelfragoso/cupertino/issues/182) |
| DQ-23 | Partial. Empties from Files and the dock with a confirmation and Cancel; volume trash folders are never emptied, dock failures are dropped, and the two alerts differ. | `c6344db7`, `e8562e44`, `bbbd25b3`, `b5149aad`, `93e73585`, `b0ab3b8f`, `c4931fc2` | [#183](https://github.com/orafaelfragoso/cupertino/issues/183) |
| DQ-24 | Open. The preview sets no menus and has no parent window. The headless check is blocked by [#160](https://github.com/orafaelfragoso/cupertino/issues/160). | none | [#184](https://github.com/orafaelfragoso/cupertino/issues/184) |
| DQ-25 | Open. No loading state; the fade is computed before the first buffer. Blocked headless by #160. | none | [#185](https://github.com/orafaelfragoso/cupertino/issues/185) |
| DQ-26 | Open. The preview grows from its own centre with no source rectangle. Blocked headless by #160. | none | [#186](https://github.com/orafaelfragoso/cupertino/issues/186) |
| DQ-27 | Open. Applications lists raw desktop entries and a configuration file with generic icons. | none | [#187](https://github.com/orafaelfragoso/cupertino/issues/187) |
| DQ-28 | Open. An unnamed device shows only its address and rows have no connection state. | none | [#188](https://github.com/orafaelfragoso/cupertino/issues/188) |
| DQ-29 | Partial. A grid of thumbnails with captions, a ring and Loading…; the Current row draws no picture and a failed thumbnail looks like one still loading. | `63839fc2`, `a33977af` | [#189](https://github.com/orafaelfragoso/cupertino/issues/189) |
| DQ-30 | Open. The launcher still indexes once at start; an entry written mid-session is not found. | none | [#190](https://github.com/orafaelfragoso/cupertino/issues/190) |
| DQ-32 | Open. Menu layout signals are not followed and no AboutToShow is sent; a replaced menu keeps stale rows and IDs. Removal on exit works. | none | [#191](https://github.com/orafaelfragoso/cupertino/issues/191) |
| DQ-33 | Needs installed-session check. Not reproduced headless: an X11 window drawing its own titlebar and ChatGPT under X11 both move through the satellite. | none | [#192](https://github.com/orafaelfragoso/cupertino/issues/192) |

Totals: 16 open (DQ-08 included), 10 partial, 3 needing the installed-session check, 2 fixed and awaiting that check, and 1 resolved by design (DQ-09). The same checks found two problems in the test tools: the Files preview never maps in a headless session ([#160](https://github.com/orafaelfragoso/cupertino/issues/160)), and the server's scripted `--scroll` moves no view ([#161](https://github.com/orafaelfragoso/cupertino/issues/161)).

## Window management, menus and input

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-01 | P1 | **Sidebar can consume the whole Settings window.** Observed at 00:12–00:17; at 00:14.5 the selected pane is hidden. | Divider dragging preserves a usable minimum content width. Any deliberate collapse has an explicit action and discoverable recovery. Check both Settings and Files at supported window sizes. |
| DQ-02 | P2 | **Sidebar divider leaks the background or shows a contrasting seam.** Observed boundary discontinuity at 00:12–00:17 and 00:41–00:53. The exact rendering cause is unconfirmed. | Sidebar, content and separator cover the whole window without a transparent gap, at integer and fractional scales, over light, dark and moving backgrounds. |
| DQ-03 | P1 | **Menubar dropdown exposes the underlying resize cursor.** Observed at 00:45–00:50, including 00:47.5. Actual button delivery remains unverified. | Open menus own pointer feedback and input. Crossing an underlying window edge does not change to its resize cursor, resize it, scroll it or activate it. Verify menu dismissal and keyboard navigation too. |
| DQ-04 | P1 | **Context-menu coverage and behavior are inconsistent.** Reported across the desktop. Files does have menus at 02:45–02:52; universal absence is not established. | Inventory desktop, file items, empty space, sidebar entries, text fields and dock icons. Provide appropriate secondary-click actions, keyboard access and consistent styling where Cupertino owns the surface; preserve third-party application actions. |
| DQ-05 | P1 | **“Delay until repeat” setting is inverted.** Maintainer clarification: key repeat works; the delay control changes the wait in the opposite direction from its labels. The broader repeat malfunction reported in the initial audit is superseded by this correction. | Moving toward Short decreases the wait before the first repeat; moving toward Long increases it. Verify both endpoints, intermediate positions, persistence and live application. Changing this delay must preserve the separately configured repeat rate. |
| DQ-06 | P1 | **Long-pressing the titlebar sometimes maximizes the app.** Reported intermittent behavior; not independently reproduced. | A single held press or drag does not accidentally count as a double-click. Explicit double-click maximization still works, with correct time, movement, button and release handling across native and XWayland windows. |
| DQ-07 | P1 | **Cursor is too large regardless of scaling.** Reported; the video cannot establish native physical size. | Default cursor size, Settings cursor-size changes, integer/fractional output scaling and movement between differently scaled displays produce consistent intended logical size and correct hotspots. Check both hardware and software cursors and native/XWayland clients. |
| DQ-08 | P2 | **Context-menu submenus open too slowly.** Reported; no precise latency measured. | Define an intentional hover delay, measure it, and avoid cumulative delays through nested menus. Direct movement into a submenu is stable; keyboard opening is immediate and crossing adjacent rows does not cause flicker. |
| DQ-09 | P2 | **Files submenus crowd the bottom work area.** Observed at 02:46–02:51; actual clipping is not established. | Constrain cascading menus to the usable output area, flip their direction when needed and keep every item reachable without colliding with the dock. |

## Materials, motion and scrolling

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-10 | P2 | **UI components do not consistently use the intended glass treatment.** Reported across surfaces; existing material APIs do not prove installed visual fidelity. | Inventory intended material-bearing components in Kit and the shell. Apply consistent recipes, outlines, tint and foreground treatment rather than isolated opaque approximations; retain intentional solid content regions. Check active/inactive, light/dark, Increase Contrast and Reduce Transparency. |
| DQ-11 | P2 | **Glass components lack expected interaction animation.** Reported. | Specify and verify hover, press, release, selection, expansion and dismissal feedback for each component. Transitions remain continuous under rapid input, meet frame budgets and respect Reduce Motion. |
| DQ-12 | P2 | **Dock Downloads popover does not match the intended glass material.** Observed conventional dark panel at 01:28–01:30; hover labels around 01:30–01:33 are separate dark capsules. | Match dock popover, menu and tooltip materials to their intended designs with legible text, consistent borders/shadows and correct accessibility alternatives. Verify the actual installed rendering path. |
| DQ-13 | P2 | **Always-visible and automatic scrollbars are excessively thick.** Reported in both settings modes. | Define resting, hovered and dragged thicknesses. Both modes honor the saved policy, retain a usable hit area and correct thumb proportions, and avoid layout jumps, clipping or exaggerated thickness at fractional scale. |
| DQ-14 | P2 | **Scrolled Settings sidebar leaves an item fragment below Search.** Observed around 00:55–01:05. | Separate the fixed header and scroll viewport with intentional clipping/masking and padding; partially scrolled rows do not leak into header space. Keep scrolling and focus discoverable. |

## Control Center, notifications and dock behavior

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-15 | P1 | **Control Center is unreadable over bright backgrounds.** Observed white text/icons/tracks disappearing into yellow wallpaper at 00:33–00:38. | Glass backing/tint and foreground adaptation preserve readable text and distinguishable controls over bright, dark and detailed content, with contrast measurements and accessibility-setting checks. |
| DQ-16 | P1 | **Control Center does not reflect Wi-Fi/Bluetooth state.** Reported; Settings shows connected Wi-Fi at 00:16–00:25 and near 02:30. Follow-up: Settings supports NetworkManager/BlueZ fallbacks, Control Center calls Cupertino's daemons; at review time NetworkManager/BlueZ were present and the Cupertino network/Bluetooth bus names were absent. Precise panel Bluetooth text is not fully legible. | Share backend discovery and state semantics with Settings. Verify actual enabled, disabled, disconnected, connected, unavailable and error states, live updates and actions against the shipped NetworkManager/BlueZ backends as well as development backends. |
| DQ-17 | P1 | **Clicking Edit Controls closes Control Center.** Reported; successful editing is not demonstrated. | Enter a visible editing flow; changes persist and can be completed or cancelled. A click must not merely dismiss the panel or activate the surface behind it. |
| DQ-18 | P2 | **Notification UI uses “centre” where “Center” is requested.** Reported spelling/terminology mismatch. This is an English UI convention request, not a claim that British spelling is invalid. | Use “Notification Center” consistently in English user-visible labels and accessibility names. Preserve internal protocol/configuration identifiers unless a separate compatibility change is justified. |
| DQ-19 | P1 | **Notification Center does not dismiss on click-away.** Reported. | An outside click dismisses the panel consistently; define whether the click is consumed. Verify Escape, focus changes, interaction with other shell panels and return of keyboard focus. |
| DQ-20 | P1 | **Dock loses hover/focus in the gap between it and the bottom edge.** Reported. | The approach/reveal region includes the intended bottom margin. Pointer travel through the gap does not dismiss or hide the dock; moving genuinely away does. Check auto-hide, magnification, popovers and multiple display arrangements. |

## Files and preview

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-21 | P1 | **Files search has a crowded, faint toolbar field.** Observed at 02:04–02:09. No completed query/result sequence establishes a matching/indexing failure. | Opening search reserves usable width and obvious focus, avoids collisions, accepts keyboard input, updates results and explains scope, loading, no results and clearing. Check long folder names and small windows. |
| DQ-22 | P1 | **Long folder title wraps into toolbar controls and the first file row.** Observed at 01:55–02:09. | Keep a stable toolbar height; truncate long titles with their full path available separately. Title, controls, search and content never overlap during resizing or search animation. |
| DQ-23 | P1 | **Trash cannot be emptied.** Reported; no destructive reproduction was performed during the audit. | Empty Trash is discoverable from the intended Files/dock surfaces, requires the intended confirmation, removes selected trashed content and updates counts/state. Cover empty Trash, errors, cancellation and mounted-volume trash without touching unrelated files. |
| DQ-24 | P1 | **Quick preview changes the menubar options unexpectedly.** Reported. | Preview retains the intended relationship to the source Files window, exposes only intentional preview actions and restores the source menubar/focus on dismissal. Avoid transient activation of unrelated menus. |
| DQ-25 | P1 | **Quick preview flashes the file.** Reported; loading, activation and repaint causes remain unverified. | Opening, replacing and closing preview do not flash the original file/window or stale content. Show deliberate loading feedback when necessary; stress rapid selection and slow preview providers. |
| DQ-26 | P2 | **Quick preview opens away from the previewed file.** Reported. | Define and implement placement/animation relative to the selected item, constrained to the correct output's usable area. Verify scrolled lists, grids, columns, multiple displays and dismissal back to the source. |
| DQ-27 | P2 | **Applications destination exposes raw launcher/configuration filenames.** Observed at 02:19–02:20. | A user-facing Applications destination presents resolved names and icons and launches the right app; alternatively label a raw launcher directory explicitly. Do not mix incidental configuration files into an application browser. |

## Settings presentation

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-28 | P2 | **Bluetooth devices are difficult to identify.** Observed address-only Nearby Devices list around 02:34. Unnamed peripherals may legitimately have no friendly name. | Resolve names when available, label discovery/pairing/connection states and provide clear actions and errors. Use an understandable fallback for anonymous devices. |
| DQ-29 | P2 | **Wallpaper chooser lacks useful visual previews.** Observed text-heavy current/choice rows at 00:29–00:53. | Provide a current-image preview and selectable thumbnails, retain accessible names and show loading/errors for missing or unreadable images. |

## Launcher application discovery

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-30 | P1 | **Applications installed during a session remain absent from the launcher.** Reported after installing `chatgpt-desktop`; follow-up confirmed a valid, visible desktop entry and startup-only application indexing. | Search and the application browser reflect desktop-entry additions, changes, atomic replacements and removals in the active XDG directories without a launcher restart or logout. Preserve visibility rules, user-over-system precedence, the active query and responsive input. Add regression coverage and verify package install/update/removal in an installed session. |

To reproduce, start Cupertino and its launcher, install an application that provides a desktop entry, then reopen launcher search and the application browser and search for it. The reported case used `chatgpt-desktop` `26.1103.60933-1` on Arch Linux in the installed TTY session, at 3840×2160 with scale 2. The package already supplied `/usr/share/applications/chatgpt.desktop`, `/usr/bin/chatgpt` and `/usr/share/pixmaps/chatgpt.png`; no user override hid the entry. `desktop-file-validate` found no errors, and GLib resolved it as visible.

Source inspection at `0582a36c` found that `crates/cupertino-launcher/src/app.rs` constructs the model with `Launcher::from_system(config)` at startup. `crates/cupertino-launcher/src/engine.rs` indexes the XDG application directories there; `set_entries` has no production caller, and no runtime catalogue watcher or refresh is wired into the launcher.

Workaround performed: terminate only the affected session's `cupertino-launcher` with SIGTERM and let the compositor supervisor restart it, forcing a fresh scan. The replacement process was verified in the server log; the compositor and open applications stayed running. With multiple sessions, identify the launcher belonging to the affected compositor before restarting it. This workaround does not close DQ-30.

## Electron window dragging through XWayland

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-33 | P1 | **ChatGPT cannot be dragged through XWayland.** Reported and tested by the maintainer on 2026-11-10, originally as part of DQ-31. Native Wayland dragging subsequently passed in a separate profile. Other Electron applications remain unverified. | Fix dragging through XWayland for windows that draw their own titlebar. Preserve working native Wayland movement, client controls and application drag regions. Verify both backends separately, including release, maximize/restore and output scaling. |

To reproduce, launch ChatGPT Desktop under its X11 backend in the installed Cupertino session, then press and drag in its own top bar. The maintainer could not move the window. Expected: the drag region moves the window until release.

The reported case used `chatgpt-desktop` `26.1103.60933-1` with installed Cupertino `0.1.0-1`, runtime base commit `0582a36c`, and a 3840×2160 display at scale 2. Process inspection identified the original run's X11 backend through XWayland. A separate profile launched with `--ozone-platform=wayland` created a native Wayland toplevel and sent `xdg_toplevel.move` with the serial of a still-held left-button press. The maintainer confirmed that this test window moves when its top bar is dragged. This verifies a workaround for ChatGPT dragging; it does not establish compatibility for all Electron applications or diagnose the XWayland failure.

For future launches, the installed `/usr/bin/chatgpt` wrapper reads `~/.config/chatgpt-flags.conf`, with one flag per line. It does not read `~/.config/electron-flags.conf`. The verified backend flag is now saved in the app-specific file:

```text
--ozone-platform=wayland
```

Quit the existing ChatGPT process and reopen the application for the flag to take effect. An already running process retains its backend. This workaround does not require disabling the sandbox or GPU acceleration.

## Menu-bar status-item actions

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-32 | P1 | **ChatGPT's menu-bar status menu shows actions that do nothing.** The maintainer could neither reopen the window nor quit from the menu. A live trace captured Cupertino sending `clicked` for item 3 while the application's current menu revision 18 used 137 for Open ChatGPT and 139 for Quit. | Status menus follow layout/property changes and refresh before opening when needed. Displayed rows and dispatched actions use the same current menu IDs and exporter. Verify Open and Quit after application state changes, including menu replacement, and verify cleanup when an instance exits. |

To reproduce, start ChatGPT Desktop, use it until its exported status menu changes, then open its menu-bar icon's menu and choose Open ChatGPT or Quit. The reported menu remained visible but its actions had no effect. The captured Open event used an ID absent from the current layout, explaining that failure. Quit was reported by the maintainer; it was not invoked during inspection because it would terminate the active application.

In `crates/cupertino-menubar/src/worker.rs`, `follow_status_item` reloads the menu when the status item's icon, title, status or tooltip changes. `sni::item_changes` does not follow the menu's own `LayoutUpdated` or `ItemsPropertiesUpdated` signals. `crates/cupertino-menubar/src/routing.rs` also ignores `AboutToShow` for status menus. Consequently, a DBusMenu replacement can leave the visible menu and its activation IDs stale even while the status icon remains valid. This path uses D-Bus and is separate from the X11/Wayland backend selection in DQ-33. No permanent fix has been applied.

## Closed Electron titlebar policy

**DQ-31 — Electron windows use their own titlebars: closed by decision on 2026-11-11.** A window whose client asks for client-side decorations keeps its own titlebar and window controls. Cupertino draws its titlebar and controls only for windows that ask for server-side decorations or make no request. It does not force its titlebar onto such a window or lay its controls over the client's content.

These applications turn off the native frame (Electron's `titleBarStyle: "hidden"` with `titleBarOverlay`), draw their own window controls and lay out their header around them. A forced titlebar would stack two titlebars, and overlaid controls would duplicate the client's controls or cover its content, both of which the acceptance excluded. Electron's Linux build gives the compositor no description of that layout, so Cupertino cannot place its controls to fit it. Applications that want Cupertino's controls over their content request the unified titlebar style of `cupertino_app_effects_v1`.

Evidence: the native Wayland trace of ChatGPT Desktop shows `zxdg_toplevel_decoration_v1.set_mode(1)` (client-side) answered by `configure(1)`. Claude Desktop `5.1231.2` (Electron 45), running as a native Wayland client, creates its main window with `titleBarStyle: "hidden"` and `titleBarOverlay: true` and shows its own titlebar in the same way. `XdgDecorationHandler::request_mode` in `crates/cupertino-server/src/handlers.rs` honors the request, and the server test `clients_that_ask_for_client_side_decorations_keep_them` in `crates/cupertino-server/tests/headless.rs` covers it. The XWayland dragging failure from the original report remains open as DQ-33.

## Closed screen-capture regression

**CAP-01 — OBS black capture after selecting a source: fixed in `fe28a392`.** PipeWire rejected buffer allocation with `Invalid argument`: Cupertino offered `MemFd` as a fixed integer while OBS's shared-memory fallback offered a mask of supported types. The producer now offers a flags choice so negotiation intersects the supported bits. The test consumer offers an OBS-style `MemPtr | MemFd | DmaBuf` mask.

Validation recorded before merge: the consumer failed to stream with the old producer and passed with the fix; eight unit tests, the stream and resize integration tests, Clippy, formatting and layer checks passed. Only `cupertino-portal` was rebuilt and installed for the local trial, then restarted. The supplied recording contains desktop frames after that replacement. This closes the observed OBS black-frame regression, not all browser/screen-share, audio, output-transform, performance or packaging acceptance checks. The local binary replacement is not a new versioned Arch package and can be overwritten by a package update.

## Completion gate

Implement the P1 fixes before broad visual polish, while keeping related rendering and interaction work together. Each item needs a focused regression test where practical and an installed-session reproduction of its acceptance checks on an exact candidate commit. Use representative light/dark backgrounds, integer/fractional scaling, native and XWayland applications and the relevant accessibility settings. Compare material and animation behavior against an explicitly chosen reference; neither existing glass code nor this recording alone establishes the intended desktop fidelity.

Keep this list open until evidence is recorded per ID. Resolving CAP-01 does not resolve the other entries or complete the daily-driver gate. No additional must-have feature inventory beyond the recording findings and the maintainer's reports has been inferred.
