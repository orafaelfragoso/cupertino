<p align="center">
  <img src="docs/src/images/cupertino-banner.jpg" alt="Cupertino: an Arch-based Linux distribution with its own Wayland desktop" width="100%">
</p>

<p align="center">
  <a href="https://github.com/orafaelfragoso/cupertino/actions/workflows/ci.yml"><img src="https://github.com/orafaelfragoso/cupertino/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/licence-GPL--3.0--or--later-blue.svg" alt="Licence: GPL-3.0-or-later"></a>
  <a href="https://orafaelfragoso.github.io/cupertino/"><img src="https://img.shields.io/badge/docs-book-informational.svg" alt="Documentation"></a>
  <img src="https://img.shields.io/badge/based%20on-Arch%20Linux-1793d1.svg" alt="Based on Arch Linux">
  <img src="https://img.shields.io/badge/written%20in-Rust-dea584.svg" alt="Written in Rust">
</p>

<p align="center">
  <a href="https://orafaelfragoso.github.io/cupertino/"><strong>Documentation</strong></a> ·
  <a href="docs/src/install-usb.md"><strong>Install from a USB stick</strong></a> ·
  <a href="#try-it-in-one-minute"><strong>Try it</strong></a> ·
  <a href="CHANGELOG.md"><strong>Changelog</strong></a> ·
  <a href="CONTRIBUTING.md"><strong>Contributing</strong></a>
</p>

# Cupertino

