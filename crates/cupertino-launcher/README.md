# cupertino-launcher

The launcher ("search panel") of the Cupertino desktop. It stands in for the design's launcher. It does desktop
entry indexing, tiered fuzzy matching with persisted frecency, a calculator and unit conversions (its own parser, no network), a
bounded file search, sectioned results beside a preview of the selected one with its actions, and keyboard navigation, and draws
its panel with CupertinoKit.

The crate has no Smithay dependency. The engine and the panel state are separate from the window, so a host can run queries on
any thread. The `cupertino-launcher` process is the host for the desktop: a CupertinoKit application whose windows sit on layer
surfaces. Panel geometry, keyboard semantics, the top-hit rule and the section limits follow the analysis of the design's launcher.
Ranking scores, row heights and the other items marked approximate are authored and configurable. Colours, type, controls and the
material come from Kit's theme.

## Running

Feature `app` builds the `cupertino-launcher` binary: the panel on a layer surface, as a client of `cupertino-server` on
`cupertino-kit` (`cupertino_kit::app::run_on`) with `cupertino-shell-client`'s layer surfaces as the windows' roles. The server starts and restarts it through the `shell` key of its configuration.

```json
{"shell": [{"name": "launcher", "command": ["cupertino-launcher", "--terminal", "foot"]}]}
```

```
cargo build -p cupertino-launcher --features app
cupertino-launcher [--no-files] [--terminal PROGRAM]
```

| Option or file | Default | Effect |
|---|---|---|
| `--no-files` | files indexed | Do not index the home directory for file results. Otherwise a background thread builds the index on the first show. |
| `--terminal PROGRAM` | the first installed of `xdg-terminal-exec`, `$TERMINAL`, `foot`, `alacritty`, `kitty`, `wezterm`, `xterm` | Run entries with `Terminal=true` as `PROGRAM -e …` (`xdg-terminal-exec` takes the command without `-e`). |
| `~/.local/share/cupertino/launcher.json` (`$XDG_DATA_HOME/cupertino/launcher.json`) | none | The launch history (see Persistence). |

Kit follows the session's appearance and display options: Reduce Transparency and Increase Contrast give the panel the opaque
fill and the high-contrast colours.

## How it works

**Windows.** The process is one Kit application with three windows, each on a layer surface (namespace `cupertino-launcher`)
that `LauncherRoles` gives it, none of them on an output, so the server puts them on the output under the pointer:

* the panel: overlay layer, anchored to the top only (so centred horizontally), the panel's top edge at one fifth of the output's
  height, with exclusive keyboard interactivity. The window is the panel (`view::panel_height` tall) plus room for its shadow on
  every side (`view::window_size`); its input region is the panel alone (`view::panel_rect`), so a press beside the panel reaches
  the catcher. When its rows change height the model asks the host to resize the surface (`LayerSurface::set_placement`), and the
  server's configure gives Kit the new size. The window is transparent: the panel is regular glass (`PaintCx::glass`) with the
  panel's corner radius, a soft shadow drawn only outside its shape and a thin dark edge; without the desktop's effects it paints
  `Surface::Menu`'s fill instead. The glass, the shadow and the content fade in together with the appear spring;
* the catcher: top layer, over the whole output, below the panel; a press on it closes the panel;
* while the panel is hidden, one transparent point that takes no input, so the process always has a mapped surface.

Opening the panel closes the hidden window and opens the panel and the catcher; closing it does the reverse. A panel or catcher
window the server closes (its output went away) closes the panel; a hidden window it closes is replaced by a new one
(`Message::Remap`, a new window key), which the server puts on an output still there, so the process keeps running. The panel's
margin is first guessed from the first output's height, since the output the server picks is not known before the configure; the
catcher's configure gives the height of the output both were put on, and `LauncherRoles` corrects the margin
(`LayerSurface::set_margin`) when it differs, before the panel's first frame commits it.

**Input and dismissal.** The query is Kit's `BareField`, a text field without chrome set at 26 pt, which has the keyboard while
the panel is open. The panel's own keys never
reach it: the panel's root widget takes them in the capture phase (`panel::takes_key`, with Kit's key event translated by
`view::panel_key`: named keys by keysym, typing by the layout's character, command chords by the lower-case letter) and the model
runs them through `Panel::handle_key`. Every other key edits the field, which reports the query back. Hover selects a row, a click on
a row activates it, a click on an action button carries that action out, and `Escape` clears the query and then closes.

