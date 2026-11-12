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
| `atomic::write_atomic_private(path, bytes)` | `write_atomic` for a file only its owner may read or change: mode `0600` whatever the old file had, and a link at the path is replaced rather than followed |
| `system::is_root_only`, `system::only_root_can_change(path)`, `system::root_only_target(path)` | a regular file of root's that no group or other user can write; such a file whose directories up to `/` are root's and writable by no one else; the file a path leads to through such directories and links of root's only (at most 40 links) |
| `conf::ConfText`, `conf::Key`, `conf::read` / `write` / `load` / `store` | the `key = value` format: a file kept as its lines, read through one table of keys per file (read, write, keep the last good value) and changed in place |
| `watch::ConfigWatcher` | watches files and reports, from its thread, each quiet burst of changes once (`WatchEvent::Changed(paths)`; `WatchEvent::Error`) |
| `dock::DockSettings` | `dock.conf` (`$CUPERTINO_DOCK_CONFIG`): pins (read only), sizes with the design validation (`sanitize_tile_size`, `sanitize_large_size`), magnification, auto-hide, position, recents, minimise into the application's tile, the launch bounce, the running indicators, and the eight corner and gesture actions (`Trigger`, `ActionTarget`, `ActionMap` with the defaults of) |
| `appearance::AppearanceSettings` | `appearance.conf` (`$CUPERTINO_APPEARANCE_CONFIG`), read by every program that draws with the toolkit: the automatic look and its two switch times, the accent and highlight colours, the sidebar icon size, what a click in a scroll bar's track does, and what a double click on a title bar does |
| `menubar::MenuBarSettings` | `menubar.conf` (`$CUPERTINO_MENUBAR_CONFIG`): the clock options (`ClockSettings`, unset as the design reads it), which items show (`ShownItems`), the bar's own background, the recent-item counts and the status icons kept out of the bar |
| `keyboard::KeyboardSettings` | `keyboard.conf` (`$CUPERTINO_KEYBOARD_CONFIG`): every key as `Option`, applied by the server over the keyboard model's defaults; input sources (`SourceSpec`), the file's choice values (`ModifierProfile`, `FnKey`, `LayoutKeyboard`, `Level3`), `CAPS_LOCK_HOLD_MAX_MS` (1500) |
| `compositor::CompositorSettings` | `compositor.json` (`$CUPERTINO_SERVER_CONFIG`): every key as `Option`; each key checked on its own (`JsonProblem`); unknown keys kept on a write. `ScrollBarsName` is `show_scroll_bars`: `automatic`, `when-scrolling` or `always` |
| `launcher::LauncherSettings` | `launcher.conf` (`$CUPERTINO_LAUNCHER_CONF`): which kinds of result the launcher lists (`results.applications`, `results.folders`, `results.files`, `results.calculator`, `results.actions`, all on when unset), `top_hit` (on when unset) and `files.exclude`, a comma-separated list of folders the file search leaves out (`exclusion_patterns` turns them into ignore patterns). The launcher reads it each time it opens, the settings application writes it |
| `notifications::NotificationSettings` | `notifications.conf` (`$CUPERTINO_NOTIFICATIONS_CONF`): what notifications show on the lock screen and how much of them shows, as a default (`show_on_lock_screen`, on when unset; `show_previews`, `Previews::{Always, WhenUnlocked, Never}`, `when-unlocked` when unset) and per-application overrides (`app.<id>.show_on_lock_screen`, `app.<id>.show_previews`, `app.<id>.allow` (off drops the application's notifications) and `app.<id>.alert_style` (`banners` or `none`: none keeps them in the centre without a banner), each unset to follow the default; `critical_during_dnd`, on when unset, lets critical notifications through Do Not Disturb; `<id>` the desktop entry id or the application name, `valid_app_id`); `policy(id)` combines them. The notification daemon reads it, the settings application writes it; `store` removes the lines of an override taken away |
| `power::PowerSettings` | `power.conf` (`$CUPERTINO_POWER_CONF`): display and system sleep timers per power source, dim before display sleep, the password delay after display sleep and the lock screen's display timeout, with the design defaults |
| `displays::DisplaysConfig` | `displays.conf` (redirected by `CUPERTINO_DISPLAYS_CONFIG`): one section per display, headed by its `display_key` (make, model and serial, else `connector <name>`), with `enabled`, `mode` (`WxH@millihertz`), `position`, `scale` and `transform`, every key required; `parse` / `to_text` round-trip, `load` reads a missing file as empty and returns the problems of malformed sections (which are skipped), `store` writes atomically and creates the directory |
| `login::LoginConfig` | `login.conf` of the session manager (`$CUPERTINO_LOGIN_CONF`, default `/etc/cupertino/login.conf`; system configuration, not under the XDG directory): the login screen's VT and account, the greeter and session commands, the account filters and power-item switches, the hint setting; `parse` gives the settings and every bad line as `(line, message)`, the key of a bad line at its safe value (the module's table), and is an error only for a missing or bad greeter or session command (a missing one is line 0); `load` reads the file (a missing file is an error: the manager refuses to start) |
| `accessibility::AccessibilitySettings` | `accessibility.conf` (`$CUPERTINO_ACCESSIBILITY_CONF`): the four display options (`DisplayOptions`: reduce motion, reduce transparency, increase contrast, differentiate without colour; off when absent), `invert_colors`, `display_contrast` (0 to 1), `color_filter` (`grayscale`, `protanopia`, `deuteranopia`, `tritanopia`, `tint <hue>`) and `color_filter_intensity` (0.25 to 1 or `default`: 0.5 for the corrections, 1.0 grayscale, 0.4 tint), `screen_reader` and `steady_caret` (`STEADY_CARET`, on or off: text carets stay shown without blinking); `load` returns the lines it did not understand, `store` writes atomically |
| `data_control::DataControlConf` | `data-control.conf` (`$CUPERTINO_DATA_CONTROL_CONF`): the applications the person allows to use data control, one `allow = [<dev>:<ino>:<size>:<mtime ns> ]<program>` per line (`Entry`, `Pin`). `Entry::for_program` pins a program unless only root can change it; `refusal` names the interpreters, loaders and Wayland proxies that are never allowed; a bad line allows nothing. `load` refuses a link, a FIFO or other non-regular file (opened without blocking), a file not the user's or writable by others, or one in a directory others can write; `store` writes with `write_atomic_private` and refuses, writing nothing, an entry a line cannot hold as itself (relative, `#`, a line break, white space at either end) or a program `refusal` names |
| `desktop_control::DesktopControlConf` | `desktop-control.conf` (`$CUPERTINO_DESKTOP_CONTROL_CONF`): the person's answer for each app that asked to control the desktop (`Service::Control`: observe windows and their controls, semantic actions, pointer and keyboard input) or to take pictures of the screen (`Service::Capture`), one `allow\|deny = <service> app:<desktop entry id>\|exe:<absolute path>` per line. The window server records an answer the first time an app asks and follows the file; Settings' Privacy pane turns an app on or off (`set`) and forgets it (`forget`), which makes it ask again. `load` and `store` refuse and write as `data_control` does (the reader's own regular file, mode `0600`); both writers change the file only through `update`, which reads, changes and stores it while holding the file's lock |
| `screen_sharing::ScreenSharingConf` | `screen-sharing.conf` (`$CUPERTINO_SCREEN_SHARING_CONF`): the displays the person let an app share again without asking, one `grant = <id> <app id> display <connector>` per line (`Grant`, the id 32 lowercase hexadecimal digits). The portal writes a grant when the person ticks to remember a share and Settings forgets it (`revoke`); `find` restores only for the same app id. `load` and `store` refuse and write as `data_control` does (the reader's own regular file, mode `0600`). Both writers change the file only through `update`, which reads, changes and stores it while holding the file's lock |
| `lock::FileLock` | An advisory `flock` on `.<name>.lock` beside a settings file that more than one process changes (mode `0600`, the reader's own regular file, never a link), held for a whole load-modify-store so no store made from an older read writes over another process's change; `acquire` waits for it at most `LOCK_WAIT` (2 s) and is then `TimedOut`, so a holder that never lets go fails the change instead of stopping the portal or Settings; dropping it lets go |

The six settings files share one shape: `path()`, `parse(text, last_good)` and `load(path, last_good)` return the settings
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
removed and created again is watched again. A file is read without waiting for a writer, so a FIFO put at a watched
path reads as empty instead of holding the watch. A panic in the callback ends the watch and is written to stderr. The callback runs on the watcher's thread: the server forwards it to a calloop channel,
a shell process to its client's waker.

Only the compositor reads and writes `displays.conf` and `accessibility.conf`. It sends the display options to the shell
processes on the session object. `SYSTEM_ACCESSIBILITY` (`/etc/cupertino/accessibility.conf`) is the machine's file in the
same format, for the login screen and every key an account's own file does not record; `SystemAccessibility::from_text` applies only
`SYSTEM_KEYS` (`reduce_motion`, `screen_reader_at_login`) from it and reports any other key. `screen_reader_at_login` (on or off)
starts the login screen with its screen reader on; it is a field of `SystemAccessibility` only, so a person's file that names it
is reported and changes nothing. `screen_reader` (on or off, off by default) is the other way round: a key of a person's file
that runs the screen reader in their session (the compositor starts it), which the machine's file cannot set.
`SCREEN_READERS` (`/usr/bin/orca`) lists the readers either may start, and `installed_screen_reader` takes the first that only
root can change (`system::root_only_target`), never one from `$PATH`. `PersonalAccessibility` is a person's `accessibility.conf`: only the
keys they changed (`record`), each kept once changed even when set back to the machine's value, and `over` lays them on the
machine's settings. A file with every key, as earlier versions wrote it, stays valid and sets them all. A key joins `SYSTEM_KEYS` only after a security review. `system::read_root_file` reads such a file only when it is a regular file of root's that no one else can write (no link followed); the compositor and `cupertino-sessiond` read the machine's files with it, and `SystemAccessibility::to_text` writes the machine's file.

**Power defaults.** Display sleep is 600 s on mains and 120 s on battery, and dim is on (the
reference's built-in power table, no model overrides them). System sleep is never on mains, because a build, a download or a
remote session takes no sleep inhibitor and nothing else says the machine is busy, and 600 s on battery. The session locks 300 s after the
display sleeps (the new-user template), and the display sleeps after 30 s on the lock screen. Values are in seconds, where the design uses minutes, so tests can use short timers. `never` switches a timer
off.

## Open issues

- The server watches `compositor.json`, `keyboard.conf` and `power.conf` (its README, *Live settings*), the dock `dock.conf`
  and the menu bar `menubar.conf`; a few keys still apply at the next start only (listed in those READMEs).
- `windowmanager.conf`, `screenshot.conf` and `shortcuts.conf` are still parsed by their processes.

## Tests

`cargo test -p cupertino-defaults`: the atomic write (a process killed between write and rename leaves the old file), the
line format, each file's keys and round trip, and the watcher (one report per change, none for unchanged contents).
