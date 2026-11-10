# Roadmap

Phase assessment recorded on 2026-11-09 against merged commit `5a4f015f`; desktop quality reassessed on 2026-11-10 from an installed-session recording and maintainer reports. Phases 1–3 have recorded completion audits. Phases 4–6 remain incomplete. Phase 7 has not started. Hardware acceptance is a separate gate and remains open. The [desktop quality backlog](desktop-quality.md) records 29 open issues and the closed OBS capture regression; implementation coverage below is not installed-session acceptance.

This assessment compares the phase criteria, merged source, tests, packaging and issue checklists. Existing test results belong to their recorded commits. Reading a test here does not mean it ran again, and code coverage does not replace a hardware run.

## Phase 1: baseline and trust

Completed for the agreed scope, WP-00 and WP-01. The contracts registry, layer graph, headless goldens and input replay exist. Supervised shell roles and session-bus mutation checks have tests. The dock and lock golden regressions found after the phase audit were fixed before the recorded green CI run on November 2.

Source: `docs/architecture/contracts.yaml`, `ci/contracts.py`, `crates/cupertino-layers`, `crates/cupertino-server/tests/shell_roles.rs` and `ci/goldens.sh`.

Since the audit, every test that starts a Cupertino binary gives it an empty asset search path, so the tests draw with the built-in assets even where a pack is installed; `crates/cupertino-server/tests/asset-isolation.rs` checks the test sources for spawns that do not. Server tests wait for the events they check rather than for a fixed time, with a 60-second guard against a hang. The CI build, test and golden jobs run in pinned Arch containers, and the golden baselines are recorded on that image's renderer.

