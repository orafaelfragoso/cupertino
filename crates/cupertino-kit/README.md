# cupertino-kit

CupertinoKit, the Cupertino UI toolkit: the one crate applications depend on. Each module lists the toolkit items
an application names, one by one ([Public API](#public-api)), and it has a `prelude`. `typeahead` (`TypeAhead`) names the type-to-select of
`cupertino-ui-controls`, and `filepicker` (`Picker` and its `Mode`, `Key`, `Request`, `Listing` and `Outcome`, with the
family's `Entry`) the file chooser's model of `cupertino-ui-files`. The `test` feature, enabled only in
dev-dependencies, adds `kit::test`: the headless backend and what a driver is built from. MPL-2.0.

A node is glass when its `paint` calls `PaintCx::glass` (`Glass` and `GlassStyle` are in the `prelude`): the compositor
draws the glass under it, and nodes in the same container merge into one shape (`cupertino-ui-core`, *Glass*):

```rust
use cupertino_kit::core::PaintCx;
use cupertino_kit::prelude::Glass;

fn paint(cx: &mut PaintCx<'_>) {
    cx.glass(Glass::regular().radius(12.0).container(1, 6.0));
}
```

Several Kit instances can run in one process, each on a UI thread of its own (the components of a shell host). What
differs between them lives with each instance, never in a process-wide static: an `Instance` (`Instance::new(area)`)
is entered on a thread (`enter`, until the guard drops; `Instance::current` gives it), and the thread's diagnostic
events are recorded in its scope (`cupertino_diagnostic_events::Scope`: its panic is `<area>.panic`); each window
system keeps its own renderer choice, while the device and its losses are the process's. Every `app::run` runs for the instance the calling thread
entered, else for the process's own (named by the program, its events the process's). The diagnostic emitter and the
panic hook stay one per process, installed by `main`. The installed theme data, the text system with the asset packs'
fonts, and the packs' icons are the process's too: the first instance to start (`app::environment` or any `app::run`)
reads them and every other starts on them (D5). The theme data never changes once read; the caches only take whole
entries that never change, under locks held just to look one up or put one in, so a panic in one instance leaves them
whole for the others, and their budgets (two generations of 512 shaped paragraphs, `SHARED_RASTER_BYTES` of glyph
rasters) are the whole process's. A program that draws installed applications' and files' icons itself reads them the
same way, from work on the I/O pool (`kit::shared`): the installed file types, the host icon themes' icons and the asset
packs' raster icons, each read or drawn by its first caller and never changed after, so an instance started again after
a panic finds them read.

Work that would hold up a UI thread goes to the process's pools as a command (`kit::task`): `task::cpu` on a pool of
up to a thread a core, `task::io` on a pool of up to `task::IO_THREADS` for blocking work with the deadline the caller
names; a pool starts a thread only when work finds every one it started busy.
Each runs for the instance that asked, its result comes back to that UI thread as a message, one instance runs at most
`task::IO_SHARE` I/O tasks at once, an I/O task still running past its deadline is reported once
(`toolkit.task.overdue`, with its instance, deadline and call site), and a task's panic is its instance's and is raised
again on the UI thread. `task::follow_settings` follows settings files (`cupertino_defaults::watch::ConfigWatcher`):
the UI loop only waits for each burst of changes to be quiet, and the files are read on the I/O pool (deadline
`task::SETTINGS_READ_DEADLINE`, 2 s). `kit::bus::connect` opens a D-Bus connection whose work the UI loop polls, with no executor
thread: the connection comes as one message and its close as another. `kit::time` reads the session's local time
(`local`, `now`) and turns a local date and time back into a moment (`from_local`), daylight saving time included; it
converts Unix seconds (`from_unix`, `to_unix`), formats a moment by a `strftime` pattern (`format`) and gives the time
format (`locale_time_format`), which says whether hours count to 12 or 24, both by the C library's current `LC_TIME`:
that locale is the whole process's and only `setlocale` sets it, so in a program that has not called it (no Cupertino
program does) both are the `C` locale's English and 24-hour `%H:%M:%S`, whatever `LANG` says.
`kit::account` reads the password database (`uid`, `current`, `by_uid`: the login name, the comment with its full name,
the home); `kit::host` the machine's host name and IPv4 addresses; `kit::process::harden` makes a process that holds a
typed password undumpable, with no core file; `kit::render::zeroed_pixels` hands over zeroed pixel memory a program fills
in place. They are the shell's safe way to these calls: shell crates forbid `unsafe` code.
`kit::spawn` starts a program for the instance the thread runs for, and the instance's UI loop reaps it on its first
turn after it ends (`test::each_turn`, which every `app::run` installs with `Runtime::with_each_turn`), reporting an
unsuccessful exit on standard error; no thread waits for it.

`app::run` and `app::options` start on the Cupertino theme from the installed theme data, and on
`ThemeTokens::neutral` when there is none or it is unusable (one line on stderr says why), so an application runs on any
compositor. `app::run` connects for the application's renderer class (`Application::RENDERER`), and its renderer factory
makes the GPU renderer once the process draws on the GPU. `app::run_on` and `app::run_on_with` run on a window system a
desktop shell set up with its surface roles (connected with `WaylandWindowSystem::connect_as` for its class);
`app::run_lock_screen_on_with` does the same for the lock screen, whose AT-SPI adapter the session's lock never mutes
(`AtspiAdapter::lock_screen`); `app::run_on_with_until` and `app::run_lock_screen_on_with_until` also end the run on the
turn a condition holds (`Runtime::with_exit_when`), as a host stops a component; and `app::run_on_with_appearance` passes the session's appearance through a function first, for a
shell surface that keeps the dark appearance in a light session. Every one of them keeps the text caret from blinking
(`caret_phase_ms` 0) while the session asks for a steady caret, from the first frame.
Every one of them reports scroll timing when `CUPERTINO_SCROLL_TIMING` names an absolute path:
one JSON line a pointer scroll event, appended, with its stages in nanoseconds of `CLOCK_MONOTONIC` (`input`, the
window system's timestamp; `handled`; `updated`, the model update and view, or `null`; `submitted`, the frame that drew
it; `presented`, the compositor's time for that frame, or `null` when it was dropped). Without the variable no timing
is kept (`Runtime::with_scroll_timing`). They report frame timing when `CUPERTINO_FRAME_TIMING`
names an absolute path: one JSON line a presented frame, appended (every process of a session
may share the file), with `pid`, `program`, `renderer` (`cpu` or `gpu`, what the process draws with now), `start` in nanoseconds of `CLOCK_MONOTONIC`, and the nanoseconds of each
phase: `update`, `view`, `layout`, `paint`, `raster`, `copy` (into the shared buffer), `commit`, and their `total`
(`Runtime::with_frame_timing`). Without the variable nothing is timed.
`app::mono_family` names the asset packs' fixed-pitch family, which the text system loads with the interface face, for
text that lines up by character (a file's contents); `None` when no pack names one.
`app::environment` gives the theme, text system and symbols a program measures text with before it has a window.
`theme::system_color(name, appearance)` is a colour the theme data names (`system-green`), from the data the process's
first application started on; `None` before that. `render::picture` draws paint operations, text included, into an
`ImageResource` on the CPU pool and gives it to the UI thread as a message. `kit::image` decodes PNG and JPEG within
the size the caller allows, resizes, crops, turns a picture upright and encodes PNG and JPEG: plain functions on
`ImageResource`s that a program runs with `task::cpu`, each refusing a picture of more pixels than a window drawn on the
CPU has.

The widgets' icons are the asset packs' symbols. `Icons::application`, the icon an alert shows, is the icon named by
the `Icon=` of the desktop entry whose id is the running program's file name (`cupertino-files.desktop` for
`cupertino-files`), from the packs, else from the host icon themes or a file, else the packs' generic application icon.
The icons' caches, like the text system's, can serve every instance of a process: they take whole images that never
change afterwards, a cache's lock is held only to look one up or put one in, and the packs are read and drawn outside
it.

`file_icons::FileIcons` gives apps that list files the icon of each item's type as image renditions (1× and 2×) for
`controls::Image`: the asset packs' and the host icon theme's icon of the type's kind or its nearest ancestor's (as
`cupertino_icons::document::document_icon_name` maps them), then the generic document icon, then the placeholder; a
special folder's own icon; and the alias badge of a symbolic link, drawn into the image. `discover` reads the installed
packs and the icon theme, `without_host` a given pack alone and `builtin` the built-in assets (placeholders), for tests
and captures. `pack_has(name)` says whether an asset pack ships a raster or SVG file of its own for an icon name, so an
app with its own art for a name can still let a pack's icon win. Files and the portal's file chooser draw their rows
with it. The items are the file family's (`cupertino-ui-files`' `file_icons`); `kit::file_icons` names them for the apps
that draw file icons without the rest of the family. `kit::files` also names the family's `thumbnail_service`, whose
`ThumbnailJob` a program runs with `task::io` within `JOB_DEADLINE`, `item_icons`, and `notice`, whose
`Notices::on` gives the future that posts a program's notices on its bus connection.

Each example runs on Cupertino headless and on headless Mutter, with these app ids and sizes: Counter (`org.cupertino.examples.Counter`, 200×64), Forms (`org.cupertino.examples.Forms`, 280×180), Virtual List (`org.cupertino.examples.VirtualList`, 480×600) and Editor (`org.cupertino.examples.Editor`, 480×320).

Windows move by their toolbar. The toolbar row of a `NavigationStack` is a window drag region: a drag where no control
takes the press moves the window, and a double click there zooms it (maximizes it, or restores it). `WindowDragArea`
makes any other part of a window one; `EventCx::move_window` and `EventCx::zoom_window` are the requests underneath.
The keyboard has no move: the compositor's Zoom, Minimize and tile shortcuts and the menu bar's Window menu cover
it (see `cupertino-ui-controls`).

`cargo run -p cupertino-kit --example kit_capture -- kit/controls` and `kit/icons` draw the row 167a controls sheet and a grid of system symbols (`--direction rtl` mirrors them),
and `kit/scroll` a scroll view at rest, stretched and with its bar shown, fading and hidden (`--phase`), `kit/under-toolbar` a
page scrolling under a toolbar row drawn in a layer over it (`--phase rest|shown|hidden`), headless, in light and dark at any scale (`examples/kit_capture.rs`, `kit::test::capture`). `kit/counter`, `kit/forms`,
`kit/virtual-list` and `kit/editor` draw the examples' windows as they open. `kit/gallery` draws every control family
side by side, `--state rest|held-button|held-switch|held-slider|held-segment|disabled` holding the pointer down on one
or disabling them all, at `--control-size mini|small|regular|large|extra-large` and `--compact on|off`, over
`--backdrop flat|busy`.

## Public API

What `shell` and `apps` crates name from the toolkit, where each item is in Kit, and what Kit refuses (W4.7 in
`docs/plans/layered-architecture.md`).

### Rules

- A `shell` or `apps` crate names the toolkit only as `cupertino_kit::…` (D23). What only the shell needs (surface
  roles, the shell protocol, the session, the lock) is not in Kit; it is `cupertino-kit-shell`'s (W4.9).
- Every Kit module is Kit's own and lists its items one by one. No module is a lower crate, or a module of one, under
  another name, and no list uses a glob.
- A module's list is the items a caller names, the types those items' signatures and public fields name, and for
  `controls` every control with its option types and layout constants.
- Kit names no Wayland object, no transport type, no raw surface or buffer, no GPU object, and no type a caller can
  build from curve or spring numbers (D7).

### Kit's modules

| Module | Holds |
|---|---|
| `kit::app` | `run`, `environment` (a `core::Environment`: the theme, text system and symbols a program measures text with before it has a window), `mono_family`, `AppError`; for now `run_on`, `run_on_with`, `run_on_with_appearance`, `run_lock_screen_on_with`, `run_on_with_until`, `run_lock_screen_on_with_until` and `options` ([Temporary exposures](#temporary-exposures)) |
| `kit::core` | the application (`Application`, `Command` (its `SetClipboardData { data, replaced }` puts `platform::ClipboardData` on the clipboard in several MIME types, a picture's `image/png` bytes among them, and sends `replaced` once another client's selection takes its place), `Window`, `WindowKey`, `Key`, `Popup`, `PopupAnchor`, `Menus`, `WindowBackground`, `RendererClass`, `Shortcut`, `CommandRegistry`); the widget (`Widget`, `Element`, `Change`, `Propagation`, `Event`, `Phase`, `EventCx`, `MeasureCx`, `PaintCx`, `SemanticsCx`, `IdleCx`, `Environment`, `FocusTarget`); semantics (`Role`, `Action`, `ActionData`, `AccessibleStates`, `AccessibleValue`, `Checked`, `Live`, `Relation`); drag and drop (`Drag`, `DragPreview`, `DropTarget`, `DropPlace`, `DropOver`, `Dropped`); glass (`Glass`, `GlassStyle`, `glass_outline`, `MAX_MATERIALS`, `SurfaceLayerMark`); icons (`Icons`, `IconImage`, `next_resource_id`); `session_scroll_bars`; for now `RuntimeOptions` |
| `kit::platform` | values of windows and input: `WindowSpec`, `WindowId`, `WindowRequest`, `DecorationMode`, `ForeignParent`, `SystemAppearance`, `ScrollBars`, `ScrollSource`, `CursorShape`, `Timestamp`, `KeyEvent`, `Keysym`, `Mods`, `PointerEvent`, `MouseButton`, `ClipboardData`, `DragProgress`, `DropAction`, `DropToken`, `GlassStyle`, and the compositor-run changes `PresentChange`, `PresentProperty`, `LayerChange`, `LayerProperty`, `LayerEdge`, `SurfaceLayerLook`, `MAX_LAYER_SAMPLES`; for now `WindowSystem` |
| `kit::layout` | `Align`, `Axis`, `Constraints`, `ContainerKind`, `Flex`, `Justify`, `LayoutDirection`, `LayoutSpec`, `Measured`, `Overflow`, `ScrollState`, `SizeSpec`, `Track` |
| `kit::render` | what a picture is made of: `PaintOp`, `Paint`, `Shape`, `PathVerb`, `FillRule`, `GradientStop`, `Effect`, `Transform`, `Sampling`, `ImageResource`, `PixelFormat`, `ResourceId`; `picture(env, ops, size, scale, done) -> Command<M>`, which draws paint operations, text included, on the CPU pool and gives the `ImageResource` (or `None`) to `done` as a message; for now the renderers and what they draw |
| `kit::text` | `TextSystem`, `AttributedText`, `TextStyle`, `ParagraphStyle`, `ParagraphLayout`, `FontWeight`, `FontStyle`, `FontFeature`, `TextAlign`, `TextRange`, `Wrap`, `ContentPurpose`, `TextBuffer`, `SecureBuffer`, `TextFieldKeys`, `Response`, `KeyInput`, `Keymap`, `binding_file::load_default`; for now the glyph rasters |
| `kit::theme` | `ThemeTokens` and the types of its fields (`ThemePreferences`, `ColorTokens`, `ControlMetrics`, `TypeScale`, `FocusRing`, `Motion`), `Appearance`, `Color`, `ColorRole`, `StateVariant`, `InteractionVariant`, `ControlFamily`, `ControlSize`, `TypeRole`, `Surface`, `BAR_GLASS_MATERIAL`, `GLASS_MATERIAL`, `CARET_PHASE_MS`, and `system_color(name, appearance) -> Option<Color>`, a colour the theme data names (`system-green`) |
| `kit::controls` | every control of `cupertino-ui-controls` with its option types (states, handlers, styles, `SortOrder`) and layout constants, and the helpers programs paint with (`caret_blinking`, `caret_opacity`, `paint_glass_edge`, `striped_element`, `unobscured_insets`) |
| `kit::filepicker`, `kit::typeahead` | the file chooser's model, whose rows are the file family's `Entry` in its order, and type-to-select |
| `kit::files` | the file family of `cupertino-ui-files`, in modules named as its own, each listing what Files and the portal's chooser name: `entry` (`Entry`, `Kind`), `natural`, `sort`, `mime`, `tags`, `applications`, `recents`, `places` (the sidebar's `standard_places` and the chooser's `chooser_places`, over the user directories of `cupertino_defaults::user_dirs`), `model` (`DirectoryModel`, whose `ModelWork` a program runs with `task::io` within `WORK_DEADLINE` and whose `ModelWatch` it follows with `Command::Perform`), `geom` (`Direction`), `layout::icon`, `rename`, `name_edit`, `open`, `operations` (with `Transfer`, a copy or a move whose `TransferSlice`s a program runs with `task::io` within `SLICE_DEADLINE`), `notice`, `thumbnail_service`, `item_icons`, `item_art`, and the views' `metrics` (`icon_view`, `list_view`, `column_view`, `timing`, `selection`, `marquee`, `drag`); the views: `ViewEvent`, `grid` (`IconGrid`), `list` (`FileList`), `columns` (`FileColumns`), `ColumnEvent`, and `panel` (`OpenSavePanel`, a file chooser's panel around a `FileList`, with `PanelEvent`, `ChoiceValue`, `PanelChoice`, `SaveAs`, `SIDEBAR_WIDTH`) |
| `kit::geometry` | `Point`, `Size`, `Rect`, `Insets` |
| `kit::motion` | [Motion](#motion) |
| `kit::shared` | what every instance of the process reads once, from work on the I/O pool: `file_types` (the shared MIME database of the XDG data directories, a `MimeDatabase`), `theme_icon` (a host icon theme's icon or a file's, as a `core::IconImage`), `pack_raster` (an asset pack's raster icon, or the placeholder a name no pack provides is drawn as) and `pack_has_raster` |
| `kit::image` | `decode` (PNG and JPEG within the side the caller allows), `resize`, `crop`, `orient` (upright from an `OutputTransform`, which the module names), `encode_png`, `encode_jpeg`, and `ImageError`: plain functions on `render::ImageResource` that a program runs as a `task::cpu`, each refusing a picture of more pixels than a window drawn on the CPU has |
| `kit::task`, `kit::bus`, `kit::time`, `kit::account`, `kit::host`, `kit::process`, `kit::file_icons`, `kit::spawn`, `kit::Instance`, `kit::Entered`, `kit::prelude` | as above |
| `kit::test` | [The `test` feature](#the-test-feature) |

`GlassStyle` is one type with two paths, `core::GlassStyle` and `platform::GlassStyle`. `theme::Motion` is the theme's
motion preferences (`reduce`, `caret_phase_ms`); the catalogue's type is `motion::Timing`.

### Motion

Motion is named, never built (D7). `kit::motion` names the three catalogue types of `cupertino-theme`'s `animation`
module, `Easing`, `SpringPreset` and `Timing`, and one of its own, `Animated`. None has a public field, a public variant
or a `From` that takes numbers.

- `Easing`: `EASE`, `LINEAR`, `EASE_IN`, `EASE_OUT`, `EASE_IN_OUT` (the timing table) and `COSINE`; `eval`, `over(seconds)
  -> Timing` (a duration with a named shape; a spring never takes a number) and `as_bezier`.
- `SpringPreset`: `DEFAULT`, `CONTROL_PANEL_OPEN`, `LAUNCHER_OPEN`, `POPOVER_OPEN`; `params`.
- `Timing`: `CONTROL_PANEL_CLOSE`, `OSD_FADE`, `INPUT_SOURCES_FADE`, `POPOVER_CLOSE`, `MENU_CLOSE`, and `From<SpringPreset>`; `value(seconds)`,
  `duration`, `curve` and `layer_curve`.
- `Animated<T: Animatable>`: a value that moves to a target and keeps its velocity when the target changes (`new`,
  `value`, `velocity`, `target`, `is_animating`, `set_immediately`, `cancel`, `track`, `animate_to(to, Timing, now)`,
  `release(to, SpringPreset, now)`).

Reading the numbers out is allowed; building from them is not. `as_bezier`, `params`, `curve` and `layer_curve` return
`cupertino-animation` types, which a `shell` or `apps` crate cannot name. `platform::PresentChange` and
`platform::LayerChange` hold a `Timing`; `cupertino-ui-wayland` sends its `layer_curve()` to the compositor (the cosine
ease as the Bézier the catalogue fits to it), so the wire format is the compositor's curves as before.
`LayerChange::through` takes samples: values a program computes from named motions, not a curve. A program that needs
a new motion adds it to the catalogue.

### The `test` feature

`cupertino-kit`'s one feature, `test`, adds `kit::test`; nothing else in Kit changes with it. A `shell` or `apps` crate
enables it only under `[dev-dependencies]`, and `cupertino-layers` refuses a normal or build dependency, or a feature,
that turns it on.

| In `kit::test` | Items |
|---|---|
| The headless backend (`cupertino-ui-test`) | `TestDriver`, `ManualClock`, `HeadlessWindowSystem`, and `StepStatus`, what a driver's steps return |
| The capture command's shared part | the `capture` module: `App`, `Look`, `Request`, `State`, `Options`, `run`, `Pixmap` (the picture `App::capture` draws), `presented` (a headless window's last frame as that picture, premultiplied) and the rest of its public items |
| What a driver is built from and reads | `Runtime`, `preferences`, `Capabilities`, `WindowEvent`, `WindowStates`, `PlaceholderIcons`, `FontIndex`, `each_turn` |

The capture examples of `files`, `login` and `kit` and `login`'s benches name it; Kit's tests and `kit_capture` are
built with the feature (`required-features`), and name the renderer's internals they test (`CpuFrame`, `render_thread`,
the window system's `Submitted` and `Shown`) from the lower crates directly. `kit::test::capture` is the shared part of the capture examples (below).

**Release builds.** `cargo build-programs` (`.cargo/config.toml`) builds the programs with every feature of the
workspace's packages, each named as `--features=PACKAGE/FEATURE`, except `cupertino-kit/test`; both package recipes
(`packaging/arch/cupertino`, `packaging/arch/cupertino-git`) build with it, and `cupertino-ui-abi`'s library without
`--all-features`. `cupertino-layers` checks that the alias names exactly those features, so a feature added to a
program is never left out of the packages.

### The capture command

`kit::test::capture`, with the `test` feature: the flags of every app's capture example, the names of the PNGs it
writes and the contact sheet. `cupertino-files`, `cupertino-kit` and `cupertino-login` use it in
`examples/<app>_capture.rs`, which draw their surfaces with their own painters on sample data.

```
cargo run -p CRATE --example APP_capture -- APP/SURFACE [--scale N] [--appearance light|dark|both]
              [--increase-contrast] [--reduce-transparency] [--size WxH] [--out DIR] [--sheet] [--STATE-FLAG VALUE]...
cargo run -p CRATE --example APP_capture -- APP    # lists the surfaces of APP
```

`CRATE` is `cupertino-files`, `cupertino-kit` or `cupertino-login`; `APP` is `files`, `kit` or `login`.

| Flag | Effect |
|---|---|
| `--scale N` | device pixels per point (default 1) |
| `--appearance` | `light`, `dark` or `both` (default) |
| `--increase-contrast`, `--reduce-transparency` | the session's display options |
| `--size WxH` | the window size in points instead of the surface's default |
| `--out DIR` | where the PNGs go (created); the default is `captures` in `$CARGO_TARGET_DIR`, else in the workspace's `target` |
| `--sheet` | every state the app lists for the surface, and one contact sheet: states as columns, appearances as rows; refused with a state flag |
| `--STATE-FLAG VALUE` | a state of the surface; each app names its own (`--login saving`, `--view list`) |

One PNG per state and appearance: `<app>-<surface>[-<flag>-<value>]...-<light|dark>[-hc][-rt]@<scale>x.png`, the value in
lower case with other characters than letters and digits as `-` (`settings-accessibility-login-saving-dark@2x.png`).
The sheet is `<app>-<surface>-sheet[-hc][-rt]@<scale>x.png`. The command prints each path it writes. It exits 2 with a
message for an unknown surface, flag or state value and when no asset pack is found: a capture with the built-in font
alone does not show the surfaces as they look.

| Surface | States |
|---|---|
| `files/window`, `files/sidebar` (the sidebar column of the window), `files/preview`, `files/popover`, `files/tabs` | `--view icon\|list\|columns\|gallery`, `--selected NAME`, `--state rename\|ring\|band` (the view's item renamed from its context menu, ringed under its open context menu, or under a band held over it), `--devices removable` (a mounted, a busy and an unmounted device in the sidebar) |
| `desktop/items` of `cupertino-shell-desktop`'s `desktop_capture` (the desktop's items over a flat fill) | `--state select\|rename\|drag` |
| `kit/controls` (the row 167a controls sheet), `kit/icons` (with the chevrons along the reading direction and a missing symbol's placeholder) | `--direction ltr\|rtl` |
| `kit/gallery` (every control family; its sheet shows each state) | `--state rest\|held-button\|held-switch\|held-slider\|held-segment\|disabled`, `--control-size mini\|small\|regular\|large\|extra-large`, `--compact on\|off`, `--backdrop flat\|busy`, `--direction ltr\|rtl` |
| `kit/scroll` (an overlay scroll view: at rest, pulled past its top edge, its bar shown, fading and hidden after a wheel step) | `--direction ltr\|rtl`, `--phase rest\|stretch\|shown\|fading\|hidden` |
| `kit/under-toolbar` (a page that scrolls under a toolbar row drawn in a layer over it, set off by the scroll edge: at rest, just after a wheel step and after the bar has faded) | `--direction ltr\|rtl`, `--phase rest\|shown\|hidden` |
| `login/lock`, `login/picker`, `login/password` (the lock screen, and the login screen's accounts and password screen, at 2560 x 1440 points unless `--size` says otherwise, on a wallpaper made for the capture) | `--field empty\|typed\|help` (not the picker: the password typed shows its dots and the submit button; help, after failed attempts, the help button, its line and the message); the picker: `--selection none\|shown` (no selection as it opens, or the first tile selected), `--users three\|one` (three accounts and "Other…", or one account alone) |

The sidebar material is the server's, so a flat fill of the window background stands in for it. The home folder and
the pictures are example data, not the machine's. Files draws at a fixed clock (2026-01-01 15:00
UTC), so a command gives the same pixels on every run.

### Temporary exposures

These stay nameable until each caller's own task replaces them; the plan's decision log records them.

| Items | Callers | Each caller's task | After |
|---|---|---|---|
| `app::run_on`, `run_on_with`, `run_on_with_appearance`, `run_lock_screen_on_with`, `platform::WindowSystem`, `core::RuntimeOptions` | `cupertino-kit-shell`'s runtime | W6.16 | a component starts through `cupertino-kit-shell` and gets a `core::Environment` |
| `app::options` | `cupertino-ui-abi`, `notifications`, the drivers | `notifications` W6.3 | `app::environment` in a program; `kit::test` for a driver; the start-up below Kit for `cupertino-ui-abi` |
| the renderers (`render::{CpuRenderer, CpuConfig, UiRenderer, RenderRequest, DisplayList, PaintNode, PaintNodeId, PaintResources, PaintRevision, DamageRegion}`) and glyph rasters (`text::{GlyphRaster, GlyphRasterizer, GlyphKey, GlyphFormat, RasterGlyph, TextError}`) | `dock`, `login` and `login`'s bench | W6.8, W6.13 | `render::picture`; `kit::test` for the bench |

A step of W6.16, after the last of these tasks, puts Kit's start-up (the theme, the fonts, the options, following the
appearance, the timing reports) in `cupertino-ui-wayland`, where `cupertino-kit`, `cupertino-kit-shell` and
`cupertino-ui-abi` call it, and deletes the items above from Kit.

### What Kit refuses

| Not in Kit | What a program names instead |
|---|---|
| a lower crate, or a module of one, as a Kit module (`cupertino_ui_core as core` and the like) | the modules above |
| GPU objects and the render thread (`render::gpu`, `render::render_thread`); raw surfaces and buffers (`platform::{Lent, DrawInto, PresentOutcome, Configure}`); `cupertino-app-protocols`' `semantics` | a Kit window |
| curves and springs from numbers (`CubicBezier`, `LayerCurve`, `SpringParams`, `Curve`, the theme's curve constants and `DEFAULT_SPRING`) | `kit::motion` |
| the runtime's wiring (`MonotonicClock`, `RendererFactory`, `EachTurn`, `AccessibilityAdapter`, `set_personal`, `layout::{layout, layout_cached, LayoutCache, LayoutTree}`) | `app::run`, which does it |
| the window system (`WaylandWindowSystem`, `SurfaceRoles`, `ShmPresenter`, the AT-SPI adapter), the connection and its outputs and raw input (`cupertino-wayland-client`) | `app::run` and a Kit window's values; the shell's in `cupertino-kit-shell` |
| the timing reports' wiring (`Timed`, `frame_timing_json`, `scroll_timing_json`, `start_preferences`) | `CUPERTINO_SCROLL_TIMING` and `CUPERTINO_FRAME_TIMING`, which every `app::run` reads |
| the theme data (`cupertino_assets::theme::load`, on the methods `shell` and `apps` crates must not call) | `theme::system_color` for a named colour; the theme a widget is painted with; the menu bar's metrics from `cupertino-kit-shell` |
| drawing maths (`cupertino-raster-math`) and the compositor's drawing (`cupertino-server-render::decorations`) | `render::Effect` and the controls |
