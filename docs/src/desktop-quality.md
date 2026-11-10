# Desktop quality and open issues

Updated 2026-11-10. This is the installed-session quality backlog for the desktop experience Cupertino targets. These issues block calling the affected interactions finished, even where their implementation or automated tests already exist. See the [roadmap](roadmap.md#desktop-quality-acceptance) for the acceptance gate.

## Evidence and status

The review used the maintainer's recording `2026-11-10 00-31-22.mp4` (3:52.53, 1920×1080 at 60 fps) of an Arch Linux Cupertino session. The installed runtime and compositor reported base commit `0582a36c`; the portal had the screenshare hotfix installed separately. Timestamps below are positions in that recording, not wall-clock times. The recording and extracted screenshots are local review artifacts, not files shipped with this book.

DQ-30 comes from a separate maintainer report and live installed-session inspection on the same date, with runtime base commit `0582a36c`; it is not a finding from the recording.

DQ-31 is a separate maintainer test on the same date: ChatGPT Desktop lacked Cupertino's titlebar and could not be moved by dragging. Follow-up process inspection identified the application's X11 backend; the decoration and move-request causes remain unconfirmed.

All 31 entries below are **open**. **Observed** means a visible symptom was found in the recording; **reported** means the maintainer reported it and a focused reproduction remains necessary; **follow-up** means source or running-service inspection adds evidence. A cursor shape does not prove a click reached another window. Without physical input timestamps the video cannot establish repeat latency, right-button delivery or submenu latency. It also cannot establish search matching correctness, accessibility compliance or which material shader ran.

**P1** affects task completion, input reliability, readable controls or core desktop behavior. **P2** affects presentation, consistency or discoverability. These priorities are the next-work recommendation, not a claim that other release criteria are satisfied. The original recording is compressed and downscaled relative to its captured 4K source; fine seams and physical cursor size need verification at native scale.

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

## Electron window titlebars and dragging

| ID | Priority | Issue and evidence | Required acceptance |
|---|---|---|---|
| DQ-31 | P1 | **Electron applications lack Cupertino's titlebar and cannot be moved by dragging.** Reported and tested by the maintainer with ChatGPT Desktop on 2026-11-10. Follow-up found `--ozone-platform=x11` in its running processes, identifying the XWayland path. Other Electron applications and native Wayland remain unverified. | Affected Electron windows show the intended Cupertino titlebar and window controls and move continuously when dragged by the titlebar. Releasing the pointer ends the move without triggering window controls or dragging application content. Verify ChatGPT Desktop in an installed session and exercise XWayland and native Wayland separately, including maximize/restore and output scaling. |

To reproduce, launch ChatGPT Desktop in the installed Cupertino session, inspect the top of its normal window, then press and drag where the titlebar should allow movement. The maintainer found no Cupertino titlebar and could not move the window by dragging. Expected: the window has Cupertino's titlebar and a usable drag region that moves it until release.

The reported case used `chatgpt-desktop` `26.1103.60933-1` with installed Cupertino `0.1.0-1`, runtime base commit `0582a36c`, and a 3840×2160 display at scale 2. Process inspection confirms that this run uses the X11 backend through XWayland; it does not establish native Wayland behavior or the root cause. No workaround has been verified. Keep DQ-31 open until the reported application passes an installed-session titlebar and dragging check at an exact candidate commit.

## Closed screen-capture regression

**CAP-01 — OBS black capture after selecting a source: fixed in `fe28a392`.** PipeWire rejected buffer allocation with `Invalid argument`: Cupertino offered `MemFd` as a fixed integer while OBS's shared-memory fallback offered a mask of supported types. The producer now offers a flags choice so negotiation intersects the supported bits. The test consumer offers an OBS-style `MemPtr | MemFd | DmaBuf` mask.

Validation recorded before merge: the consumer failed to stream with the old producer and passed with the fix; eight unit tests, the stream and resize integration tests, Clippy, formatting and layer checks passed. Only `cupertino-portal` was rebuilt and installed for the local trial, then restarted. The supplied recording contains desktop frames after that replacement. This closes the observed OBS black-frame regression, not all browser/screen-share, audio, output-transform, performance or packaging acceptance checks. The local binary replacement is not a new versioned Arch package and can be overwritten by a package update.

## Completion gate

Implement the P1 fixes before broad visual polish, while keeping related rendering and interaction work together. Each item needs a focused regression test where practical and an installed-session reproduction of its acceptance checks on an exact candidate commit. Use representative light/dark backgrounds, integer/fractional scaling, native and XWayland applications and the relevant accessibility settings. Compare material and animation behavior against an explicitly chosen reference; neither existing glass code nor this recording alone establishes the intended desktop fidelity.

Keep this list open until evidence is recorded per ID. Resolving CAP-01 does not resolve the other entries or complete the daily-driver gate. No additional must-have feature inventory beyond the recording findings and the maintainer's reports has been inferred.
