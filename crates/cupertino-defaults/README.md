# cupertino-defaults

The settings store shared by the Cupertino processes. Every settings file lives in `$XDG_CONFIG_HOME/cupertino/` (default
`~/.config/cupertino/`), and each file has its own environment variable that redirects it, so tests and fixtures never
touch the user's files. For each file the crate has one typed reader and writer, used by the process that applies the
file and by the settings application, an atomic write, and change notification. Dependencies: `notify` (inotify) and
`serde_json` (for `compositor.json`).

## Public API

| Function / type | Contents |
|---|---|
| `config_home | `$XDG_CONFIG_HOME`, else `$HOME/.config`; `None` when neither is set |
| `data_dirs | XDG data directories, most important first: `$XDG_DATA_HOME` (default `~/.local/share`), then `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`) |
| `config_file(env, name) | the value of `env` when it is set and not empty, else `<config home>/cupertino/<name>; the file need not exist |
| `atomic::write_atomic(path, bytes) | replace a file atomically: a temporary file in the same directory (`.<name>.<pid>.<n>.tmp`; a name a crash left behind is skipped), flushed, renamed over the file, then the directory flushed; the directory is created, an existing file keeps its permissions; a settings file that is a symbolic link stays one (the file it names is replaced, in its own directory); a failure before the rename removes the temporary file and leaves the old file |
| `conf::ConfText`, `conf::Key`, `conf::read` / `write` / `load` / `store` | the `key = value` format: a file kept as its lines, read through one table of keys per file (read, write, keep the last good value) and changed in place |
| `watch::ConfigWatcher` | watches files and reports, from its thread, each quiet burst of changes once (`WatchEvent::Changed(paths)`; `WatchEvent::Error`) |
| `dock::DockSettings` | `dock.conf` (`$CUPERTINO_DOCK_CONFIG`): pins (read only), sizes with the design validation (`sanitize_tile_size`, `sanitize_large_size`), magnification, auto-hide, position, recents, and the eight corner and gesture actions (`Trigger`, `ActionTarget`, `ActionMap` with the defaults of) |
| `menubar::MenuBarSettings` | `menubar.conf` (`$CUPERTINO_MENUBAR_CONFIG`): the clock options (`ClockSettings`, unset as the design reads it) |
| `keyboard::KeyboardSettings` | `keyboard.conf` (`$CUPERTINO_KEYBOARD_CONFIG`): every key as `Option`, applied by the server over the keyboard model's defaults; input sources (`SourceSpec`), the file's choice values (`ModifierProfile`, `FnKey`, `LayoutKeyboard`, `Level3`), `CAPS_LOCK_HOLD_MAX_MS` (1500) |
| `compositor::CompositorSettings` | `compositor.json` (`$CUPERTINO_SERVER_CONFIG`): every key as `Option`; each key checked on its own (`JsonProblem`); unknown keys kept on a write |
| `power::PowerSettings` | `power.conf` (`$CUPERTINO_POWER_CONF`): display and system sleep timers per power source, dim before display sleep, the password delay after display sleep and the lock screen's display timeout, with the design defaults |
| `displays::DisplaysConfig` | `displays.conf` (redirected by `CUPERTINO_DISPLAYS_CONFIG`): one section per display, headed by its `display_key` (make, model and serial, else `connector <name>`), with `enabled`, `mode` (`WxH@millihertz`), `position`, `scale` and `transform`, every key required; `parse` / `to_text` round-trip, `load` reads a missing file as empty and returns the problems of malformed sections (which are skipped), `store` writes atomically and creates the directory |
| `login::LoginConfig` | `login.conf` of the session manager (`$CUPERTINO_LOGIN_CONF`, default `/etc/cupertino/login.conf`; system configuration, not under the XDG directory): the login screen's VT and account, the greeter and session commands, the account filters and power-item switches, the hint setting; `parse` returns every bad line as `(line, message)` and a missing required key as line 0, `load` reads the file (a missing file is an error: the manager refuses to start) |
| `accessibility::AccessibilitySettings` | `accessibility.conf` (`$CUPERTINO_ACCESSIBILITY_CONF`): the four display options (`DisplayOptions`: reduce motion, reduce transparency, increase contrast, differentiate without colour; off when absent), `invert_colors`, `display_contrast` (0 to 1), `color_filter` (`grayscale`, `protanopia`, `deuteranopia`, `tritanopia`, `tint <hue>`) and `color_filter_intensity` (0.25 to 1 or `default`: 0.5 for the corrections, 1.0 grayscale, 0.4 tint); `load` returns the lines it did not understand, `store` writes atomically |

The five settings files share one shape: `path()`, `parse(text, last_good)` and `load(path, last_good)` return the settings
with the problems (`conf::Problem { line, message }`; `JsonProblem { key, message }` for JSON), `store(&self, path)`
writes atomically.

## How it works

**Reading.** Lines apply in order, a later line of a key wins. A line that is not understood (unknown key, bad value, a bare
word where none is allowed) is a problem with its line number; the other lines apply. With the last good settings given,
the key of a bad line keeps its last good value, so a typo while the file is being edited does not reset a running process
to the default. Without them (at start) the key is unset or at its default. `compositor.json` is checked key by key in the
same way; a file that is not a JSON object is one problem and keeps all the last good values.

**Writing.** `store` reads the file, compares the written form of each key in the file with the new settings and rewrites only
the keys that differ: in place on the key's last line, keeping its indentation and comment, else appended; a key set back
to unset is removed. Comments, blank lines, unknown keys, the dock's pin lines and the order of every other line stay. The
result is written with `write_atomic`. `compositor.json` keeps unknown keys too; JSON has no comments, and a write orders
the keys alphabetically (`serde_json` without `preserve_order`, which would change the map type for the whole workspace).

**Change notification.** `ConfigWatcher` watches the directory of each file (inotify through `notify`, non-recursive), so an
atomic replacement, an editor's save, a creation and a removal are seen, also for a file an environment variable moved
elsewhere. Events are coalesced until a burst is quiet for `watch::COALESCE` (50 ms, approximate); then each watched file
whose contents changed since the last report is reported once. A write of the same contents, other files and the
temporary files report nothing. After a notification error (an overflowing queue) every file is compared again, so a
change in that window is not lost. The parents of the watched directories are watched too: a settings directory that is
removed and created again is watched again. A panic in the callback ends the watch and is written to stderr. The callback runs on the watcher's thread: the server forwards it to a calloop channel,
a shell process to its client's waker.

Only the compositor reads and writes `displays.conf` and `accessibility.conf`. It sends the display options to the shell
processes on the session object. `SYSTEM_ACCESSIBILITY` (`/etc/cupertino/accessibility.conf`) is the machine's file in the
same format, for the login screen and accounts without their own; `AccessibilitySettings::from_system_text` applies only
`SYSTEM_KEYS` (`reduce_motion`) from it and reports any other key. A key joins `SYSTEM_KEYS` only after a security review.

**Power defaults.** Display sleep is 600 s on mains and 120 s on battery, system sleep is 600 s on both, and dim is on (the
reference's built-in power table, no model overrides them). The session locks 300 s after the
display sleeps (the new-user template), and the display sleeps after 30 s on the lock screen. Values are in seconds, where the design uses minutes, so tests can use short timers. `never` switches a timer
off.

## Open issues

- The server watches `compositor.json`, `keyboard.conf` and `power.conf` (its README, *Live settings*), the dock `dock.conf`
  and the menu bar `menubar.conf`; a few keys still apply at the next start only (listed in those READMEs).
- `windowmanager.conf`, `screenshot.conf` and `shortcuts.conf` are still parsed by their processes.

## Tests

`cargo test -p cupertino-defaults`: the atomic write (a process killed between write and rename leaves the old file), the
line format, each file's keys and round trip, and the watcher (one report per change, none for unchanged contents).