Remaining security and coverage debt stays visible in [#28](https://github.com/orafaelfragoso/cupertino/issues/28) and [#53](https://github.com/orafaelfragoso/cupertino/issues/53). Completion of the baseline does not establish isolation from every same-user application.

## Phase 2: transport and scene

Completed for the agreed scope, WP-02, WP-03 and the GLES half of WP-04. The standard Wayland client, neutral scene model and render API are merged. Layer rules keep the public toolkit separate from the private shell protocols. Settings and Files gained damage submission.

Source: `crates/cupertino-wayland-client`, `crates/cupertino-scene`, `crates/cupertino-render-api` and the server's scene tests. The old client adapter is gone.

Since the audit:

- Every surface the server shows (windows, subsurfaces, popups, layer, lock, X11, cursor, drag icons and input-method popups) is told which outputs it is on through `wl_surface.enter` and `leave`. `wl_compositor` version 6 sends each surface its preferred buffer scale and transform, beside the exact fractional scale.
- A window across two displays takes the scale of the display that holds most of it, and changes only when another holds clearly more, so it does not switch back and forth at the seam.
- Displays keep a scale model with size choices, check a whole arrangement before applying it and roll it back if it fails, recover when a display goes and restore its windows when it returns. See [Displays and GPUs](displays-and-gpus.md).
- A window's toolbar row can be a synchronized subsurface (app effects version 11), so content scrolls behind the toolbar glass with a soft edge under it. Reduce Transparency gives the row a solid fill.
- Click catchers and scrims are single-pixel buffers the compositor stretches, so they cost no full-output upload.

Frame scheduling, presentation tracking, scene serialization, dirty-region propagation and asynchronous resource preparation moved to Phase 6. Vulkan and comparisons between renderer backends remain deferred. Damage submission alone does not prove that Files meets its frame-time target ([#40](https://github.com/orafaelfragoso/cupertino/issues/40)).

## Phase 3: CupertinoKit core

Completed for WP-06 and the CPU half of WP-07. Runtime, layout, text editing, controls, CPU rendering, themes and AT-SPI are merged. Counter, Forms, Virtual List and Editor have headless event-trace goldens and compatibility tests on Cupertino and a second compositor. The virtual-list tests bound work and memory for 50,000 rows.

Source: `crates/cupertino-kit/tests`, `crates/cupertino-ui-test`, `crates/cupertino-server/tests/standard-clients.rs` and `crates/cupertino-wayland-client/tests/mutter.rs`. See [Get started with CupertinoKit](kit-getting-started.md).

Since the audit, Kit gained:

- pointer and scrolling input: axis sources, acceleration profiles, kinetic scrolling timed from each event's own timestamp, and overlay scroll bars;
- text input: inline composition, grapheme-safe input-method edits, multi-click and drag selection, a caret drawn as an overlay on device pixels, and a steady caret option;
- one table of control sizes, a search field capsule for fields that sit on a material, and glass buttons;
- menus and popups that open, track and close under touch;
- a CPU renderer whose incremental frames are pixel-identical to full frames, checked by a property test.

GPU rendering and per-node GPU resource retirement belong to the deferred renderer work. External CJK input methods remain out of scope ([#24](https://github.com/orafaelfragoso/cupertino/issues/24)). Core accessibility does not complete the app migration checks in Phase 4.

## Phase 4: apps and portals

In progress, WP-08, WP-09 and WP-10. Updated on 2026-11-10. The Kit migration and feature implementations listed below exist, but the installed session has functional and presentation gaps. The implementation is not complete for daily use: the [desktop quality backlog](desktop-quality.md) and the installed-session and hardware checks below remain acceptance requirements.

Implemented:

- Every Cupertino app and shell process draws through Kit except the authentication prompt. Settings (its old painter and window modules were deleted in [#92](https://github.com/orafaelfragoso/cupertino/pull/92)), OSD, Control Center, launcher, menu bar and notifications were already on it; the Dock ([#109](https://github.com/orafaelfragoso/cupertino/issues/109)), the application switcher and the overview's chrome ([#112](https://github.com/orafaelfragoso/cupertino/issues/112)), login and lock ([#110](https://github.com/orafaelfragoso/cupertino/issues/110)) and the screenshot tool ([#111](https://github.com/orafaelfragoso/cupertino/issues/111)) now are too. The authentication prompt (`cupertino-polkit-agent`) and the portal's colour-picker overlay still draw without Kit; the screen-sharing item is a status item that the menu bar draws with Kit. Kit menus share one session-bus connection per app. [What runs on CupertinoKit](apps-on-kit.md) tracks the entry points.
- Files runs on Kit and the old window is deleted ([#46](https://github.com/orafaelfragoso/cupertino/issues/46)). Removable media mount, unlock and eject from the sidebar; the encrypted-device and policy checks on hardware remain ([#65](https://github.com/orafaelfragoso/cupertino/issues/65)). Search widens its field while it runs and shows No Results when nothing matches ([#39](https://github.com/orafaelfragoso/cupertino/issues/39)). Drops onto folders, places, the Trash and the open folder, drags out with an image of the items ([#68](https://github.com/orafaelfragoso/cupertino/issues/68)), file-type icons, thumbnails, the toolbar's View, Tags and Action pop-ups, the rubber band, disclosure triangles, click-to-rename, a clickable path bar and transfers off the window's thread with progress and Stop are in ([#108](https://github.com/orafaelfragoso/cupertino/issues/108)).
- The overview lays out windows below its desktops strip without overlap, with room for each title ([#67](https://github.com/orafaelfragoso/cupertino/issues/67)). Menu bar and Control Center interaction gaps are fixed ([#113](https://github.com/orafaelfragoso/cupertino/issues/113), [#114](https://github.com/orafaelfragoso/cupertino/issues/114)), and notification cards use materials, wrap their bodies and scroll in the centre ([#115](https://github.com/orafaelfragoso/cupertino/issues/115)).
- Portals provide Settings, FileChooser, ScreenCast, AppChooser and Screenshot, with app icons, activation tokens, screenshot retry, colour picking, remembered screen shares (Forget in Settings, file locking between the portal and Settings), upright captures of rotated displays and the requesting app named in screen-sharing consent ([#116](https://github.com/orafaelfragoso/cupertino/issues/116)). The keyring unlocks at login and at the lock screen ([#117](https://github.com/orafaelfragoso/cupertino/issues/117)), the authentication prompt names the requesting app, and log out, restart and shut down can reopen windows at the next login ([#37](https://github.com/orafaelfragoso/cupertino/issues/37)).
- Accessibility audit ([#30](https://github.com/orafaelfragoso/cupertino/issues/30)), building on the screen-reader checks of [#93](https://github.com/orafaelfragoso/cupertino/pull/93): every Kit surface, Files included, is checked by a shared audit on every settled frame in the tests, against a stand-in reader that follows the AT-SPI bridge's rules. These are automated checks; speech and braille with a real reader are checked by hand. Settings has a Screen reader switch and `Super+Alt+S` toggles the reader; long labels and footnotes wrap ([#35](https://github.com/orafaelfragoso/cupertino/issues/35)).
- Frame time at 4K ([#40](https://github.com/orafaelfragoso/cupertino/issues/40)): Files and Settings repaint only what changed. Selection, hover, the caret, the search field and renaming in Files, and switches and sliders in Settings, meet 8 ms at 3840×2160 at scales 2 and 1.5, and so does scrolling a Settings pane at its default window size (a long pane fits a 4K window unscrolled); a Settings pane switch or search and closing the Files search field miss on one or two frames, and scale 1 misses more often. Scrolling the Files list, opening a large folder and the search keystroke that fills the list take 57–76 ms, as they repaint most of the window. Meeting 8 ms there needs a scroll blit or the GPU composition planned for Phase 6, pending maintainer acceptance. [Frame time](apps-on-kit.md#frame-time) has the measurements.
- Look and materials: windows, menus, the Dock, the menu bar, toolbars and the Control Center draw on glass materials with their Reduce Transparency and Increase Contrast recipes, and text on glass follows the tone of what is behind it. Windows have a hairline outline and an inner rim, inactive windows use their own recipes, and the light window background is white. The Control Center is a grid of separate glass tiles with detail views. The launcher sits on a dimmer material for large panels, and the halo behind each menu bar item follows how unevenly light the wallpaper is under it (shell protocol version 21). The apps, folders, documents and the Trash use the project's own icon pack.
- Launcher: one panel for search and the application browser, which the Dock opens. Its providers and previews can be cancelled, choosing a running app brings it forward, its status announcements wait until the results settle, and its icons load off the event loop. Its frame budgets are measured in CPU time.
- Files has a column view, a preview window, tabs and tags, and its context menu groups the view modes in a Show As submenu. Files and Settings scroll their content under the toolbar row, Settings panes on the window background.
- The window buttons the server draws, the revealed title bar of a full-screen window included, are exposed to assistive technology through AccessKit, with presses accepted only while a button is shown, enabled and unlocked.
- A popup grab ends whenever the keyboard moves outside it: to a layer surface such as the launcher or the Control Center, the lock screen, the authentication prompt or another window.

The earlier menu/Control Center fixes and glass implementations listed above do not close the newly observed or reported failures: dropdown cursor ownership, Control Center contrast and service compatibility, inconsistent materials and interaction motion, sidebar bounds and Files toolbar/search layout remain open. Notifications, dock behavior, Trash, preview, cursor sizing and input timing also need the checks recorded below.

Still required:

- Resolve and verify the [29 desktop quality issues](desktop-quality.md): window/input/menu behavior (DQ-01–09), materials/motion/scrolling (DQ-10–14), Control Center/notifications/dock behavior (DQ-15–20), Files/Trash/preview (DQ-21–27), and Settings presentation (DQ-28–29).
- Preserve the OBS buffer-negotiation fix in `17c33c42`. The locally rebuilt portal produced the review recording after the fix; browser sharing and the remaining output, performance and package checks are still required.

- Verify in an installed session and on hardware:
  - browser file choosing and screen sharing through the portals;
  - Orca speech and braille, and Orca and Accerciser on the window buttons the server draws;
  - USB and LUKS media ([#65](https://github.com/orafaelfragoso/cupertino/issues/65));
  - PAM logins and the keyring;
  - suspend and console (VT) switches;
  - display hotplug and a rotated monitor, including unplugging and replugging with windows, full-screen windows and the lock screen up;
  - two displays at 60 Hz and 120 Hz, with the overview staying smooth;
  - the pointer staying smooth on DRM while an app is stopped (`kill -STOP`);
  - the lock screen on two displays, with nothing unlocked showing during a plug or unplug;
  - the output events `xwayland-satellite` receives, read with `WAYLAND_DEBUG`;
  - the `pkexec` authentication scrim as a single-pixel buffer, read with `WAYLAND_DEBUG`;
  - menus and popups under a real touch screen;
  - a window's toolbar scale across two displays of different scales;
  - the launcher's frame budget in a release build on an idle machine.

  The installed-session check of the portals, polkit and credential services is tracked in [#23](https://github.com/orafaelfragoso/cupertino/issues/23).
- Maintainer decisions on the items the reviews left open, including [#40](https://github.com/orafaelfragoso/cupertino/issues/40)'s measured state.
- The phase-end review.

## Phase 5: command line, diagnostics and SDK

In progress, WP-14, WP-16 and WP-11.

The `cupertino` registry implements `appearance assets check`, human and JSON output, and defined exit codes. `cupertino-diagnose` is a separate shipped binary. The C ABI has a headless runtime, generational handles, panic containment and symbol, header and layout baselines.

Source: `crates/cupertino-cli/src/lib.rs`, `crates/cupertino-diagnostics`, `crates/cupertino-ui-abi` and `packaging/release.py`.

Still required:

- Package `/usr/bin/cupertino` and add `diagnose`, `dev layers`, `desktop start` and the SDK commands to its registry.
- Generate completions and manual pages, support JSONL streams and report component versions.
- Build diagnostic events and their bounded store, redaction and service integration.
- Extend the C ABI to windows, nodes and UI events; run a C sample in CI and package the shared library, header and pkg-config file.
- Complete the phase-end review. See [The cupertino command](cupertino-command.md) for the commands available today.

## Phase 6: hardware and distribution

In progress, WP-05, WP-15 and WP-13.

Implemented in code: per-card DRM output handling and cross-card copies, render-GPU selection, explicit sync when supported, VT resume and GPU-reset recovery. Signed Arch repositories, keyring checks and rollback exist. The live ISO has VM boot evidence. The release workflow signs the ISO checksum and requires the checked-in release-key fingerprint; the local build writes an unsigned checksum.

Source: `crates/cupertino-server/src/backend`, `scripts/build-iso.sh`, `packaging/installer`, `packaging/keyring` and `.github/workflows/release.yml`.

Still required:

- Finish output-presenter scheduling carried from Phase 2, atomic failure diagnostics, scanout reason counters and per-output metrics.
- Run the Intel, AMD, NVIDIA and hybrid-GPU hardware matrix, including multi-monitor suspend, VT switching, capture revocation and GPU reset with clients alive.
- Add touch-screen input to the compositor. `crates/cupertino-server/src/backend/input.rs` opens mice, touchpads and pointing sticks but not touch screens; Kit's menus and popups already handle touch.
- Confirm offline master-key custody, distribute the public release key, and record a complete install, reboot, login and recovery run. No hosted package repository is published.
- Implement `cupertino system update check|plan|apply|rollback` with snapshots, receipts, matched kernel images and interrupted-update recovery. The current pacman/ext4 path does not satisfy this criterion.
- Complete installer encryption, ISO reproducibility, SBOM and signed-manifest work, then the phase-end review.

NetworkManager and BlueZ are the shipped ISO and installed-system backends. Cupertino's network and Bluetooth daemons remain available for development; they do not establish replacement parity. See [Architecture](architecture.md), [Displays and GPUs](displays-and-gpus.md) and [Signed updates and rollback](updates-and-rollback.md).

## Desktop quality acceptance

Open, assessed 2026-11-10. The [complete backlog](desktop-quality.md) distinguishes recording evidence, maintainer reports and follow-up inspection, with priorities and acceptance checks for all 29 items. It includes titlebar long-press maximization, cursor scaling, keyboard repeat, slow submenus, missing context-menu behavior, sidebar bounds and leaks, unreadable or incorrect Control Center state, Edit Controls dismissal, the dock's bottom-edge gap, notification wording/dismissal, Trash emptying, preview activation/flashing/placement, thick scrollbars and material/motion inconsistencies. It also records the additional Files toolbar/title, sidebar clipping, submenu placement, Applications, Bluetooth naming and wallpaper-preview findings.

P1 items are the next functional acceptance work; P2 items remain part of the intended desktop experience. A fix needs focused automated coverage where practical and an installed-session check at an exact candidate commit. Do not close these items from existing feature tests, a headless screenshot or the presence of a glass recipe alone. The OBS black-capture regression CAP-01 is fixed; that does not close this gate.

## Daily-driver acceptance

Open. None of the ten recorded hardware acceptance criteria has complete evidence. VM checks cover parts of login, graphics, keyboard layouts, X11 and service integration. They do not close the hardware gate.

Required evidence includes login with VT and suspend cycles, an eight-hour application soak, NVIDIA presentation, GPU-reset recovery, browser portals, audio device switching, mixed-scale Wayland and X11 clipboard use, keyboard layouts, network and Bluetooth, and five working days without a compositor crash or session loss. The desktop quality acceptance gate above must also pass. Run the [release checklist](release-checklist.md) against one exact candidate and record blocked checks explicitly.

## Phase 7: AI and voice

Not started, WP-17, WP-18 and WP-19. The planned scope is a per-user AI service, local and user-supplied providers, context actions, local voice and toolkit APIs. There is no shipped implementation to mark complete.

## Deferred work

The Vulkan half of WP-04 and the Verse adapter remain deferred. X11 applications already run through `xwayland-satellite`; XWayland is part of daily-driver acceptance, not a deferred feature. External CJK input methods remain out of scope ([#24](https://github.com/orafaelfragoso/cupertino/issues/24)). Translation and locale support remain open in [#66](https://github.com/orafaelfragoso/cupertino/issues/66).