**Preview and actions.** Beside the results the pane shows the selected result: a file's picture (a thumbnail through
`cupertino-thumbnails`), the first lines of a text file or its type's icon, then its name, its type in words, its size (or a
folder's item count), when it changed and its folder; an application's icon at the pane's size and its description; a
calculation's or a conversion's value set large over its expression. The host reads a file's preview on one thread
of its own that takes only the newest request (`preview::LatestOnly`, `Host::preview`, `Message::Previewed`), so moving
through the results never piles reads up; it reads at most 4 KiB of text, only from a regular file opened without waiting
(a pipe or device never holds it), and pictures through the thumbnailer's size and decode limits. A preview that arrives
for a result no longer selected is dropped. Below it the
result's actions are capsule buttons, the chosen one filled in the accent colour (`Launcher::actions`): Open, Reveal in Files and
Copy Path for a file; Open and Copy Path for a folder; Open and the entry's desktop actions for an application; Copy for a value;
Search for the web search. Tab and Shift+Tab move through them round and round; with the list focused, Left and Right move
through them and stop at the ends; Return carries out the chosen one. Moving the selection chooses the result's own action
again. The panel also closes when its window loses the keyboard (another
window took it), on a press outside the panel, after a launch, and when the session locks: `LauncherRoles` sees the server's
`cupertino_app_effects_v1` session-lock event (which Kit passes only to the accessibility adapter) and sends the model
`Message::Locked`. The launch history is written when the panel closes.

