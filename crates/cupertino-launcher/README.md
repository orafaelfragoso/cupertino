# cupertino-launcher

The launcher ("search panel") of the Cupertino desktop. It stands in for the design's launcher. It does desktop
entry indexing, tiered fuzzy matching with persisted frecency, a calculator and unit conversions (its own parser, no network), a
bounded file search, sectioned results beside a preview of the selected one with its actions, and keyboard navigation. In the same
panel it shows the application browser: every application, filtered by category and the query, as a grid or a list, which the Dock
opens. It draws its panel with CupertinoKit.

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
| `launcher.conf` (`$CUPERTINO_LAUNCHER_CONF`) | every kind on | Read each time the launcher opens: which kinds of result it lists (applications, folders, files, calculator, actions), whether it marks a top hit, and the folders the file search leaves out. |
| `--no-files` | files indexed | Do not index the home directory for file results. Otherwise a background thread builds the index on the first show. |
| `--terminal PROGRAM` | the first installed of `xdg-terminal-exec`, `$TERMINAL`, `foot`, `alacritty`, `kitty`, `wezterm`, `xterm` | Run entries with `Terminal=true` as `PROGRAM -e …` (`xdg-terminal-exec` takes the command without `-e`). |
| `~/.local/share/cupertino/launcher.json` (`$XDG_DATA_HOME/cupertino/launcher.json`) | none | The launch history (see Persistence). |

Kit follows the session's appearance and display options: Reduce Transparency and Increase Contrast give the panel the opaque
fill and the high-contrast colours.

## How it works

**Windows.** The process is one Kit application with three windows, each on a layer surface (namespace `cupertino-launcher`)
that `LauncherRoles` gives it, none of them on an output, so the server puts them on its active output, the one the person works on:

* the panel: overlay layer, anchored to the top only (so centred horizontally), the panel's top edge at 203/982 of the output's
  height, with exclusive keyboard interactivity. The enclosure (the panel's shape: `view::compact_shape`, `view::search_shape`,
  `view::browse_shape`) sits in a window with room for its shadow on every side (`view::window_extent`). While the shape moves,
  the window holds both the shape presented and the one it heads for, and once it settles the window is exactly the target's, so
  it never cuts the enclosure off mid-way nor keeps room it no longer needs. Its input region is the enclosure alone
  (`view::enclosure_rect`) while the panel is open and nothing while it closes, so a press beside the panel reaches the catcher.
  When the shape changes the model asks the host to resize the surface (`LayerSurface::set_placement`), and the server's
  configure gives Kit the new size. A settled shape's width and height are multiples of 4 pt (`view::SHAPE_GRID`), so the glass
  ends on whole device pixels at 1, 1.25, 1.5, 1.75 and 2. The window is transparent: the enclosure is large-panel glass
  (`Glass::large_panel`, the regular recipe blurred twice as wide with a refraction band at most 6 pt deep and a face of its
  own: in the dark appearance its tones pressed together, a lower white, a lifted black and a little less saturation; in the
  light one a bright, more saturated sheet, black behind coming through as a light grey and a dark scene as a light tint of
  its own hue, with a bright line just inside the edge; so the panel reads as one calm sheet over any wallpaper) with the
  panel's corner radius and a thin dark edge;
  the compositor draws the glass's soft shadow outside its shape. Without the desktop's effects it paints `Surface::Menu`'s
  fill and a shadow of its own instead. While the browser shows, the panel fades by giving the glass, the search bar and
  the browser's body each their opacity rather than compositing the whole window as one group, which keeps every frame of
  the fade within the frame budget at 2x;
* the catcher: top layer, over the whole output, below the panel; a press on it closes the panel. It is
  `WindowBackground::INPUT_ONLY`: a transparent single pixel the compositor stretches over the output, so it costs no
  drawing or upload at any output size or scale (a compositor without one-pixel buffers gets a drawn transparent frame);
* while the panel is hidden, one transparent point that takes no input, so the process always has a mapped surface.

Opening the panel closes the hidden window and opens the panel and the catcher; closing it does the reverse. A panel or catcher
window the server closes (its output went away) closes the panel; a hidden window it closes is replaced by a new one
(`Message::Remap`, a new window key), which the server puts on an output still there, so the process keeps running. The panel's
margin is first guessed from the first output's height, since the output the server picks is not known before the configure; the
catcher's configure gives the height of the output both were put on, and `LauncherRoles` corrects the margin
(`LayerSurface::set_margin`) when it differs, before the panel's first frame commits it.

