# cupertino-launcher

The launcher ("search panel") of the Cupertino desktop. It stands in for the design's launcher. It does desktop
entry indexing, tiered fuzzy matching with persisted frecency, a calculator, a bounded file search, sectioned results, keyboard
navigation and CPU rendering of the panel.

The crate has no Smithay dependency. The engine, the panel state and the renderer are separate, so a host can run queries on
any thread and draw the panel itself. The `cupertino-launcher` process is that host for the desktop. Panel geometry, material,
keyboard semantics, the top-hit rule and the section limits follow the analysis of the design's launcher.
Ranking scores, row heights
and the other items marked approximate are authored and configurable. Colours, typography, rim and shadow come from
`cupertino-theme` / `cupertino-decorations` / `cupertino-glass`.

## Running

Feature `app` builds the `cupertino-launcher` binary: the panel on a layer surface, as a client of `cupertino-server` on
`cupertino-wayland-client`, `cupertino-shell-client`, `cupertino-protocol-app` and a `cupertino-ui-wayland` presenter per surface. The server starts and restarts it through the `shell` key of its configuration.

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

Reduce Transparency and Increase Contrast from the session's display options select the opaque material and the high-contrast
catalog.

## How it works

**Surface.** One overlay layer surface over the whole output. It is created without an output, at start and each time the panel
opens, so the server puts it on the output under the pointer. The process takes that output from the surface's
`wl_surface.enter` (`Connection::surface_outputs`) when it is configured, replaces the surface on that output when the output needs
another surface size, and puts the outside-click catcher there. When the output goes away the server closes the surfaces, and
they start again on the output the server chooses. Keyboard interactivity is `exclusive` while the panel is shown and `none`
otherwise. The input region is the whole surface while shown (so a press outside the panel closes it) and empty otherwise.
The panel material is a server-rendered backdrop (`PanelStyle::material_for`, `Menu` when the recipe is missing) with the
panel's rectangle and radius. The process draws only the shadow and the content (`Layers { shadow, content }`) with the appear
spring as opacity. The panel is centred with its top edge at one fifth of the output height (`PanelStyle::placement`).

**Session.** The appearance comes from `cupertino_session_v1`, read at start. On a change the renderer is rebuilt, the backdrop
is created again with the material of the new appearance (and the surfaces are replaced if the shadow margins differ), and the
panel is drawn again. On `locked` a shown panel closes (the launch history is saved as for any close).

**Input and dismissal.** Key events of the toolkit become panel keys in `surface::panel_key`: named keys by evdev code, typing
by the layout's character, command chords by the lower-case letter. Held keys repeat through the toolkit. Hover selects a row,
a click on a row activates it, and `Escape` clears the query and then closes. The panel also closes when the keyboard
focus moves elsewhere (another window took it), on a press outside the panel, on a non-left button press and after a launch.
The launch history is written when the panel closes.

