# cupertino-launcher

The launcher ("search panel") of the Cupertino desktop. It stands in for the design's launcher. It does desktop
entry indexing, tiered fuzzy matching with persisted frecency, a calculator and unit conversions (its own parser, no network), a
bounded file search, sectioned results beside a preview of the selected one with its actions, and keyboard navigation. In the same
panel it shows the application browser: every application, filtered by category and the query, as a grid or a list, which the Dock
opens. It draws its panel with CupertinoKit.

The crate has no Smithay dependency. The engine and the panel state are separate from the window, so a host can run queries on
any thread. The `component` module is the host for the desktop: a component of the shell host whose windows sit on layer
surfaces. Panel geometry, keyboard semantics, the top-hit rule and the section limits follow the analysis of the design's launcher.
Ranking scores, row heights and the other items marked approximate are authored and configurable. Colours, type, controls and the
material come from Kit's theme.

## Running

The launcher is a component of the shell host (`cupertino-shell`, entry `launcher` of its `COMPONENTS`): the
`Component` `component::Launcher`, a library with no binary of its own. The host runs it on a thread of its own with its
own connection to the server and to the session bus, and starts it again alone when it fails; the server starts the host.
With the desktop's nested server (`cargo run -p cupertino-server -- --winit --spawn foot`) `Super+Space` opens the
panel.

| File | Default | Effect |
|---|---|---|
| `launcher.conf` (`$CUPERTINO_LAUNCHER_CONF`) | every kind on | Read as the component starts and again at each change (watched): which kinds of result it lists (applications, folders, files, calculator, actions), whether it marks a top hit, and the folders the file search leaves out, applied the next time the panel opens. |
| `~/.local/share/cupertino/launcher.json` (`$XDG_DATA_HOME/cupertino/launcher.json`) | none | The launch history (see Persistence). |

Entries with `Terminal=true` run in the first installed of `xdg-terminal-exec`, `$TERMINAL`, `foot`, `alacritty`, `kitty`,
`wezterm` and `xterm`, as `PROGRAM -e …` (`xdg-terminal-exec` takes the command without `-e`). The home folder is indexed
for file results the first time the panel shows, unless `launcher.conf` turns files off.

**What a restart loses.** The panel's query, its results, the browser's arrangement and scroll positions, and a launch made
in the moment before whose history write had not finished. A shown panel goes with the component that failed, and the
keyboard returns to the window that had it. The launch history, `launcher.conf`, the applications and the file index are
read again as the component starts.

Kit follows the session's appearance and display options: Reduce Transparency and Increase Contrast give the panel the opaque
fill and the high-contrast colours.

## How it works

**Windows.** The component shows two windows while the panel is up and none while it is hidden, each on a layer surface
(namespace `cupertino-launcher`) that `Program::surface` describes, both transient: the server dismisses them when the
component stops. They go on the output a `TogglePanel` publication of the host bus names, else on none, so the server puts
them on its active output, the one the person works on:

