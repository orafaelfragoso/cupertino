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

The compositor prints `WAYLAND_DISPLAY=<socket>` once it listens; start more clients with that variable set. `Super+Return` starts a terminal (the default terminal through `xdg-terminal-exec` or `$TERMINAL`, else the first existing of `foot`, `alacritty`, `kitty`, `wezterm`, `xterm`; configurable with `terminals`). `Ctrl+Alt+Backspace` quits.

Without a pack of your own, text, icons and cursors come from the built-in fallbacks (see [Asset packs](asset-packs.md)); a missing asset never stops a start.

## The whole desktop on a TTY

From a text console (switch with `Ctrl+Alt+F3`), a checkout can start the full desktop — compositor, dock, menu bar, launcher, notifications, control panel, OSD, window manager, Files and Settings:

```bash
./run-cupertino.sh
```

The script refuses to run inside a desktop session (`WAYLAND_DISPLAY` or `DISPLAY` set). It builds the workspace's release binaries (`cargo build --release --workspace --all-features --bins`), writes a shell process list to `$XDG_RUNTIME_DIR/cupertino-compositor.json`, starts a fresh session bus with `dbus-run-session`, and runs `cupertino-server --udev` with Files and Settings. It installs nothing and enables no service: the session manager and the login screen are not used. `XDG_RUNTIME_DIR` must be set, which a console login does.

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

The script uses the release binaries in `target/release`, or `target/debug` when the release build is missing (`BIN_DIR` overrides both), with a temporary sample home folder and cleared environment, so a capture never reads your files and looks the same on every machine: a stand-in `wpctl` holds the volume at 50 % (no sound device is read), the shell prompt and terminal title name neither user nor machine, and the pointer starts on the bottom-right corner, over no surface, until a `--pointer` moves it. The Files sidebar still names the computer by the machine's host name. `AFTER` sets the seconds before the shot (default 5), `SIZE` the output size (default `1280x800`), and `dbus-run-session` is required. One asset pack is used: `CUPERTINO_ASSET_DIR`, else the user pack under `$XDG_DATA_HOME` (or `~/.local/share`), else the built-in assets; the system pack is not searched. The script prints the path it wrote.

## Surface captures

`ci/capture.sh` draws one surface of Files or the toolkit with the app's own painter on sample data, without a display, a session bus or the server, for look checks and merge screenshots. `overview/all` runs the server headless (`session.sh`) with test windows and builds the release binaries first unless `BIN_DIR` names them:

```bash
ci/capture.sh files/sidebar --drop-place Desktop --increase-contrast --sheet
ci/capture.sh kit                # lists the surfaces of an app
ci/capture.sh overview/all --windows 3 --scale 2
```

It writes one PNG per state and appearance into `captures` under `$CARGO_TARGET_DIR`, else under the workspace's `target` (`--out` changes it) and prints each path. The flags, the file names and the surfaces are in `crates/cupertino-capture/README.md`. It needs an asset pack and exits 2 without one.

## The shell processes

The dock, menu bar, launcher, notifications and the other shell parts are separate processes. Each builds its binary behind an `app` feature:

```bash
cargo build --release -p cupertino-dock --features app
cargo build --release -p cupertino-menubar --features app
```