**Launching.** `Activation::Run` asks the server for an `xdg-activation` token with the key press or click that chose the
entry, and starts the command with `XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID` when the token arrives. A copy result (the
calculator's) goes on the system clipboard through `cupertino-ui-wayland`'s `Clipboard` with the same input event,
and the launcher serves it until another selection replaces it. Reveal and web search run `xdg-open`. The other panel commands are logged as not provided.

Screenshot: `cupertino-server/output/launcher-open.png`.

## Protocols / D-Bus it speaks

The process owns `org.cupertino.Launcher1` at `/org/cupertino/Launcher1`. A second launcher cannot take the name and exits with
failure. The interface lives in `control` (`BUS_NAME`, `OBJECT_PATH`, `serve`). It also speaks `cupertino_session_v1` and
`xdg-activation` to the server.

| Method | Effect |
|---|---|
| `Toggle | Shows the panel or hides it. The server calls it for `Super+Space`. |

```
gdbus call --session --dest org.cupertino.Launcher1 --object-path /org/cupertino/Launcher1 --method org.cupertino.Launcher1.Toggle
```

## Icons

Application results show the entry's own `Icon=` (an absolute path or `hicolor`; `cupertino_icons::ThemeIcons`) and the asset
`application.generic` when it has none. Folder results are the asset `folder`, file results `document.generic` (file results
are not typed: the launcher holds no MIME database yet), and the web search row is `symbol.magnifyingglass` in the secondary
label colour. `PanelRenderer::new` fails when the directory lacks `application.generic`, `folder`, `document.generic` or `symbol.magnifyingglass`. The calculator row keeps its drawn tile (no asset exists for it; approximate); no other result is drawn when an icon is missing.

## Values applied


| Item | Value |
|---|---|
| Panel width / collapsed height / minimum expanded height | 600 / 52 / 430 pt |
| Corner radius | 16 pt, continuous |
| Position | horizontally centred, top edge at 1/5 of the output height |
| Input | 26 pt (large-title style), 20 pt side margins (560 pt field) |
| Result icon, selection radius, cell inset | 32 pt, 12 pt continuous, 12 pt |
| Selected-row fill | quaternary label colour (vibrant wash), text keeps its label colours, no animation |
| Shadow | the standard window row of the shadow table |
| Material | light: inline-sidebar recipe, dark: sidebar recipe (opaque twins when transparency is reduced) |
| Keyboard | no wrap, Up on the first result leaves the list, Primary+Up/Down jump sections, Tab/Delete/Esc in the list return to the field, Esc clears then closes, Primary+Return/R/B/I/Y/K/L/D/,/H commands, Space previews |
| Top hit | one item, moved out of its section; a calculator result leads instead |
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
| `calc` | arithmetic parser (`+ − × ÷ ^ % `, constants, `sqrt sin cos …), number formatting |
| `files` | bounded breadth-first walker, ignore rules, in-memory name index and search |
| `engine` | `Launcher`: providers → `Results { sections }`; `activate` → `Activation` |
| `panel` | `Panel`: query editing, selection, scrolling, appear animation (spring) |
| `render` | `PanelStyle`, `PanelRenderer`, `PanelLayout`, hit testing |

## Integration API

```rust
use cupertino_launcher::{Launcher, LauncherConfig, Activation};
use cupertino_launcher::files::{FileIndex, FileSearchConfig};
use cupertino_launcher::panel::{Panel, Key, Mods, Now, PanelEvent};
use cupertino_launcher::render::{PanelRenderer, PanelStyle, RenderOptions};

// startup: index XDG application dirs, load ~/.local/share/cupertino/launcher.json
let mut launcher = Launcher::from_system(LauncherConfig::default());
launcher.config.terminal_command = vec!["foot".into(), "-e".into()];   // Terminal=true entries
launcher.set_activation_token_hook(Some(Box::new(|req| mint_xdg_activation_token(req.desktop_id.as_deref()))));

// files: build off-thread, hand over when done, then panel.refresh(..)
let handle = FileIndex::spawn_build(FileSearchConfig::home().unwrap());
// ... later: launcher.set_file_index(Some(Arc::new(handle.join().unwrap())));

let mut panel = Panel::new();
let mut renderer = PanelRenderer::new(PanelStyle::from_theme(&theme, Appearance::Light, false), &assets)?;   // assets: Assets::from_env()?
panel.max_rows = renderer.style.max_rows;

// hot key: panel.toggle(&launcher, Now { mono: frame_time, unix: unix_time });
// key press (already translated by xkb): 
match panel.handle_key(Key::Char('f'), Mods::NONE, &mut launcher, now) {
    PanelEvent::Activate(Activation::Run(cmd)) => spawn(cmd),               // CommandSpec: program, args, env, cwd
    PanelEvent::Activate(Activation::CopyToClipboard(text)) => set_clipboard(text),
    PanelEvent::Activate(Activation::Nothing) | PanelEvent::None => {}
    PanelEvent::Command(cmd) => run_panel_command(cmd, panel.selected_item()),   // reveal, preview, web search, …
    PanelEvent::Changed | PanelEvent::Close => request_redraw(),
}
// every frame while panel.needs_draw(t):
let layout = renderer.layout(&panel);
let rect = renderer.style.placement(output_size_pt, layout.size.height);   // centred, 20 % from the top
let img = renderer.render(&panel, &layout, &RenderOptions { scale, caret_visible, ..Default::default() });
// upload img.pixmap (premultiplied RGBA8) at rect, alpha = panel.progress(t)
// pointer: layout.item_at(point_in_panel) → panel.set_selected(i) / panel.activate_index(i, ..)
// when the file index arrives: launcher.set_file_index(..); panel.refresh(&launcher, now);
// shutdown / after launches: launcher.frecency.save_if_dirty()
```