Cupertino is an Arch-based Linux distribution with its own Wayland desktop, written in Rust on [Smithay](https://github.com/Smithay/smithay): it installs from a USB stick, and its updates are signed packages that can be rolled back. It gives you a quiet, consistent desktop: translucent materials, soft shadows, spring animations, a dock, a menu bar, a launcher, notifications, a control panel, a file manager, a settings app, a lock screen and a session manager.

**Status: early release (0.x).** It runs nested inside another Wayland session and on a TTY. The TTY backend has run in a virtual machine, not yet on real hardware; treat it as experimental. Interfaces and configuration files can still change between releases. The [changelog](CHANGELOG.md) lists what changed in the current candidate.

## Status

Work is grouped in seven phases. The [roadmap](docs/src/roadmap.md) records the assessment, last updated on 2026-11-12 at commit `b030dced`:

| Phase | State |
|---|---|
| 1. Baseline and trust | Done |
| 2. Transport and scene | Done |
| 3. CupertinoKit core | Done |
| 4. Apps and portals | In progress: every Cupertino app and shell process draws with Kit except the password prompts and the portal's colour picker, and five portal interfaces exist; the [desktop quality backlog](docs/src/desktop-quality.md) and the installed-session and hardware checks remain |
| 5. Command line, diagnostics, SDK | In progress: the `cupertino` command with its command groups, diagnostic events, and a C SDK whose library runs the headless backend; the C interface for windows and UI events remains |
| 6. Hardware and distribution | In progress: ISO, installer, signed packages and rollback exist; hardware acceptance and transactional updates remain open |
| 7. AI and voice | Not started |

The live ISO and TTY backend have VM evidence. The full hardware matrix and installed-system acceptance remain open. [Install from a USB stick](docs/src/install-usb.md) has the steps.

Cupertino serves Wayland clients. X11 programs run through [`xwayland-satellite`](https://github.com/Supreeeme/xwayland-satellite) when it is installed, started at the first X11 program. The live image ships it with Xwayland; the `cupertino` package lists both as optional (`sudo pacman -S xwayland-satellite xorg-xwayland`); without `xwayland-satellite`, X11 programs stay off.

## Try it in one minute

Run it as a window inside your current Wayland session:

```bash
git clone https://github.com/orafaelfragoso/cupertino.git
cd cupertino
cargo run --release -p cupertino-server -- --winit --spawn foot
```

You need a current stable Rust toolchain, OpenGL ES drivers (Mesa) and a terminal such as `foot`. Other terminals work: `Super+Return` starts the first one it finds among `foot`, `alacritty`, `kitty`, `wezterm` and `xterm`. `Ctrl+Alt+Backspace` quits. See [Building and running](docs/src/building.md) for the TTY session and the other options.

Or start the full desktop on a text console (`Ctrl+Alt+F3`) with `./run-cupertino.sh`. It builds the release binaries, starts the compositor and the shell processes with a fresh session bus, and writes `~/cupertino-tty.log`; it installs nothing and enables no service. `Ctrl+Alt+Backspace` quits and `Ctrl+Alt+F<n>` returns to the console you came from. The TTY backend has run in a virtual machine, not yet on real hardware.

Known problems and how to report them: [Trying Cupertino](docs/src/trying.md).

To install a build of your checkout on Arch Linux, `scripts/package-local.sh` builds the `cupertino`, `cupertino-data` and `cupertino-kit` packages of the committed HEAD; install them together with `sudo pacman -U ~/.cache/cupertino-pkg/*.pkg.tar.zst`. See [Building and running](docs/src/building.md#installing-a-local-build).

## Keys

| Keys | Action |
|---|---|
| `Super+Space` | launcher |
| `Super+Tab`, `Super+Shift+Tab` | application switcher (release `Super` to pick) |
| `Super+Return` | terminal |
| `Ctrl+Up`, `F3`, the overview key | overview (the same key closes it) |
| `Ctrl+Left`, `Ctrl+Right`, `Ctrl+1` to `Ctrl+9` | workspaces |
| `Super+Q`, `Super+W`, `Super+M`, `Super+H` | quit application, close window, minimize, hide application |
| `Super+Ctrl+F`, `Super+Ctrl+Left`, `Super+Ctrl+Right` | full screen, tile left, tile right |
| `Super+Shift+3`, `Super+Shift+4`, `Super+Shift+5` | screenshot of the screen, of a selection, toolbar |
| `Super+Ctrl+Q` | lock |

Shortcuts and everything else are configured in plain files under `~/.config/cupertino/`. The server's README lists every file and key.

## What is in the box

Cupertino is a set of small processes around one compositor. A crash in a shell process does not take the desktop down; the compositor restarts it.

| Process | Role |
|---|---|
| `cupertino-server` | compositing, window policy, input, materials, supervision of the shell |
| `cupertino-dock` | dock, application switcher, overview, app grid, workspaces, hot corners |
| `cupertino-menubar` | menu bar, status items, application menus |
| `cupertino-launcher` | search and launch panel |
| `cupertino-notifications` | banners, notification list, `org.freedesktop.Notifications` |
| `cupertino-controlcenter`, `cupertino-osd` | control panel, volume and brightness overlays |
| `cupertino-windowmanager` | stage and tiling policy (off by default) |
| `cupertino-files`, `cupertino-settings`, `cupertino-screenshot` | file manager (and the items on the desktop), settings app, screenshot tool |
| `cupertino-portal` | xdg-desktop-portal backend: file chooser, screen sharing, screenshots, app chooser, appearance settings |
| `cupertino-polkit-agent`, `cupertino-keyring-prompter` | the password prompts of polkit and of the keyring |
| `cupertino-control` | desktop control for local agents, with the person's consent per app, and its MCP bridge |
| `cupertino-login`, `cupertino-sessiond` | lock screen and greeter, root-owned session manager |
| `cupertino-netd`, `cupertino-btd`, `cupertino-powerd` | network, Bluetooth and power daemons; the installed system uses NetworkManager and BlueZ for network and Bluetooth |
| `cupertino`, `cupertino-diagnose` | the command line ([The cupertino command](docs/src/cupertino-command.md)) and the machine check it replaces |

The compositor composites and draws its materials on the GPU with OpenGL ES. CupertinoKit applications and shell processes currently rasterize their windows on the CPU and hand them over in shared memory; drawing them on the GPU, with the CPU only as a fallback, is planned ([#155](https://github.com/orafaelfragoso/cupertino/issues/155)).

The 87 crates are layered (`foundation`, `models`, `platform`, `protocol`, `transport`, `render`, `ui`, `core`, `shell`, `services`, `apps`, `tools`). Model crates are pure logic and are tested without a display. `cargo run -p cupertino-layers` checks that a crate only depends on layers below it. The design is described in [Architecture](docs/src/architecture.md); each crate has its own README.

## Asset packs

Fonts, icons, cursors and wallpapers are not hard-coded. The shell loads them from an **asset pack**: a directory (`CUPERTINO_ASSET_DIR`) with a manifest, and falls back to the default pack for anything the pack does not provide. You can swap the whole look, or one part of it, without rebuilding.

The default pack uses the Inter font, Cupertino's built-in cursors, Cupertino's original icons (`data/assets/cupertino`, `GPL-3.0-or-later`; the host system's freedesktop icon theme covers the rest), and wallpapers picked for this project. The pack format and the steps to build your own are in [Asset packs](docs/src/asset-packs.md). Packs are data: apart from that first-party icon pack, they are not part of the source tree.

## Licence

Cupertino is licensed under the [GNU General Public License, version 3 or (at your option) any later version](LICENSE) (`GPL-3.0-or-later`), except the library crates under the [Mozilla Public License 2.0](https://www.mozilla.org/en-US/MPL/2.0/) (`MPL-2.0`): `cupertino-wayland-client` and the crates it and the toolkit libraries depend on, each with its own `LICENSE`. An MPL-2.0 crate may not depend on a GPL crate; `cargo run -p cupertino-layers` checks it. Contributions are accepted under a contributor licence agreement; see [CONTRIBUTING.md](CONTRIBUTING.md).

Cupertino is an independent project. It is not affiliated with or endorsed by any operating system vendor.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. Report security problems as described in [SECURITY.md](SECURITY.md). Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).