The server starts and restarts the processes listed under the `"shell"` key of `/etc/cupertino/compositor.json` (the packages install one that starts the whole desktop), for example `[{"name": "dock", "command": ["/usr/bin/cupertino-dock"], "roles": ["session", "dock"]}]`. A process that has roles must be a program under `/usr`; the user file `~/.config/cupertino/compositor.json` can remove or reorder the entries but not add a process with roles, and a checkout or nested run that needs other programs starts the server with `--dev-shell`. A process that exits with failure is restarted at most once per second. The optional `roles` list is what the server lets the process do on its connection: `session`, `menu_bar`, `dock`, `window_manager`, `session_lock`, `clipboard` (data control without an entry in the person's `data-control.conf`); the default is `["session"]`, and a process asks for no role it does not hold. The roles end with the process.

## Backends

| Flag | Backend |
|---|---|
| `--winit` | nested window in a Wayland or X session (development) |
| `--dev-shell` | let any configuration file name the shell processes and the screenshot process, from any path; for checkouts and nested runs only |
| `--udev` | TTY: logind, DRM/KMS and evdev |
| `--headless` | no display; use with `--screenshot out.png --exit-after-screenshot` to render one frame |

Without a flag the server runs nested when `WAYLAND_DISPLAY` or `DISPLAY` is set and on the TTY otherwise. Other useful flags: `--size WxH`, `--scale F`, `--appearance light|dark`, `--spawn "cmd args"` (repeatable), `--run-for S`. `cargo run -p cupertino-server -- --help` lists them all, and the server's README describes each.

The TTY backend has run in a virtual machine, not yet on real hardware. Treat it as experimental.

## Tests

```bash
python3 ci/worktree-build.py run --profile dev -- python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features
python3 ci/worktree-build.py run --profile dev -- cargo test --workspace --all-features --doc
python3 ci/worktree-build.py run --profile dev -- cargo clippy --workspace --all-targets
python3 ci/worktree-build.py run --profile dev -- cargo run -p cupertino-layers
```

Run these commands from a linked worktree. The outer wrapper keeps its target private and lets at most four heavy builds run at once across worktrees (`--slots N` changes that), each with a share of the processors; a new dependency or a changed `Cargo.lock` keeps the target, as Cargo rebuilds only what it affects; the fixture wrapper prepares integration-test programs before nextest starts. Keep the same profile and environment throughout the target's lifecycle; the repository's contribution guide explains seed preparation and cleanup.

Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`; CI uses 0.9.151). It runs every test in its own process and kills a test that runs longer than 2 minutes (`.config/nextest.toml`), so a hang fails with the test's name; doctests run separately. Shell and app crates need `--features app` to test their process code; pass the crate selection and features to nextest inside the wrappers above. A killed run can leave a test `dbus-daemon` behind: stop it, never your real session bus.

## Soak

`ci/soak.py` runs the daily-driver workload for hours and samples the server's RSS and open file descriptors every minute: Chrome and an Electron window each play a looping video while the page scrolls itself (from the page's script, not from input events), foot prints a listing five times a second, Files shows the home folder, and `glxgears` draws through `xwayland-satellite`. A `cupertino-testclient --intervals` window draws on every frame callback and logs the time between its presented frames; each sample carries that minute's median and 99th percentile, and the result reports how both changed after the warm-up (reported, not judged). It needs `google-chrome-stable`, `electron43`, `foot`, `glxgears`, `xwayland-satellite` and `ffmpeg` (it encodes the test video). `cupertino-files` and `cupertino-testclient` are taken from `BIN_DIR` (default `target/release`), else from `PATH`; `--headless` also runs the server from there.

```bash
ci/soak.py                                     # in a terminal of the session: 8 h, 1 h warm-up
ci/soak.py --headless --hours 1 --warmup-minutes 10
```

The run fails when the server's RSS at the end is more than 10% above its RSS after the warm-up (the median of five samples each), when any descriptor count after the warm-up is more than 5% from the one after the warm-up, when the server exits, or when a workload program exits. It writes `samples.csv` (a line a minute, as it goes), `frames.txt`, `workloads.log` and `results.json` into `soak-DATE-TIME` (`--out` changes it), prints `PASSED` or `FAILED` with the reasons, and exits 1 on a failure. It ignores the hang-up of the terminal, so a server crash that takes the terminal down still ends with `results.json`. `--without NAME` leaves a workload out and marks the result incomplete. `--headless` runs the same soak as a client of `scripts/session.sh`'s headless desktop and writes the server's log and a screenshot of the desktop three minutes in (`desktop.png`) next to the results.

## Configuration

Plain files under `~/.config/cupertino/`, each with an environment override: `shortcuts.conf` (chord to action lines), `compositor.json`, `keyboard.conf`, `dock.conf`, `menubar.conf`. The server's README lists every file and key.