**Launching.** `Activation::Run` asks Kit for an `xdg-activation` token for the panel window (`Command::ActivationToken`, with the
entry's desktop id), and starts the command with `XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID` when the token arrives. A copy
result (a value, or a path) goes on the system clipboard through `Command::SetClipboard`. Reveal in Files (the action, or
Primary+R) opens the folder holding a file, or a folder itself, in the session's file manager through its desktop entry
(`engine::FILE_MANAGER_ID`, with an activation token), else with `xdg-open`; the file manager opens folders, so the file itself is
not selected. Web search runs `xdg-open`. The other panel commands are logged as not provided.

Screenshot: `cupertino-server/output/launcher-open.png`.

## Protocols / D-Bus it speaks

The process owns `org.cupertino.Launcher1` at `/org/cupertino/Launcher1`. A second launcher cannot take the name and exits with
failure. The interface lives in `control` (`BUS_NAME`, `OBJECT_PATH`, `serve`). It also speaks `cupertino_session_v1` and
`xdg-activation` to the server.

| Method | Effect |
|---|---|
| `Toggle | Shows the panel or hides it. The server calls it for `Super+Space`. Only the server (the process's parent) and the launcher itself may: any other caller is refused with `org.cupertino.Error.UntrustedCaller` (`AnonymousCaller`, `FlatpakCaller`, `SnapCaller`), `cupertino_peer::check_call`. |

A call from a shell (`gdbus call ... --method org.cupertino.Launcher1.Toggle`) is refused for that reason; press
`Super+Space`, or pass `--key 2:Super+space` to a scripted server (`cupertino-server/scripts/capture-overlays.sh`).

## Icons

Application results show the entry's own `Icon=` (an absolute path or `hicolor`; `cupertino_icons::ThemeIcons`) and the asset
`application.generic` when it has none. Folder results are the asset `folder`, file results `document.generic` in the list
(the preview types the selected file with the MIME database and shows its type's icon through `cupertino_kit::file_icons`); they
are loaded at the icon size for scales 1, 2 and 3 and shown as Kit images, which draw the rendition that covers the output's scale. The web
search row is Kit's `symbol.magnifyingglass` symbol in the secondary label colour, as is the symbol in front of the query. The
calculator and conversion rows have no icon. Missing assets are reported once at start and drawn as placeholders.

## Values applied


| Item | Value |
|---|---|
| Panel width / collapsed height / minimum expanded height | 640 / 56 / 430 pt |
| Corner radius | 28 pt (a capsule while collapsed), kept as the panel grows |
| Material | regular glass; a shadow of 40 pt blur, 16 pt below, black at 0.5 (dark) or 0.22 (light); a 0.5 pt edge, black at 0.55 or 0.12 |
| Position | horizontally centred, the panel's top edge at 1/5 of the output height |
| Input | a 22 pt search symbol, 16 pt gap, the query at 26 pt without chrome, 22 pt side margins |
| Results list / preview | 288 pt list, a separator, the preview in the rest with 16 pt padding |
| Preview | picture at most 300 × 136 pt (6 pt corners), icon 64 pt, name 15 pt semibold, details 11 pt with labels ending at the pane's middle, value 28 pt; the pane takes the width the list leaves |
| Action buttons | 26 pt capsules, 4 pt apart, at most 4 shown around the chosen one |
| Result icon, selection radius, cell inset | 32 pt, 12 pt continuous, 12 pt |
| Selected-row fill | the theme's secondary selected-control fill, text keeps its label colours, no animation |
| Keyboard | no wrap, Up on the first result leaves the list, Primary+Up/Down jump sections, Backspace/Delete/Esc in the list return to the field, Esc clears then closes, Tab/Shift+Tab cycle the actions, Left/Right step them in the list, Primary+Return/R/B/I/Y/K/L/D/,/H commands |
| Top hit | one item, moved out of its section; a calculation or conversion leads instead |
| Conversions | length, mass, US customary volume, area, time (a Julian year), data (decimal and binary), speed, temperature, angle; no currencies; 6 significant digits |
| Section limits for hosts | 3 rows per section, 5 sections before "show more", 3 suggestions |
| Category defaults | `engine::DEFAULT_CATEGORY_ORDER` (enablement list; runtime order is relevance driven) |
| Show/hide animation | none found; the spring is kept as an authored fade |
| Row height 40, header 32, calculator row 64 | authored |

## Modules

| Module | Content |
|---|---|
| `desktop` (re-export of `cupertino-apps::entry`) | Desktop Entry parsing, locale fallback, `NoDisplay`/`Hidden`/`OnlyShowIn`/`NotShowIn`/`TryExec`, XDG directory indexing with id shadowing |
| `exec` (re-export of `cupertino-apps::exec`) | `Exec` quoting and field-code expansion, `CommandSpec` (with activation token and terminal wrapping) |
| `matcher` | tiers exact › prefix › word-prefix › acronym › substring › fuzzy; field weights; highlight positions |
| `frecency` | decayed launch counts + learned query→item associations, JSON persistence; the file format, its path and the ranking are `cupertino-apps::frecency` (re-exported `default_path`, `ItemRecord`, `FILE_VERSION`), which the menu bar reads the store with |
| `calc` | arithmetic parser (`+ − × ÷ ^ % `, constants, `sqrt sin cos …), number formatting; division by zero, overflow and input past 200 bytes give no result |
| `convert` | unit conversions (`10 km in mi`, `72 f to c`, `2^10 kib = kb`): the units table, the quantity through `calc` |
| `preview` | what the preview shows of a file: its type in words, size, change date, folder, text excerpt; the newest-only reader thread |
| `files` | bounded breadth-first walker, ignore rules, in-memory name index and search |
| `engine` | `Launcher`: providers → `Results { sections }`; `actions` of a result; `activate` and `activate_action` → `Activation` |
| `panel` | `Panel`: query editing, selection, scrolling, appear animation (spring); `takes_key` |
| `view` | the panel, catcher and hidden windows on CupertinoKit, panel metrics, `panel_key` |
| `kit` | `Model`, `Message`, `update` and `view`: the launcher as a Kit application, the session behind the `Host` trait |

## Integration API

```rust
use cupertino_launcher::{Launcher, LauncherConfig};
use cupertino_launcher::kit::{self, Host, Message, Model};

// startup: index XDG application dirs, load ~/.local/share/cupertino/launcher.json
let launcher = Launcher::from_system(LauncherConfig::default());
// `host` starts commands, opens paths, builds the file index, resizes the panel's surface, loads icons, reads previews and
// tells the time
let mut model = Model::new(launcher, Box::new(host));
// a Kit application: kit::update(&mut model, message) and kit::view(&model) are its update and view;
// Message::Toggle shows or hides the panel
```

* `Launcher::query(&self, text, unix_now) -> Results` is pure; run it wherever you like.
* `Launcher::activate(&mut self, item, query, unix_now) -> Activation` records frecency
  (`app:<desktop id>[#action]`, `file:<path>`; calculator copies are not recorded) and returns
  - `Run(CommandSpec)`: `argv` is the program plus arguments, `working_dir` is the entry's `Path`;
    `CommandSpec::with_activation_token` adds `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID`, `CommandSpec::to_command` builds a
    `std::process::Command`;
  - `CopyToClipboard(text)`;
  - `Nothing` (unusable `Exec`).
