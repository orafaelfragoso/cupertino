# Roadmap

Assessed on 2026-11-04 against merged commit `0a928c70`. Phases 1–3 have recorded completion audits. Phases 4–6 remain incomplete. Phase 7 has not started. Hardware acceptance is a separate gate and remains open.

This assessment compares the phase criteria, merged source, tests, packaging and issue checklists. Existing test results belong to their recorded commits. Reading a test here does not mean it ran again, and code coverage does not replace a hardware run.

## Phase 1: baseline and trust

Completed for the agreed scope, WP-00 and WP-01. The contracts registry, layer graph, headless goldens and input replay exist. Supervised shell roles and session-bus mutation checks have tests. The dock and lock golden regressions found after the phase audit were fixed before the recorded green CI run on November 2.

Source: `docs/architecture/contracts.yaml`, `ci/contracts.py`, `crates/cupertino-layers`, `crates/cupertino-server/tests/shell_roles.rs` and `ci/goldens.sh`.

Remaining security and coverage debt stays visible in [#28](https://github.com/orafaelfragoso/cupertino/issues/28) and [#53](https://github.com/orafaelfragoso/cupertino/issues/53). Completion of the baseline does not establish isolation from every same-user application.

## Phase 2: transport and scene

Completed for the agreed scope, WP-02, WP-03 and the GLES half of WP-04. The standard Wayland client, neutral scene model and render API are merged. Layer rules keep the public toolkit separate from the private shell protocols. Settings and Files gained damage submission.

Source: `crates/cupertino-wayland-client`, `crates/cupertino-scene`, `crates/cupertino-render-api` and the server's scene tests. The old client adapter is gone.

Frame scheduling, presentation tracking, scene serialization, dirty-region propagation and asynchronous resource preparation moved to Phase 6. Vulkan and comparisons between renderer backends remain deferred. Damage submission alone does not prove that Files meets its frame-time target ([#40](https://github.com/orafaelfragoso/cupertino/issues/40)).

## Phase 3: CupertinoKit core

Completed for WP-06 and the CPU half of WP-07. Runtime, layout, text editing, controls, CPU rendering, themes and AT-SPI are merged. Counter, Forms, Virtual List and Editor have headless event-trace goldens and compatibility tests on Cupertino and a second compositor. The virtual-list tests bound work and memory for 50,000 rows.

Source: `crates/cupertino-kit/tests`, `crates/cupertino-ui-test`, `crates/cupertino-server/tests/standard-clients.rs` and `crates/cupertino-wayland-client/tests/mutter.rs`. See [Get started with CupertinoKit](kit-getting-started.md).

GPU rendering and per-node GPU resource retirement belong to the deferred renderer work. External CJK input methods remain outside the agreed core scope ([#24](https://github.com/orafaelfragoso/cupertino/issues/24)). Core accessibility does not complete the app migration checks in Phase 4.

## Phase 4: apps and portals

In progress, WP-08, WP-09 and WP-10.

Implemented:

- Settings uses Kit, including About; its old painter and window modules were deleted in [#92](https://github.com/orafaelfragoso/cupertino/pull/92).
- OSD, Control Center, launcher, menu bar and notifications draw through Kit. Dock, login and lock, screenshot UI and the window-manager UI still need migration. [What runs on CupertinoKit](apps-on-kit.md) tracks the actual entry points.
- The parallel Kit Files window has list and icon views, file operations, new windows and menus. The installed binary still starts `app::run` in `crates/cupertino-files/src/main.rs`.
- Portals provide Settings, FileChooser, ScreenCast, AppChooser and Screenshot. App icons, activation tokens and screenshot retry are merged. Kit menus now share one session-bus connection per app.
- [#93](https://github.com/orafaelfragoso/cupertino/pull/93) adds app and portal screen-reader checks. These are automated checks, not a complete assistive-technology acceptance run.

Still required:

- Finish Files parity, remove its old window and direct drawing dependencies, and resolve the file-operation correctness findings before changing the default ([#46](https://github.com/orafaelfragoso/cupertino/issues/46)).
- Migrate the remaining shell clients and fix the reviewed menu-bar, Control Center and notification interaction gaps.
- Complete portal colour picking, remembered permissions, rotated captures and requesting-app identification in screencast consent. Verify browser file choosing and screen sharing in an installed session.
- Complete the application accessibility audit, semantic-tree hardening, phase screenshots and phase-end review.

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
- Confirm offline master-key custody, distribute the public release key, and record a complete install, reboot, login and recovery run. No hosted package repository is published.
- Implement `cupertino system update check|plan|apply|rollback` with snapshots, receipts, matched kernel images and interrupted-update recovery. The current pacman/ext4 path does not satisfy this criterion.
- Complete installer encryption, ISO reproducibility, SBOM and signed-manifest work, then the phase-end review.

NetworkManager and BlueZ are the shipped ISO and installed-system backends. Cupertino's network and Bluetooth daemons remain available for development; they do not establish replacement parity. See [Architecture](architecture.md), [Displays and GPUs](displays-and-gpus.md) and [Signed updates and rollback](updates-and-rollback.md).

## Daily-driver acceptance

Open. None of the ten recorded hardware acceptance criteria has complete evidence. VM checks cover parts of login, graphics, keyboard layouts, X11 and service integration. They do not close the hardware gate.

Required evidence includes login with VT and suspend cycles, an eight-hour application soak, NVIDIA presentation, GPU-reset recovery, browser portals, audio device switching, mixed-scale Wayland and X11 clipboard use, keyboard layouts, network and Bluetooth, and five working days without a compositor crash or session loss. Run the [release checklist](release-checklist.md) against one exact candidate and record blocked checks explicitly.

## Phase 7: AI and voice

Not started, WP-17, WP-18 and WP-19. The planned scope is a per-user AI service, local and user-supplied providers, context actions, local voice and toolkit APIs. There is no shipped implementation to mark complete.

## Deferred work

The Vulkan half of WP-04 and the Verse adapter remain deferred. X11 applications already run through `xwayland-satellite`; XWayland is part of daily-driver acceptance, not a deferred feature. External CJK input remains outside the current scope. Translation and locale support remain open in [#66](https://github.com/orafaelfragoso/cupertino/issues/66).