* the panel: overlay layer, anchored to the top only (so centred horizontally), the panel's top edge at 203/982 of the output's
  height, with exclusive keyboard interactivity while the panel is shown and none while it closes. The enclosure (the panel's shape: `view::compact_shape`, `view::search_shape`,
  `view::browse_shape`) sits in a window with room for its shadow on every side (`view::window_extent`). While the shape moves,
  the window holds both the shape presented and the one it heads for, and once it settles the window is exactly the target's, so
  it never cuts the enclosure off mid-way nor keeps room it no longer needs. Where the window system shows surface layers
  (and Reduce Motion is off) the window instead keeps the largest size the panel takes for as long as the panel shows
  (`Model::window_held`, `Model::largest_extent`: both scopes and the tallest list of results, each with its shape's
  overshoot, on the 4 pt grid and within the output's room below the panel), so a fold or unfold the compositor runs never
  waits for the window to be configured again; hidden, the panel has no window. Its input region is the enclosure and the
  mode buttons (`view::enclosure_rect`) while the panel is open and nothing while it closes, so a press beside the panel
  reaches the catcher. When the window's size changes the surface's size follows with the window's next frame
  (`Program::surface` gives `Model::extent`), and the server's configure gives Kit the new size. A settled shape's width and height are multiples of 4 pt (`view::SHAPE_GRID`), so the glass
  ends on whole device pixels at 1, 1.25, 1.5, 1.75 and 2. The window is transparent: the enclosure is large-panel glass
  (`Glass::large_panel`, the regular recipe with its bend, rim and key and fill light, blurred twice as wide and with a face of its
  own: in the dark appearance its tones pressed together, a lower white, a lifted black and a little less saturation; in the
  light one a bright, more saturated sheet, black behind coming through as a light grey and a dark scene as a light tint of
  its own hue, with a bright line just inside the edge; so the panel reads as one calm sheet over any wallpaper) with the
  panel's corner radius and a thin dark edge, named as the search results variant at its own size while the panel holds
  more than its field, and at the large size while it is the plain search bar (the mode buttons beside it too;
  `view::enclosure_variant`, `view::BUTTON_VARIANT`): the system names the bar a platter of the regular glass, but shows
  it as the same milky sheet with a thin dark rim and a soft shadow, where regular glass would be a clear one without
  a shadow. Neither adapts to what is behind it; the compositor draws the glass's soft shadow outside its shape. Without the desktop's effects it paints `Surface::Menu`'s
  fill and a shadow of its own instead. While the browser shows, the panel fades by giving the glass, the search bar and
  the browser's body each their opacity rather than compositing the whole window as one group, which keeps every frame of
  the fade within the frame budget at 2x. Where the window system has surface layers and the desktop's effects, a panel
  at rest is drawn as compositor layers (`view::layers`): the enclosure's glass (a layer that draws only its glass, so the
  window stands on none), its content, its dark edge (a layer on a 4 pt grid around the enclosure with room for the
  stroke, centred on the point the panel grows about), and each mode button's glass and face. Closing it then hands the
  close to the compositor (`view::layers::close_changes`): the glass grows to the close's end rect and fades, the edge
  follows it and blurs, the content blurs to a deviation of 4 pt and fades, all on the close's ease-out over 0.1 s, and
  the panel draws no frame until the compositor is done and the window goes: the panel takes every key meanwhile and the
  query field holds its caret (`BareField::hold_caret`), so neither a key nor the caret's blink draws the content
  again. At each frame the panel draws of its own close, the compositor's layers at the same moment, from the curves it
  sent, are its pixels. While the panel moves, under Reduce Motion and
  without the capability it draws its own close. The compositor can also draw the edge along the glass itself
  (`view::layers::glass_edge` in a glass layer's look, `SurfaceLayerLook::edge`), the panel's own edge along capsules and
  straight sides; a scope's open from hidden can be handed over (`view::layers::open_changes`, the layers drawn once at
  rest and moved on the open's spring from where it starts, `Model::hand_opens`): part way its glass is the
  panel's own, its content the panel's pixels within 1 of 255 (while the panel scales its content keeps its size, cut
  off by the glass, `Overflow::Crop`, and lies on whole device pixels as one picture, `Widget::on_pixels`, in the
  panel's drawing as in the compositor's) and its edge the rest's stroke scaled, a half-point line that the scale puts
  between device pixels, within two fifths of the line's opacity of the panel's own;
  the search field's open with its mode buttons can be too (`view::layers::field_open_changes`, every value through
  the panel's own, the field's glass carrying the edge the compositor draws round each shape cut by the others while
  the shapes join, each button's symbol fading in and sharpening through its face layer's opacity and content blur;
  `Model::hand_field_opens`). An open handed over ends when the compositor tells the panel's layers came to rest
  (`WindowEvent::LayersSettled`, heard through `FrameClock::on_layers_settled`), in the frame that shows their last
  values, not on the panel's own clock, which starts a frame or more before the compositor first shows the open; a
  timer a second past the open's end takes it back if no word comes, and a timer of an earlier open ends nothing. The
  field holds its caret through it, and a paint for any other reason draws the caret as it was shown; through the
  search field's open the caret is a layer of its own over the query row's (`layers::CARET_LAYER`), its opacity the
  content's fade times the field's own blink from the open's start (`BareField::blink_start`), until the layers rest
  or a key shows the caret again;
* the catcher: top layer, over the whole output, below the panel; a press on it closes the panel. It is
  `WindowBackground::INPUT_ONLY`: a transparent single pixel the compositor stretches over the output, so it costs no
  drawing or upload at any output size or scale (a compositor without one-pixel buffers gets a drawn transparent frame);