* `Panel` is the keyboard model without a window: `Panel::handle_key` returns what the host has to do (`PanelEvent`), and
  `panel::takes_key` tells beforehand whether a key is the panel's or the query field's. Its bindings are listed in `panel.rs`
  (Up/Down without wrap, Primary+Up/Down section jumps, PageUp/PageDown, Tab and Left/Right through the actions, Alt+1…9, Enter,
  Escape). Shortcuts that need the
  host arrive as `PanelEvent::Command(PanelCommand)`; `Mods::primary` is the Super key.

## Matching and ranking

Whitespace-separated tokens must all match; a token matches a field of a candidate in the best
available tier (scores: exact 1000, prefix 900, later word 700, acronym 600 — camel-case humps and
punctuation count as word starts —, substring 450, fuzzy 100–280). Fuzzy matches must be compact
(span ≤ len + max(4, len/2)). Fields and weights: name 1.0, generic name 0.7, keywords 0.65,
executable 0.6, desktop id 0.4, comment 0.3 (comment and keywords never match fuzzily). Desktop
actions appear as `App: Action` only when a token matches the action's own name. Case and common
Latin diacritics are folded.

Final score = text score + frecency bonus (≤ 120, `45·ln(1+score)`, score halves every 14 days)
+ query-association bonus (≤ 80, for items launched after a query that starts with the current one).
The bonuses cannot lift a fuzzy match over a prefix match. Empty query: the most frecent
applications ("Suggestions"). The calculator section is first when the input is arithmetic; otherwise the best application / file / folder
match is promoted to a one-item "Top Hit" section (`LauncherConfig::top_hit`).

## File search

`FileIndex::build(FileSearchConfig)`: breadth-first walk of the root with bounds
(`max_depth` 6, `max_entries` 200 000, `max_dirs` 40 000, 5 s), skipping hidden entries, heavy
directory names, `.gitignore`/`.ignore` rules (negation, anchoring, `**`, directory-only) and extra
patterns; symlinks are not followed. Search matches the file name (parent folder name as a weak
second field), requires at least a substring-tier match (`min_score` 300), prefers shallow and
recently modified files. Opening goes through `open_command` (default `xdg-open`).

## Persistence

`~/.local/share/cupertino/launcher.json` (`$XDG_DATA_HOME/cupertino/launcher.json`), versioned,
written atomically; a missing, corrupt or foreign-version file starts empty. Bounded to 2 000 items
and 500 queries. The README of the compositor integration should call `save_if_dirty` on a timer or
at shutdown.

## Open issues

The keypad navigation keys without Num Lock are not mapped, and the field edits with Kit's standard chords rather than the user's
keyboard profile. The preview's sized labels draw in the label colours themselves rather than through Kit's text roles. Reveal
opens the folder without selecting the file, since the file manager takes folders only.

## Tests

```
cargo run -p cupertino-launcher --release --example search -- [--files] <query>
cargo test  -p cupertino-launcher --features app
CUPERTINO_UPDATE_GOLDENS=1 cargo test -p cupertino-launcher view::   # re-record the fixtures, then fails
cargo bench -p cupertino-launcher
```

| Item | Proves or shows |
|---|---|
| `tests/control.rs` | The `Toggle` interface against a private `dbus-daemon`, that a second launcher cannot take the name, and that another process (a `dbus-send` the test starts) is refused as `UntrustedCaller`. |
| `cupertino-server/tests/shell_processes.rs` | The real process (`--no-files`) connects to the real server, maps its hidden window at start and keeps running. |
| `src/view/tests.rs` | The windows run headless on Kit's test driver: the search field, the results list and keyboard navigation against the Inspector goldens in `tests/goldens/` and the screen-reader readings in `tests/fixtures/screen-reader/`; launching, the clipboard, hover, click and dismissal; the preview arriving (and a stale one dropped), Reveal in Files, choosing an action with the arrows, a conversion. |
| `cargo test` | Matching, ranking, frecency, calculator (division by zero, huge numbers, invalid input), conversions, preview words and dates, file search, panel state and action traversal, that `takes_key` agrees with `handle_key`, and the `app` process code. |
| `cargo bench` | Scores 5 000 synthetic applications: about 1–3 ms per query in a release build (14 ms to build the index), calculator 0.6 µs. |
