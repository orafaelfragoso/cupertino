# Building and running

## Requirements

- A current stable Rust toolchain (`rustup` or your distribution's package).
- OpenGL ES drivers (Mesa) and a Wayland session to run nested in. A TTY session needs logind and a DRM/KMS device.
- A Wayland terminal such as `foot` to start inside the session.
- Some crates link system libraries (for example the session manager links libpam); each crate's README lists what it needs.

Build output is large. Keep several GB free for the target directory.

## Try it in one minute (nested)

Run the compositor as a window inside your current Wayland session:

```bash
git clone https://github.com/orafaelfragoso/cupertino.git
cd cupertino
cargo run --release -p cupertino-server -- --winit --spawn foot
```

From a checkout, the apps' desktop entries (`crates/*/data/applications`) are not installed, so the menu bar and the dock show
"Cupertino-files" and a generic icon instead of "Files" and its icon. Add the crates' data directories to the search path
(keep the host's directories after them):

```bash
export XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
```

Installed packages put the entries in `/usr/share/applications`; nothing to set there.

The compositor prints `WAYLAND_DISPLAY=<socket>` once it listens; start more clients with that variable set. `Super+Return` starts a terminal (the first existing of `foot`, `alacritty`, `kitty`, `wezterm`, `xterm`; configurable with `terminals`). `Ctrl+Alt+Backspace` quits.

Without a pack of your own, text, icons and cursors come from the built-in fallbacks (see [Asset packs](asset-packs.md)); a missing asset never stops a start.

## The whole desktop on a TTY

From a text console (switch with `Ctrl+Alt+F3`), a checkout can start the full desktop — compositor, dock, menu bar, launcher, notifications, control panel, OSD, window manager, Files and Settings:

```bash
./run-cupertino.sh
```

The script refuses to run inside a desktop session (`WAYLAND_DISPLAY` or `DISPLAY` set). It builds the workspace's release binaries (`cargo build --release --workspace --all-features --bins`), writes a shell process list to `$XDG_RUNTIME_DIR/cupertino-compositor.json`, starts a fresh session bus with `dbus-run-session`, and runs `cupertino-server --udev` with Files and Settings. It changes nothing on the system: the session manager and the login screen are not used. `XDG_RUNTIME_DIR` must be set, which a console login does.

- `Ctrl+Alt+Backspace` quits.
- `Ctrl+Alt+F<n>` returns to the console session you came from.
- The log is `~/cupertino-tty.log`; when the server exits with a non-zero status the script prints the last 20 lines.
- Server flags pass through: `./run-cupertino.sh --run-for 600` stops by itself after ten minutes.

The TTY backend has run in a virtual machine, not yet on real hardware. Treat it as experimental.

## Headless captures

`crates/cupertino-server/scripts/capture.sh` renders the full desktop once and writes a PNG:

```bash
crates/cupertino-server/scripts/capture.sh out.png --spawn cupertino-files
AFTER=6 crates/cupertino-server/scripts/capture.sh switcher.png --spawn cupertino-files --spawn cupertino-settings --hold-modifiers --key 5:Super+Tab
```

The script uses the release binaries in `target/release`, or `target/debug` when the release build is missing (`BIN_DIR` overrides both), with a temporary sample home folder and cleared environment, so a capture never reads your files and looks the same on every machine. `AFTER` sets the seconds before the shot (default 5), `SIZE` the output size (default `1280x800`), and `dbus-run-session` is required. One asset pack is used: `CUPERTINO_ASSET_DIR`, else the user pack under `$XDG_DATA_HOME` (or `~/.local/share`), else the built-in assets; the system pack is not searched. The script prints the path it wrote.

## The shell processes

The dock, menu bar, launcher, notifications and the other shell parts are separate processes. Each builds its binary behind an `app` feature:

```bash
cargo build --release -p cupertino-dock --features app
cargo build --release -p cupertino-menubar --features app
```

The server starts and restarts the processes listed under the `"shell"` key of `~/.config/cupertino/compositor.json`, for example `[{"name": "dock", "command": ["cupertino-dock"]}]` (the list is empty by default). A process that exits with failure is restarted at most once per second.

## Backends

| Flag | Backend |
|---|---|
| `--winit` | nested window in a Wayland or X session (development) |
| `--udev` | TTY: logind, DRM/KMS and evdev |
| `--headless` | no display; use with `--screenshot out.png --exit-after-screenshot` to render one frame |

Without a flag the server runs nested when `WAYLAND_DISPLAY` or `DISPLAY` is set and on the TTY otherwise. Other useful flags: `--size WxH`, `--scale F`, `--appearance light|dark`, `--spawn "cmd args"` (repeatable), `--run-for S`. `cargo run -p cupertino-server -- --help` lists them all, and the server's README describes each.

The TTY backend has run in a virtual machine, not yet on real hardware. Treat it as experimental.

## Tests

```bash
cargo nextest run --workspace --all-features
cargo test --workspace --all-features --doc
cargo clippy --workspace --all-targets
cargo run -p cupertino-layers
```

Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`; CI uses 0.9.149). It runs every test in its own process and kills a test that runs longer than 2 minutes (`.config/nextest.toml`), so a hang fails with the test's name; doctests run separately. Shell and app crates need `--features app` to test their process code (`cargo nextest run -p cupertino-dock --features app`). A killed run can leave a test `dbus-daemon` behind: stop it, never your real session bus.

## Configuration

Plain files under `~/.config/cupertino/`, each with an environment override: `shortcuts.conf` (chord to action lines), `compositor.json`, `keyboard.conf`, `dock.conf`, `menubar.conf`. The server's README lists every file and key.