* `Launcher::query(&self, text, unix_now) -> Results` is pure; run it wherever you like.
* `Launcher::activate(&mut self, item, query, unix_now) -> Activation` records frecency
  (`app:<desktop id>[#action]`, `file:<path>; calculator copies are not recorded) and returns
  - `Run(CommandSpec)`: `argv` is the program plus arguments, `env` contains
    `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID` when the hook returned a token,
    `working_dir` is the entry's `Path`; `CommandSpec::to_command` builds a `std::process::Command`;
  - `CopyToClipboard(text);
  - `Nothing` (unusable `Exec`).
* Compositor with its own material pass: `Layers::CONTENT_ONLY` renders input, rows and rim only;
  draw shadow and background from `PanelStyle::material` (recipe `PanelStyle::material_for(appearance),
  fallback `Menu`), radius `style.corner_radius`, curve `style.corner_style`.
  `Backdrop { image, panel_origin }` runs the recipe through `cupertino-glass`'s CPU path instead
  (feature `material`, default on).
* The query is a shared text field (`cupertino-ui-text`'s `TextBuffer`): caret with blink (steady under Reduce Motion), word and line moves, Shift selection,
  click, double and triple click, drag, copy, cut and paste with the system clipboard, undo. Keys the panel does not
  take come back as `PanelEvent::Edit` and go to `Panel::edit`.
* The panel's keyboard bindings are listed in `panel.rs` (Up/Down without wrap,
  Primary+Up/Down section jumps, PageUp/PageDown, Tab, Alt+1…9, Enter, Escape). Shortcuts that need the
  host arrive as `PanelEvent::Command(PanelCommand) (reveal, properties, preview, web search, settings,
  help, dictionary / reference results); `Mods::primary` is the Super key. `Panel::focus tells whether
  the field or the list owns the keyboard.

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

**Text.** The `ui` font is a variable font whose optical size follows the point size, so every `TextRenderer` owner calls `set_scale` with the output scale before it draws and measures (`measure` and `fit` work in points, `draw_text` at the render scale): text is laid out in points at the same optical size it is drawn with, at every output scale.

## Open issues

The keypad navigation keys without Num Lock are not mapped.

## Tests

```
cargo run -p cupertino-launcher --release --example launcher_samples   # output/*.png
cargo run -p cupertino-launcher --release --example search -- [--files] <query>
cargo test  -p cupertino-launcher --features app
cargo bench -p cupertino-launcher
```

| Item | Proves or shows |
|---|---|
| `tests/control.rs` | The `Toggle` interface against a private `dbus-daemon`, and that a second launcher cannot take the name. |
| `cupertino-server/tests/shell_processes.rs` | The real process (`--no-files`) connects to the real server, draws its hidden panel at start and keeps running. |
| `cargo test` | Matching, ranking, frecency, calculator, file search, panel state and rendering, and the `app` process code. |
| `output/ | Suggestions, applications, acronym, multi-word (dark), calculator and file results (dark), rendered over a procedural wallpaper through the material recipe. |
| `cargo bench` | Scores 5 000 synthetic applications: about 1–3 ms per query in a release build (14 ms to build the index), calculator 0.6 µs. |