A panel or catcher window the server closes (its output went away) hides the panel at once (`Message::Gone`), and so does
the session lock (`Message::Locked`). The panel's margin and the browser's height come from the size of the output the panel
opened on; when the server picks the output (no `TogglePanel` output), from its only output, else the first one it
described, since the component is not told which output the server chose.

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
the generic application icon standing in for a missing one. They are looked up, read and decoded off the UI thread, as tasks of Kit's I/O pool
(`Host::app_icon`, answered as `Message::AppIcon`), nearest the view first; at most 14 wait at once (`kit::MAX_APP_ICON_LOADS`), and each that arrives makes room for the next. It launches with an activation token like a search result, when the press on it lets go. A press that moves 4 pt first drags the application out instead: its desktop file as a `text/uri-list`, its icon under the pointer where it was held. The dock keeps it where it lands and the desktop takes a copy; the catcher steps aside while it is dragged, and a drop something took closes the panel.

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
`cupertino-icons`' `thumbnail`), the first lines of a text file or its type's icon, then its name, its type in words, its size (or a
folder's item count), when it changed and its folder; an application's icon at the pane's size and its description; a
calculation's or a conversion's value set large over its expression. The host reads a file's preview as a task of Kit's I/O
pool, and a read that a newer request replaced before it started is skipped (`Host::preview`, `Message::Previewed`), so
moving through the results never piles reads up; it reads at most 4 KiB of text, only from a regular file opened without waiting
(a pipe or device never holds it), and pictures through the thumbnailer's size and decode limits. A preview that arrives
for a result no longer selected is dropped. Below it the
result's actions are capsule buttons, the chosen one filled in the accent colour (`Launcher::actions`): Open, Reveal in Files and
Copy Path for a file; Open and Copy Path for a folder; Open and the entry's desktop actions for an application; Copy for a value;
Search for the web search. Tab and Shift+Tab move through them round and round; with the list focused, Left and Right move
through them and stop at the ends; Return carries out the chosen one. Moving the selection chooses the result's own action
again. The panel also closes when its window loses the keyboard (another
window took it; not its view options menu or the query field's own menu of Cut, Copy, Paste and Select All, popups of
the panel that give the keyboard back; when another surface takes the keyboard from the field's menu the panel closes
with the menu), on a press outside the panel, after a launch, and when the session locks (the shell host's `Event::Session`, as `Message::Locked`). The launch history is written when
the panel closes, as a task of the I/O pool (`Host::save_history`).

**Launching.** `Activation::Run` asks Kit for an `xdg-activation` token for the panel window (`Command::ActivationToken`, with the
entry's desktop id), and starts the command with `XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID` when the token arrives. A copy
result (a value, or a path) goes on the system clipboard through `Command::SetClipboard`. Reveal in Files (the action, or
Primary+R) opens the folder holding a file, or a folder itself, in the session's file manager through its desktop entry
(`engine::FILE_MANAGER_ID`, with an activation token), else with `xdg-open`; the file manager opens folders, so the file itself is
not selected. Web search runs `xdg-open`. The other panel commands are logged as not provided.

**Opening an application.** Choosing an application, in the browser or among the search results (its Open action),
opens it as a click on its Dock tile does; a desktop action (a new private window, say) always runs its own command.
`Launcher::activate` gives `Activation::OpenApp` for it, and the token is asked for as above. When the token arrives the host
asks the server to bring the application forward (`Host::bring_forward`: `cupertino_kit_shell::session::activate_app` with the desktop
file ID), and the server answers with `app_activation` (`Message::Opened`):

- `activated`: it showed a window and the server brought it forward through the same path as the Dock's `activate_app`: the
  frontmost window takes the focus, a minimised window is restored when it has no other, a hidden application is shown, and
  nothing is started.
- `resident`: a process of it runs without a window. When the desktop file says `DBusActivatable=true` the host calls
  `org.freedesktop.Application.Activate` with the token (`cupertino_apps::activate`, the Dock's call, bounded by 3 s); when
  that is not answered, or the entry is not D-Bus activatable, the entry is started again with the token and a running
  instance opens a window.
- `not_running`: the entry is started with the token, as before.

The server matches the desktop file ID to an application the way the Dock does:
its application id, or the desktop entry of its application id (by ID, `StartupWMClass` or program), without regard to case.

Screenshot: `cupertino-server/output/launcher-open.png`.

## Protocols / D-Bus it speaks

The component owns `org.cupertino.Launcher1` at `/org/cupertino/Launcher1` on its own session bus connection, the one the
server knows as the launcher's (D20). When another connection holds the name the component exits with failure and the host
starts it again. The interface lives in `control` (`BUS_NAME`, `OBJECT_PATH`, `serve`). Through the shell host it hears
`TogglePanel` for the launcher and the applications and publishes `PanelShown` on the host bus, asks the server to bring an
application forward, and asks for `xdg-activation` tokens; it calls `org.freedesktop.Application.Activate` on a resident
D-Bus activatable application.

| Method | Effect |
|---|---|
| `Toggle` | Shows the search field or hides whatever is up. The server calls it for `Super+Space`, from the connection that owns `org.cupertino.Session1`. Only that connection may (D20; the check asks the bus daemon who owns the name and reads nothing else): any other caller, another connection of the server's process and the components of the host included, is refused with `org.cupertino.Error.UntrustedCaller` (`cupertino_peer::CallerError`). |
| `ToggleApplications` | Opens the application browser, unfolds the search field into it, or closes it. The server calls it for the Dock's app-grid action; the same caller alone is allowed. |

A call from a shell (`gdbus call ... --method org.cupertino.Launcher1.Toggle`) is refused for that reason; press
`Super+Space`, or pass `--key 2:Super+space` to a scripted server.

## Icons

Application results show the entry's own `Icon=` through Kit's icons (an asset pack's icon of that name, else the host's
icon themes or an absolute path), and the asset `application.generic` when it has none. Folder results are the asset `folder`, file results `document.generic` in the list
(the preview types the selected file with the MIME database and shows its type's icon through `cupertino_kit::file_icons`); they
are loaded at the icon size for scales 1, 2 and 3 and shown as Kit images, which draw the rendition that covers the output's scale. They
are looked up, read and decoded off the UI thread, as tasks of Kit's I/O pool (`Host::icon`, answered as `Message::ResultIcon`), and a row is
blank until its icon arrives; the preview's application icon loads there too (`Message::Previewed`, or `Message::NoPreview` when the
application has no icon of its own and the preview shows only the result). The web
search row is Kit's `symbol.magnifyingglass` symbol in the secondary label colour, as is the symbol in front of the query. The
calculator and conversion rows have no icon. A missing asset is drawn as Kit's placeholder.

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
| `preview` | what the preview shows of a file: its type in words, size, change date, folder, text excerpt |
| `files` | bounded breadth-first walker, ignore rules, in-memory name index and search |
| `engine` | `Launcher`: providers → `Results { sections }`; `actions` of a result; `activate` and `activate_action` → `Activation` |
| `panel` | `Panel`: query editing, selection, scrolling; `takes_key` |
| `nav` | `Nav`: the view, the nested state and the keyboard's place; what Escape, the toggles and a dismissal do |
| `browse` | `Browser`: the application browser's model, categories, grid and list geometry, keyboard, scrolling and selection |
| `motion` | `Motion`: the opening, unfolding, folding and closing transitions, sampled at a time passed in |
| `providers` | `Tracker`: generations, cancel tokens, deadlines and the status of results that arrive late |
| `view` | the panel and catcher windows on CupertinoKit, panel metrics, `panel_key`; `view::browse` the browser's nodes |
| `kit` | `Model`, `Message`, `update` and `view`: the launcher as a Kit application, the session behind the `Host` trait |
| `component` | `Launcher`, the component of the shell host: its surfaces, the host bus, `ComponentHost` (the `Host` on Kit's task pools) |
| `control` | `org.cupertino.Launcher1`, served on the component's connection |

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

**Applications installed while it runs.** The process does not index once: it reads the applications through
`cupertino_apps::catalogue` and follows the XDG application folders (the person's and each system one, with their
subfolders, and a folder that is made later) from a task of its loop. After a burst of file events goes quiet (250 ms, at
most 2 s after its first event) a task on Kit's I/O pool reads the folders again, with the same visibility rules and the person's
entries over the system's, and when the applications differ it sends them all as `Message::Installed`. The model then
swaps the index: the results of the query on screen are composed again with what the file search already found (the
query and the selected result stay), the open browser takes the new list (its filter, selection and scroll stay), and
the icon and preview of an application that changed are asked for again. Nothing is read from disk on the loop.

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
and 500 queries. The component reads it as it starts and writes it when the panel closes, both as tasks of Kit's I/O pool
(`Frecency::unsaved` hands the model's copy over).

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

## Examples and benchmarks

```
cargo run -p cupertino-launcher --release --example search -- [--files] <query>
cargo bench -p cupertino-launcher --bench matching
cargo bench -p cupertino-launcher --bench instructions   # the same matching in instruction counts, under Valgrind
```

`matching` scores 5 000 synthetic applications: about 1–3 ms per query in a release build (14 ms to build the index), calculator 0.6 µs. `instructions` counts the index of the same applications, seven queries and the scorer alone: about 110 million instructions to build the index, 36 to 61 million a query.