**Navigation.** `nav` holds which view is up (hidden, search or the browser), the nested state over it (the enlarged preview,
the view options menu) and where the keyboard is. Escape steps back one level at a time: a nested state closes and the keyboard
goes back to what opened it; in search the keyboard leaves the result list for the field; a typed query is cleared; in the browser
a category filter is cleared; the browser folds into the search field; the search field closes. The browser toggles: its shortcut
(Primary+1) or the Dock's request opens it from hidden, unfolds it from the search field (the query carries over) and closes it
while it is up. A dismissal closes whatever is up, nested states included.

**Application browser.** Under the field, which filters it, sit a row of category filters (each application in the first
category its desktop entry's `Categories` maps to, `browse::Category::of`, or Other): 26 pt pills with 8.5 pt corners, a light lift over the
glass with their names in the secondary text colour, as wide as their names and spread so the row always spans the browser,
narrowing their gaps and padding before a name would be cut (`BrowseLayout::filter_rects`). The applications sit in 7 columns
with 67 pt icons: a row of
the recently used ones (from the launch history), a separator, then every application by name. View Options at the header's end
switches between the grid and a list, which the process remembers while it runs. Its menu is a Kit `Menu` in a grabbing popup of
the panel's window, below the button and free of the panel's bounds, on the menu material every menu of the shell uses; it holds
the keyboard while open, and the panel stays open meanwhile. A leave of the panel's window is judged in the order the window
system reports it, so one that follows the menu's dismissal (something else took the keyboard) still closes the panel. A press on the button opens it without moving the keys there, so a choice made with the pointer leaves
no focus ring; one made after Tab reached the button leaves the ring on it. The keyboard stays in the field: the arrows,
Page Up/Down, Home and End move the selection, Return opens it, Tab and Shift+Tab move between the applications, the filters and
View Options, and the selection is spoken through a polite live region. The content scrolls under a scroll indicator that shows
while it moves and for a moment after the browser opens (`browse::SCROLLER_LINGER`, `SCROLLER_FADE`), and keeps the side inset (`BrowseLayout::CONTENT_BOTTOM`) under its
last row and under a row the keys scroll to. A filter starts the selection and the scroll over; clearing it puts
back the scroll the unfiltered view had. The browser reopens at the top with nothing selected. It is 844 × 577 pt, and on a short
output it leaves 96 pt free below (`kit::BROWSE_BOTTOM_ROOM`, at least 300 pt tall). Icons load at 32, 60, 120 and 180 px,
the generic application icon standing in for a missing one. They are looked up, read and decoded off the loop, on a
thread of the host's with its own icon lookup (`Host::app_icon`, answered as `Message::AppIcon`), nearest the view
first; at most 14 wait at once (`kit::MAX_APP_ICON_LOADS`), and each that arrives makes room for the next. It launches with an activation token like a search result, when the press on it lets go. A press that moves 4 pt first drags the application out instead: its desktop file as a `text/uri-list`, its icon under the pointer where it was held. The dock keeps it where it lands and the desktop takes a copy; the catcher steps aside while it is dragged, and a drop something took closes the panel.

**Mode buttons and the files scope.** Beside the plain search bar stand round buttons (`modes`, `view::modes`) that switch
to a scope: the applications (Primary+1) and, while files are searched, the files (Primary+2). Each is a 55 pt circle of the
panel's glass, 64 pt apart, joined to the field's glass where they come close. They bud out of the field's end as one shape
and pinch apart when the panel opens, going a few points past their places on a slow spring, and merge back into the field
when anything opens below it. Hovering one shows its shortcut as two chips in the field's trailing end; Tab moves the keys
to them, Left and Right along them, Return or Space press, Escape gives the keys back. Each is a button with a name and its
shortcut for assistive technology. The files scope (`scope`, `view::scope`, 640 × 547 pt, 28.5 pt corners) has three
filters of equal width (recent, documents, images), a row of the most recently changed files, a bold heading and a grid of
5 columns 120 pt apart with 67 pt thumbnails and names of up to two lines. The file index is searched off the loop
(`ScopeRequest`, `Host::scope_search`) and the grid shows no spinner while it waits; thumbnails load on a thread of the host's
(`Host::file_thumb`), the type's icon standing in until they arrive. The arrows, Home, End, Page Up and Down move the
selection, Return opens it, Tab moves between the files and the filters, Escape clears the query, then the filter, then folds
the scope into the plain bar. Reopening keeps the previous query, shown selected so typing replaces it; a kept query filters
and searches nothing until it is edited.

**Motion.** `motion` presents the panel at a time passed in: its shape, opacity, scale about its centre, the scope
content's opacity, how blurred it is, which header shows and the mode buttons. Opening starts the enclosure at 1.1 times
its size and settles it to 1 on an underdamped spring (0.30 s response, damping 0.74) while it fades in over 0.1 s (a
scope's content following 0.08 s later over 0.10 s); the plain search bar only widens from that scale, its height staying
57 pt. The mode buttons bud out on their own spring and their symbols sharpen 0.25 s later; after a scope folds back into
the bar they wait 1.1 s before budding again (`motion::RESPLIT_DELAY`). Unfolding opens the shape along a spring (0.27 s, damping 0.83 growing;
0.295 s, 0.86 shrinking) with the content fading in 0.06 s later over 0.12 s, and into the files scope the width also dips
by 123 pt and comes back, the header switching and the growth starting 0.09 s after the dip (`motion::FILES_GROW_DELAY`); folding fades the content out over 0.05 s, then contracts the shape along the spring; closing takes
0.1 s: the enclosure grows 2 %, its content blurs and everything fades. A transition under way turns from where it is with its
velocity. Only the enclosure's rect scales; the content keeps its layout and is centred in it. Once the spring is past its
settling time, or within 1/64 pt of its target for a frame, the target itself is presented, so a later repaint never reshapes the
glass. Reduce Motion puts every shape where it heads at once and keeps only the fades.

**Slow results.** The file search runs off the loop (`Host::search`). Each query starts a new generation (`providers::Tracker`)
whose cancel token the worker checks; a reply for an older generation is dropped, and a provider that has not answered within
0.5 s times out. With no result to show the panel says why in a status row: still searching, no results, or that some results
could not be searched (a failure or a timeout), never an empty answer for a failure. The row shows at once but is said aloud
only once the query has stood 0.5 s (`kit::STATUS_SETTLE`, the providers' deadline; `Message::Settled`): providers still
pending then time out first, so a search that answers in time says nothing before its results, typing says nothing between
keys, and one status is said per settled query at most, by a live region of its own (`StatusAnnouncer`; a failure
assertively, the rest politely). The same status is never said twice for the same query while the panel is open, even when
the query is edited away and typed again (the last 32 are remembered). The browser's "No Results" follows the same rule:
its selection announcer says it only once the filter (query and category) has stood 0.5 s with nothing to show
(`Message::BrowseSettled`), at most once per settled filter, and never again for the same filter while the panel is open.
A provider still pending when the query settles has passed its deadline, and its late answer is dropped, so what is said
then is that some results could not be searched, which is what the row shows too. Space on a file
result enlarges its preview (Escape returns to the list); a preview that has not arrived 1.5 s later (`kit::PREVIEW_TIMEOUT`)
says there is none, and one that arrives late still shows.

**Input and dismissal.** The query is Kit's `BareField`, a text field without chrome set at 26 pt in a 57 pt bar, its
text starting 63 pt in, after a 23 pt search symbol, which has the keyboard while the panel is open. An empty field lists
nothing. A kind token anywhere in the query (`/apps`, `/files`, `/folders`, `/calc`, `/web`; `refine`) keeps only the results
of its kind, and Tab after a kind's name alone turns it into the token. Up in an empty field recalls the last searches that
opened something (newest first, at most 20, shown selected). Holding the Primary modifier shows the selected file's location
in the preview pane. The panel's own keys never
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

**Opening an application.** Choosing an application, in the browser or among the search results (its Open action),
opens it as a click on its Dock tile does; a desktop action (a new private window, say) always runs its own command.
`Launcher::activate` gives `Activation::OpenApp` for it, and the token is asked for as above. When the token arrives the host
asks the server to bring the application forward (`Host::bring_forward`: `cupertino_session_v1.activate_app` with the desktop
file ID, shell version 20), and the server answers with `app_activation` (`Message::Opened`):

- `activated`: it showed a window and the server brought it forward through the same path as the Dock's `activate_app`: the
  frontmost window takes the focus, a minimised window is restored when it has no other, a hidden application is shown, and
  nothing is started.
- `resident`: a process of it runs without a window. When the desktop file says `DBusActivatable=true` the host calls
  `org.freedesktop.Application.Activate` with the token (`cupertino_apps::activate`, the Dock's call, bounded by 3 s); when
  that is not answered, or the entry is not D-Bus activatable, the entry is started again with the token and a running
  instance opens a window.
- `not_running`: the entry is started with the token, as before.

With a server older than shell version 20, or without the shell global (the launcher has the Session role by default), the
application is started at once, as before. The server matches the desktop file ID to an application the way the Dock does:
its application id, or the desktop entry of its application id (by ID, `StartupWMClass` or program), without regard to case.

Screenshot: `cupertino-server/output/launcher-open.png`.

## Protocols / D-Bus it speaks

The process owns `org.cupertino.Launcher1` at `/org/cupertino/Launcher1`. A second launcher cannot take the name and exits with
failure. The interface lives in `control` (`BUS_NAME`, `OBJECT_PATH`, `serve`). It also speaks `cupertino_session_v1` (from
shell version 20 its `activate_app` request, which needs the Session role the server gives the launcher's connection and
checks on every request) and `xdg-activation` to the server, and calls `org.freedesktop.Application.Activate` on a resident
D-Bus activatable application.

| Method | Effect |
|---|---|
| `Toggle` | Shows the search field or hides whatever is up. The server calls it for `Super+Space`. Only the server (the process's parent) and the launcher itself may: any other caller is refused with `org.cupertino.Error.UntrustedCaller` (`AnonymousCaller`, `FlatpakCaller`, `SnapCaller`), `cupertino_peer::check_call`. |
| `ToggleApplications` | Opens the application browser, unfolds the search field into it, or closes it. The server calls it for the Dock's app-grid action; the same callers are allowed. |

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
| Material | large-panel glass (the compositor's shadow: 28 pt blur, 16 pt below, black at 0.5 dark or 0.22 light); a 0.5 pt edge, black at 0.55 or 0.12; without effects the menu fill and a 40 pt shadow, 16 pt below |
| Position | horizontally centred, the panel's top edge at 1/5 of the output height |
| Input | a 57 pt bar, a 23 pt search symbol 23 pt in, the query at 26 pt without chrome starting 63 pt in; an empty field lists nothing |
| Results list / preview | 288 pt list, a separator, the preview in the rest with 16 pt padding |
| Separators | over the glass a 1 pt line of white at 0.175 (dark) or black at 0.1 (light); the theme's separator colour over the opaque fill and with Increase Contrast |
| Preview | picture at most 300 × 136 pt (6 pt corners), icon 64 pt, name 15 pt semibold, details 11 pt with labels ending at the pane's middle, value 28 pt; the pane takes the width the list leaves |
| Action buttons | 26 pt capsules, 4 pt apart, at most 4 shown around the chosen one |
| Result icon, selection radius, cell inset | 32 pt, 12 pt continuous, 12 pt |
| Selected-row fill | the theme's secondary selected-control fill, text keeps its label colours, no animation |
| Keyboard | no wrap, Up on the first result leaves the list, Primary+Up/Down jump sections, Backspace/Delete/Esc in the list return to the field, Esc clears then closes, Tab/Shift+Tab cycle the actions, Left/Right step them in the list, Primary+Return/R/B/I/Y/K/L/D/,/H commands; Up in an empty field recalls searches that opened something, Tab after a kind's name scopes to it, `/apps` `/files` `/folders` `/calc` `/web` tokens keep one kind, Primary held shows a file's location |
| Top hit | one item, moved out of its section; a calculation or conversion leads instead |
| Conversions | length, mass, US customary volume, area, time (a Julian year), data (decimal and binary), speed, temperature, angle; no currencies; 6 significant digits |
| Section limits for hosts | 3 rows per section, 5 sections before "show more", 3 suggestions |
| Category defaults | `engine::DEFAULT_CATEGORY_ORDER` (enablement list; runtime order is relevance driven) |
| Motion | open from 1.1 scale about the centre, spring 0.30 s damping 0.74 (the plain bar widens only), fade 0.10 s; content 0.08 s later over 0.10 s; shape spring 0.27 s / 0.83 growing and 0.295 s / 0.86 shrinking; unfolding content 0.06 s later over 0.12 s; fold fade 0.05 s; close 0.10 s (grows 2 %, blurs, fades); mode buttons bud on springs of 0.305 s / 0.68 (field) and 0.575 s / 0.69 (buttons), symbols sharpening 0.25 s later and budding again 1.1 s after a fold; into the files scope the growth 0.09 s after the width's dip; Reduce Motion: fades only |
| Shape grid | settled widths and heights on multiples of 4 pt |
| Browser | 844 × 577 pt, 28.5 pt corners, 7 columns of 102 pt rows (67 pt icons, 13 pt labels), 44 pt list rows (32 pt icons), 26 pt category filters (15 pt regular, 8.5 pt corners, the row always filled), 96 pt kept free below on a short output |
| Deadlines | file search 0.5 s, enlarged preview 1.5 s |
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
| `panel` | `Panel`: query editing, selection, scrolling; `takes_key` |
| `nav` | `Nav`: the view, the nested state and the keyboard's place; what Escape, the toggles and a dismissal do |
| `browse` | `Browser`: the application browser's model, categories, grid and list geometry, keyboard, scrolling and selection |
| `motion` | `Motion`: the opening, unfolding, folding and closing transitions, sampled at a time passed in |
| `providers` | `Tracker`: generations, cancel tokens, deadlines and the status of results that arrive late |
| `view` | the panel, catcher and hidden windows on CupertinoKit, panel metrics, `panel_key`; `view::browse` the browser's nodes |
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

## Decisions

* The launcher's browser is the desktop's only application browser: the Dock's app-grid action opens it (`ToggleApplications`)
  and the Dock's own grid is gone.
* An application chosen in the launcher opens as its Dock tile does (*Opening an application*). The server does the raising
  (a session-object request rather than a window list in the launcher), so the launcher learns nothing about windows beyond
  that one answer, and an ordinary client, which never sees the shell global, gains nothing. The token is asked for before
  the server is, while the panel's window still exists, so a launch after the answer still takes the focus; a token the
  answer makes unnecessary is left to expire.
* Escape clears a category filter before it folds the browser; the arrangement is remembered while the process runs; the browser
  reopens at the top with nothing selected; applications whose entry maps to no category are under Other.
* The keyboard stays in the query field while browsing, and the selection is spoken through a live region.
* A preview that arrives after its deadline still shows.

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
CUPERTINO_REPAINT_DIR=out cargo test -p cupertino-launcher --release frames::frame_cost_report -- --ignored --nocapture
CUPERTINO_REPAINT_OUTPUT=1512x982 CUPERTINO_REPAINT_SCALES=1,2 CUPERTINO_REPAINT_DIR=out cargo test -p cupertino-launcher --release frames::frame_cost_report -- --ignored --nocapture   # on another output
cargo test -p cupertino-launcher --release frames::   # the frame budgets, in CPU time, hold only in an optimised build
cargo test -p cupertino-launcher --release frames::every_frame -- --ignored   # the same budgets in wall time, on an idle machine
```

| Item | Proves or shows |
|---|---|
| `tests/control.rs` | The `Toggle` and `ToggleApplications` methods against a private `dbus-daemon`, that a second launcher cannot take the name, and that another process (a `dbus-send` the test starts) is refused as `UntrustedCaller`. |
| `cupertino-server/tests/shell_processes.rs` | The real process (`--no-files`) connects to the real server, maps its hidden window at start and keeps running; the Dock's app-grid action opens the browser, and Escape folds it into the search field and then closes it. |
| `src/view/tests.rs` | The windows run headless on Kit's test driver: the search field, the results list and keyboard navigation against the Inspector goldens in `tests/goldens/` and the screen-reader readings in `tests/fixtures/screen-reader/`; launching, the clipboard, hover, click and dismissal; opening an application that runs (brought forward from the search and the browser, nothing started), one that does not (started with its token once the server says so), a resident one (asked over D-Bus, started again when it does not answer or is not D-Bus activatable) and a desktop action (never asks the server); the preview arriving (and a stale one dropped), Reveal in Files, choosing an action with the arrows, a conversion; the browser opened by the Dock's request, the arrows, typing and category filters, the list arrangement (goldens `browser-grid` and `browser-list`), the view options as a popup below their button on the menu material, the panel kept open while their menu holds the keyboard and closed by a leave after it, and the focus ring a pointer choice does not leave, the shortcut that unfolds the search field, a late file search dropped and one past its deadline, the enlarged preview and its timeout, Reduce Motion, and the window following the browser on a moving clock. |
| `src/view/tests/frames.rs` | On a clock that moves a 60 Hz frame at a time: at 1.25, 1.5 and 1.75, the arrows and the pointer in the browser reshape no glass and repaint no whole window, and a transition reshapes it at most once a frame; at 1 to 2, a settled panel's glass ends on whole device pixels. `frame_cost_report` (ignored) times each interaction's frames with the CPU renderer and counts its whole repaints and glass changes, the Dock opening the browser over 200 applications among them. `opening_the_browser_over_200_applications_keeps_icon_decoding_off_the_frames`: with each icon costing 3 ms of CPU to decode on the host's thread, every icon near the view is asked once, at most 14 wait at once, asking costs the loop under a millisecond of CPU in all, and the frames showing the icons arrive within the 8 ms frame budget in the loop thread's CPU time; the first frame, which lays the browser out, is held to the budget in an optimised build (2.7 ms in release at 1x; about 54 ms unoptimised). It measures CPU time, not wall time, so a machine busy with other work does not fail it: under a busy loop on every processor the arriving frames' median took about 3 ms of CPU against 2 ms on a quiet machine (unoptimised), while their wall time reached 15 ms. `every_frame_of_the_panel_s_transitions_fits_the_budget_in_an_optimised_build`: in an optimised build, drawn with the CPU renderer, every frame of the search field opening and closing, typing, unfolding and folding, the arrows and the pointer in the browser, the Dock opening the browser and Escape folding it, and the Dock opening it over 200 applications, is under 8 ms at 1x and 16 ms at 2x, the first frame of a new window included; a run with a frame over the budget is run again afresh, up to three runs, and the test fails when every run has one, so a frame doing several times its work fails it while a build that comes and goes beside it does not; it measures the CPU time the loop's thread spends on each frame, which other work on the machine raises only by what sharing caches and cores costs (up to a quarter on the slowest frames under a busy loop on every processor), where wall time is stretched two to six times; other tests drawing on every processor raise it past the budget, so `.config/nextest.toml` runs this test and the icon test with no other test beside them; unoptimised it measures nothing. `every_frame_of_the_panel_s_transitions_fits_the_budget_in_wall_time_on_an_idle_machine` (ignored) holds the same frames to the same budgets in wall time, run by hand in the release profile on an idle machine. In release, on a 1000×700 pt output, the slowest frames are about 2.5 ms at 1x (a typed key) and 4.9 ms at 2x (the first frame of the Dock opening the browser over 200 applications); on a 1512×982 pt output (`CUPERTINO_REPAINT_OUTPUT`) about 2.6 ms and 7 ms (`frame_cost_report`). The catcher adds nothing to a first frame at any output size: it is a stretched pixel, where a drawn transparent catcher cost 1.5 to 8 ms more on the first frame of a window. |
| `src/nav.rs`, `src/browse/tests.rs`, `src/motion.rs`, `src/providers.rs` | The navigation table (Escape order, toggles, dismissal), the browser's model (categories, grid and list keys, Tab zones, scroll kept across a filter, the bottom inset under a revealed row, selection kept by application, 5 000 applications), the transitions and their interruption, generations, cancellation and deadlines. |
| `cargo test` | Matching, ranking, frecency, calculator (division by zero, huge numbers, invalid input), conversions, preview words and dates, file search, panel state and action traversal, that `takes_key` agrees with `handle_key`, and the `app` process code. |
| `cargo bench` | Scores 5 000 synthetic applications: about 1–3 ms per query in a release build (14 ms to build the index), calculator 0.6 µs. |
