# cupertino-kit

CupertinoKit, the Cupertino UI toolkit: the one crate applications depend on. It re-exports the `cupertino-ui-*` crates
as modules and has a `prelude`. `typeahead` (`TypeAhead`) and `filepicker` (`Picker` and its `Mode`, `Key`,
`Request`, `Listing`, `Entry` and `Outcome`; `places` with `session_places`, `parse_user_dirs` and `Place`) name the
type-to-select and file chooser model of `cupertino-ui-controls`. MPL-2.0.

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
panic hook stay one per process, installed by `main`.

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
(`local`, `now`) and turns a local date and time back into a moment (`from_local`), daylight saving time included.
`kit::spawn` starts a program for the instance the thread runs for, and the instance's UI loop reaps it on its first
turn after it ends (`app::each_turn`, which every `app::run` installs with `Runtime::with_each_turn`), reporting an
unsuccessful exit on standard error; no thread waits for it.

`app::run` and `app::options` start on the Cupertino theme from the installed theme data, and on
`ThemeTokens::neutral` when there is none or it is unusable (one line on stderr says why), so an application runs on any
compositor. `app::run` connects for the application's renderer class (`Application::RENDERER`), and its renderer factory
makes the GPU renderer once the process draws on the GPU. `app::run_on` and `app::run_on_with` run on a window system a
desktop shell set up with its surface roles (connected with `WaylandWindowSystem::connect_as` for its class);
`app::run_lock_screen_on_with` does the same for the lock screen, whose AT-SPI adapter the session's lock never mutes
(`AtspiAdapter::lock_screen`), and `app::run_on_with_appearance` passes the session's appearance through a function first, for a
shell surface that keeps the dark appearance in a light session. Every one of them keeps the text caret from blinking
(`caret_phase_ms` 0) while the session asks for a steady caret, from the first frame.
Every one of them reports scroll timing when `CUPERTINO_SCROLL_TIMING` (`app::SCROLL_TIMING_ENV`) names an absolute path:
one JSON line a pointer scroll event, appended, with its stages in nanoseconds of `CLOCK_MONOTONIC` (`input`, the
window system's timestamp; `handled`; `updated`, the model update and view, or `null`; `submitted`, the frame that drew
it; `presented`, the compositor's time for that frame, or `null` when it was dropped). Without the variable no timing
is kept (`app::Timed`, on `Runtime::with_scroll_timing`). They report frame timing when `CUPERTINO_FRAME_TIMING`
(`app::FRAME_TIMING_ENV`) names an absolute path: one JSON line a presented frame, appended (every process of a session
may share the file), with `pid`, `program`, `renderer` (`cpu` or `gpu`, what the process draws with now), `start` in nanoseconds of `CLOCK_MONOTONIC`, and the nanoseconds of each
phase: `update`, `view`, `layout`, `paint`, `raster`, `copy` (into the shared buffer), `commit`, and their `total`
(`Runtime::with_frame_timing`). Without the variable nothing is timed.
`app::mono_family` names the asset packs' fixed-pitch family, which the text system loads with the interface face, for
text that lines up by character (a file's contents); `None` when no pack names one.

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
with it.

Each example runs on Cupertino headless and on headless Mutter, with these app ids and sizes: Counter (`org.cupertino.examples.Counter`, 200×64), Forms (`org.cupertino.examples.Forms`, 280×180), Virtual List (`org.cupertino.examples.VirtualList`, 480×600) and Editor (`org.cupertino.examples.Editor`, 480×320).

Windows move by their toolbar. The toolbar row of a `NavigationStack` is a window drag region: a drag where no control
takes the press moves the window, and a double click there zooms it (maximizes it, or restores it). `WindowDragArea`
makes any other part of a window one; `EventCx::move_window` and `EventCx::zoom_window` are the requests underneath.
The keyboard has no move: the compositor's Zoom, Minimize and tile shortcuts and the menu bar's Window menu cover
it (see `cupertino-ui-controls`).

`cargo run -p cupertino-kit --example kit_capture -- kit/controls` and `kit/icons` draw the row 167a controls sheet and a grid of system symbols (`--direction rtl` mirrors them),
and `kit/scroll` a scroll view at rest, stretched and with its bar shown, fading and hidden (`--phase`), `kit/under-toolbar` a
page scrolling under a toolbar row drawn in a layer over it (`--phase rest|shown|hidden`), headless, in light and dark at any scale (`examples/kit_capture.rs`, `crates/cupertino-capture`). `kit/counter`, `kit/forms`,
`kit/virtual-list` and `kit/editor` draw the examples' windows as they open. `kit/gallery` draws every control family
side by side, `--state rest|held-button|held-switch|held-slider|held-segment|disabled` holding the pointer down on one
or disabling them all, at `--control-size mini|small|regular|large|extra-large` and `--compact on|off`, over
`--backdrop flat|busy`.

## Public API (proposal)

The proposal of W4.7 step 1 in `docs/plans/layered-architecture.md`: what `shell` and `apps` crates name from the
toolkit, where each item is in Kit, and what Kit refuses. It describes the target; the text above this section describes
Kit as it is. It takes effect when the plan's decision log records its approval, with the changes to other tasks listed
under [Step 2](#step-2).

### Rules

- A `shell` or `apps` crate names the toolkit only as `cupertino_kit::…` (D23). `cupertino-kit-shell` adds what only the
  shell needs: surface roles, the shell protocol, the session, the lock.
- Every Kit module is Kit's own and lists its items one by one. No module is a lower crate, or a module of one, under
  another name, and no list uses a glob.
- A module's list is the items a caller names, the types those items' signatures and public fields name, and for
  `controls` every control with its option types and layout constants. An item without a caller is not listed.
- Kit names no Wayland object, no transport type, no raw surface or buffer, no GPU object, and no type a caller can
  build from curve or spring numbers (D7).
- A path that stays keeps its name: `cupertino_kit::core::Widget` and `cupertino_kit::filepicker::Picker` are the same
  paths before and after.

### What programs name today

A use is one entry of a `use` declaration, groups expanded, or one full path written in code. Comments and string
literals are left out; `src/`, `examples/` and `benches/` are read. A cell is distinct paths / uses.
`rg -n '\bcupertino_(kit|ui_\w+|theme|animation|raster_math|gpu|wayland_client|app_protocols|shell_client|protocols|decorations|capture)\b' crates/cupertino-CRATE -g '*.rs'`
lists the lines of one crate.

| | ui-core | ui-text | ui-wayland | ui-test | theme | animation | raster-math | wayland-client | app-protocols | shell-client | protocols | decorations | capture | kit |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `control` | | | | | | | | 5/9 | 6/9 | 3/4 | | | | |
| `controlcenter` | | | 3/3 | | | 2/2 | | 6/6 | | 9/9 | | | | 93/98 |
| `dock` | | | 3/3 | | | 4/11 | | 6/6 | | 21/24 | | | | 102/403 |
| `launcher` | | 1/1 | 3/3 | | | 5/8 | | 5/5 | | 13/13 | | | | 115/278 |
| `login` | | | 3/5 | 2/4 | | | 2/2 | 6/6 | | 24/31 | | | 6/6 | 122/237 |
| `menubar` | | 5/5 | 3/3 | | 1/1 | | | 10/10 | | 37/38 | | | | 90/98 |
| `notifications` | | | 3/3 | | 3/3 | 3/5 | | 6/6 | | 9/9 | | | | 88/124 |
| `osd` | | | 3/3 | | | | | 5/5 | | 9/9 | | | | 46/50 |
| `polkit-agent` | 12/27 | | 3/6 | | 5/8 | | | 14/31 | | 8/13 | | 8/13 | | |
| `portal` | | | 2/2 | | | | | 14/24 | | 8/8 | 3/3 | | | 60/62 |
| `screenshot` | | | 3/3 | | | | | 5/6 | | 9/9 | 2/2 | | | 63/66 |
| `windowmanager` | | | | | | | | | | | 5/5 | | | |
| `files` | | 1/1 | 3/3 | 2/2 | 1/1 | | | 7/11 | | | | | 5/5 | 189/844 |
| `settings` | | | | | | | | 2/4 | | | | | | 115/175 |
| all | 12/27 | 5/7 | 6/37 | 2/6 | 9/13 | 6/26 | 2/2 | 22/129 | 6/9 | 60/167 | 10/10 | 8/13 | 6/11 | 327/2435 |

- Below Kit's facade (`ui-core` to `app-protocols`): 70 paths, 256 uses.
- Below `cupertino-kit-shell`'s facade (`shell-client`, `protocols`): 70 paths, 177 uses.
- Through Kit: 327 paths, 2435 uses. 295 paths (2367 uses) keep their path, 10 paths (24 uses) change in step 2 and
  22 paths (44 uses) leave with their last caller, one of those uses in step 2
  ([Through Kit today](#through-kit-today)).
- Callers outside `shell` and `apps`: Kit's examples (67 paths, 106 uses) and tests (35, 46), the application template
  of `cupertino-cli` (`templates/rust/src/main.rs`: 9, 9) and `cupertino-ui-abi` (5, 5). The lists below hold their
  paths too, except `core::Runtime` (`cupertino-ui-abi`) and `app::each_turn` (Kit's `tests/children.rs`).
- No `shell` or `apps` crate names `cupertino-ui-controls`, `cupertino-ui-render`, `cupertino-ui-abi`, `cupertino-gpu`,
  `cupertino-glass` or `cupertino-cursor`.
- Each non-empty cell left of `decorations` is one `facade` line of `crates/cupertino-layers/remaining.txt` (55 lines);
  the list holds one more, `cupertino-settings -> cupertino-theme`, a dependency whose items no source names.
- Six programs reach `cupertino-theme` with no dependency on it, through `cupertino_assets::theme::load`: `files`
  (`kit/tag_editor.rs`, `kit/desktop/mod.rs`), `controlcenter`, `notifications`, `menubar` and `polkit-agent`.

### Below the facade today: where each item goes

`control` and `portal` are `services` crates in the target (W5.12, W6.11): a service names the transport and the
protocol crates itself and never Kit, so their columns need nothing from Kit. The portal's file chooser goes to the file
views (W4.8) and its colour picker to `cupertino-screenshot` (D10).

| From | Items (uses) | In Kit | Callers use |
|---|---|---|---|
| `cupertino-ui-core` | `Role`, `Action`, `AccessibleStates`, `AccessibleValue`, `Checked`, `Live`, `Relation`, `platform::WindowId` (19) | `kit::core`, `kit::platform` | the same items; a widget states them in `semantics` |
| | `AccessibilityAdapter`, `AccessibilityNode`, `AccessibilityUpdate`, `NodeId` (8) | refused | nothing: Kit's runs publish the tree a window's widgets describe. Only `polkit-agent` builds one by hand, and its prompt is a Kit window in `cupertino-prompts` (W6.14) |
| `cupertino-ui-text` | `TextBuffer`, `TextFieldKeys`, `Response`, `KeyInput`, `Keymap` (7) | `kit::text` | `kit::text::…` |
| `cupertino-ui-wayland` | `WaylandWindowSystem`, `SurfaceRoles`, `RoleEvent` (29) | refused: the window system hands out its `Connection` | `cupertino-kit-shell`'s components and surface roles (W4.9); the desktop items in `cupertino-shell-desktop` (W6.10); an application calls `app::run` |
| | `ShmPresenter`, `Ready` (6) | refused: a raw surface | a Kit window |
| | `atspi::AtspiAdapter` (2) | refused | nothing: `app::run` installs it |
| `cupertino-ui-test` | `TestDriver`, `ManualClock` (6) | `kit::test` | `kit::test::…` |
| `cupertino-theme` | `animation::{EASE_IN_OUT_CURVE, DEFAULT_SPRING}` (2) | refused (D7) | `kit::motion` |
| | `Theme`, `Typography`, `ShapeTokens` (3, `notifications`, `menubar`, `files`): the menu bar's height and fonts | not in Kit | `cupertino-kit-shell` (question 6) |
| | `Theme`, `Appearance`, `AccentColor`, `SemanticColor`, `DesignTokens` (8, `polkit-agent`) | `kit::theme::{Appearance, ColorRole, ThemeTokens, system_color}` | the theme a widget is painted with |
| `cupertino-animation` | `CubicBezier`, `SpringParams`, `Spring`, `Curve`, `LayerCurve`, `AnimatedValue` (26) | refused (D7) | `kit::motion` |
| `cupertino-raster-math` | `dither`, `blur::sigma_for_radius` (2, the login wallpaper) | refused: drawing maths (D1) | an image painted with `render::Effect::Blur` (W6.13) |
| `cupertino-wayland-client` | the connection: `Connection`, `Error`, `Event`, `ExtensionEvent`, `State`, `SurfaceId`, `ExternalRole`, `Waker` (74) | refused | `cupertino-kit-shell`; Settings asks `org.cupertino.Displays1` through `kit::bus` (D14, W7.2) |
| | outputs: `OutputId`, `OutputInfo`, `OutputGeometry`, `OutputTransform` (23) | refused | `cupertino-kit-shell`'s outputs; Settings as above |
| | application effects: `app::{AppEffects, AppEvent, Appearance, Backdrop}` (17) | refused | `platform::SystemAppearance` and the theme for appearance and display options; `PaintCx::glass` and `theme::Surface` for backdrops; `cupertino-kit-shell` for the lock notice |
| | raw input: `KeyboardEvent`, `PointerEvent`, `Button`, `Modifiers`, `key` (15) | refused | `platform::{KeyEvent, PointerEvent, MouseButton, Mods, Keysym}` of a Kit window |
| `cupertino-app-protocols` | `semantics::{Action, ActionReply, Data, Invocation, Observation, MAX_TEXT}` (9, `control`) | refused | `control` is a service |
| `cupertino-shell-client` | layer surfaces (7 paths, 76); events and errors (3, 21); the shell protocol (23, 29); the session (14, 20); desktop control and consent (7, 11); lock and idle (4, 5); `test_clock` (2, 5) | not in Kit | `cupertino-kit-shell` (W4.9, W5.10) |
| `cupertino-protocols` | the window manager, screenshot and global shortcut bindings (10) | not in Kit | `cupertino-kit-shell`; `portal` is a service |
| `cupertino-decorations` | `field::*`, `text::TextRenderer`, `shape::rounded_path`, `palette::Rgba` (13, `polkit-agent`) | refused: the compositor's drawing | `controls::{SecureField, Text, Button}` in `cupertino-prompts` (W6.14) |
| `cupertino-capture` | `App`, `Look`, `Request`, `State`, `run`, `assets` (11) | `kit::test::capture` (W4.11) | `kit::test::capture::…` |
| `cupertino-assets` | `theme::load` (6): named system colours (`files`' tags), the menu bar's height and fonts, the prompt's palette | `kit::theme::system_color` for the colours | that function; `cupertino-kit-shell` for the menu bar; the theme a widget is painted with |

### Through Kit today

| Module today | Paths / uses | Changes in step 2 (uses) | Leaves with its last caller (uses) |
|---|---|---|---|
| `core` | 51 / 800 | `PlaceholderIcons` (1): `kit::test` | `RuntimeOptions` (3) |
| `controls` | 111 / 334 | | |
| `layout` | 20 / 319 | | |
| `platform` | 35 / 236 | `LayerCurve` (1): `kit::motion`; `Capabilities`, `WindowEvent`, `WindowStates` (9, examples and benches): `kit::test` | `WindowSystem` (2) |
| `geometry` | 8 / 197 | | |
| `render` | 25 / 183 | | `CpuRenderer`, `CpuConfig`, `UiRenderer`, `RenderRequest`, `DisplayList`, `PaintNode`, `PaintNodeId`, `PaintResources`, `PaintRevision`, `DamageRegion` (12) |
| `text` | 26 / 163 | `FontIndex` (1): `kit::test` | `GlyphRaster`, `GlyphRasterizer`, `GlyphKey`, `GlyphFormat`, `RasterGlyph`, `TextError` (13) |
| `theme` | 17 / 108 | `EASE_IN_OUT_CURVE`, `EASE_OUT_CURVE`, `LINEAR_CURVE` (9): `kit::motion` | |
| `prelude` | 10 / 27 | | |
| `app` | 8 / 23 | `preferences` (3): `kit::test`; `options` in Settings (1): `app::environment` | `run_on`, `run_on_with`, `run_lock_screen_on_with` (10), `options` (3) |
| `filepicker`, `typeahead` | 11 / 25 | | |
| `file_icons` | 2 / 8 | | |
| `spawn`, `Instance`, `task` | 3 / 12 | | |

Every other path stays. [Step 2](#step-2) says when each item of the last column leaves and what its caller names then.

### Kit's modules

| Module | Holds |
|---|---|
| `kit::app` | `run`, `environment` (a `core::Environment`: the theme, text system and icons a program lays out with before it has a window), `mono_family`, `AppError` |
| `kit::core` | the application (`Application`, `Command`, `Window`, `WindowKey`, `Key`, `Popup`, `PopupAnchor`, `Menus`, `WindowBackground`, `RendererClass`, `Shortcut`, `CommandRegistry`); the widget (`Widget`, `Element`, `Change`, `Propagation`, `Event`, `Phase`, `EventCx`, `MeasureCx`, `PaintCx`, `SemanticsCx`, `IdleCx`, `Environment`, `FocusTarget`); semantics (`Role`, `Action`, `ActionData`, `AccessibleStates`, `AccessibleValue`, `Checked`, `Live`, `Relation`); drag and drop (`Drag`, `DragPreview`, `DropTarget`, `DropPlace`, `DropOver`, `Dropped`); glass (`Glass`, `GlassStyle`, `glass_outline`, `MAX_MATERIALS`, `SurfaceLayerMark`); icons (`Icons`, `IconImage`, `next_resource_id`); `session_scroll_bars` |
| `kit::platform` | values of windows and input: `WindowSpec`, `WindowId`, `WindowRequest`, `DecorationMode`, `ForeignParent`, `SystemAppearance`, `ScrollBars`, `ScrollSource`, `CursorShape`, `Timestamp`, `KeyEvent`, `Keysym`, `Mods`, `PointerEvent`, `MouseButton`, `ClipboardData`, `DragProgress`, `DropAction`, `DropToken`, `GlassStyle`, and the compositor-run changes `PresentChange`, `PresentProperty`, `LayerChange`, `LayerProperty`, `LayerEdge`, `SurfaceLayerLook`, `MAX_LAYER_SAMPLES` |
| `kit::layout` | `Align`, `Axis`, `Constraints`, `ContainerKind`, `Flex`, `Justify`, `LayoutDirection`, `LayoutSpec`, `Measured`, `Overflow`, `ScrollState`, `SizeSpec`, `Track` |
| `kit::render` | what a picture is made of: `PaintOp`, `Paint`, `Shape`, `PathVerb`, `FillRule`, `GradientStop`, `Effect`, `Transform`, `Sampling`, `ImageResource`, `PixelFormat`, `ResourceId`; and `picture` (question 4) |
| `kit::text` | `TextSystem`, `AttributedText`, `TextStyle`, `ParagraphStyle`, `ParagraphLayout`, `FontWeight`, `FontStyle`, `FontFeature`, `TextAlign`, `TextRange`, `Wrap`, `ContentPurpose`, `TextBuffer`, `SecureBuffer`, `TextFieldKeys`, `Response`, `KeyInput`, `Keymap`, `binding_file::load_default` |
| `kit::theme` | `ThemeTokens` and the types of its fields (`ThemePreferences`, `ColorTokens`, `ControlMetrics`, `TypeScale`, `FocusRing`, `Motion`), `Appearance`, `Color`, `ColorRole`, `StateVariant`, `InteractionVariant`, `ControlFamily`, `ControlSize`, `TypeRole`, `Surface`, `BAR_GLASS_MATERIAL`, `GLASS_MATERIAL`, `CARET_PHASE_MS`, and `system_color(name, appearance) -> Option<Color>`, a named colour of the theme data (`system-green`) |
| `kit::controls` | every control with its option types and layout constants; the file views (W4.8) |
| `kit::filepicker`, `kit::typeahead` | as they are; the open and save panel of W4.8 is built on them |
| `kit::geometry` | `Point`, `Size`, `Rect`, `Insets` (question 5) |
| `kit::motion` | [Motion](#motion) |
| `kit::image` | question 7 |
| `kit::task`, `kit::bus`, `kit::time`, `kit::file_icons`, `kit::spawn`, `kit::Instance`, `kit::Entered`, `kit::prelude` | as they are |
| `kit::test` | [The `test` feature](#the-test-feature) |

`GlassStyle` is one type with two paths, `core::GlassStyle` and `platform::GlassStyle`; both have callers and both stay.
`theme::Motion` is the theme's motion preferences (`reduce`, `caret_phase_ms`); the catalogue's type is `motion::Timing`.

### Motion

Motion is named, never built (D7). Kit names three catalogue types, defined in `cupertino-theme`'s `animation` module,
and one of its own. None has a public field, a public variant or a `From` that takes numbers.

```rust
// cupertino_theme::animation, named by Kit as kit::motion::{Easing, SpringPreset, Timing}

/// A timing function of the design catalogue.
pub struct Easing(/* private */);
impl Easing {
    pub const EASE: Easing;
    pub const LINEAR: Easing;
    pub const EASE_IN: Easing;
    pub const EASE_OUT: Easing;
    pub const EASE_IN_OUT: Easing;
    pub const COSINE: Easing;
    /// Eased progress for linear progress from 0 to 1.
    pub fn eval(&self, x: f64) -> f64;
    /// This easing over `seconds`.
    pub fn over(self, seconds: f64) -> Timing;
    /// The control points, for a crate that sends or evaluates them; `None` for the cosine ease.
    pub fn as_bezier(&self) -> Option<CubicBezier>;
}

/// A spring of the design catalogue.
pub struct SpringPreset(/* private */);
impl SpringPreset {
    pub const DEFAULT: SpringPreset;
    // one constant for each spring of the catalogue
    /// The spring's numbers, for a crate that sends or evaluates them.
    pub fn params(&self) -> SpringParams;
}

/// How a value moves: an easing over a duration, or a spring.
pub struct Timing(/* private */);
impl From<SpringPreset> for Timing;
impl Timing {
    // one constant for each timed motion of the catalogue
    /// Progress `seconds` after the start: 0 at the start, 1 at rest; a spring may pass 1.
    pub fn value(self, seconds: f64) -> f64;
    /// Seconds until rest: an easing's duration, a spring's settling time.
    pub fn duration(self) -> f64;
    /// The numbers, for a crate that evaluates them or sends them to the compositor.
    pub fn curve(self) -> cupertino_animation::Curve;
    pub fn layer_curve(self) -> cupertino_animation::LayerCurve;
}

// kit::motion

/// A value that moves to a target and keeps its velocity when the target changes.
pub struct Animated<T: Animatable>(/* private */);
// `new`, `value`, `velocity`, `target`, `is_animating`, `set_immediately`, `cancel`, `track`,
// `animate_to(to, Timing, now)`, `release(to, SpringPreset, now)`
pub trait Animatable;
```

- `Easing` and `SpringPreset` are the catalogue's types of those names. `Easing`'s `Bezier(CubicBezier)` variant and
  `SpringPreset`'s public fields go: the theme data's parser builds both inside the crate, and no crate outside
  `cupertino-theme` names either today.
- Reading the numbers out is allowed; building from numbers is not. `as_bezier`, `params`, `curve` and `layer_curve`
  return `cupertino-animation` types, which a `shell` or `apps` crate cannot name. `cupertino-ui-wayland` sends
  `Timing::layer_curve()` to the compositor, the cosine ease as the Bézier the catalogue fits to it; Kit's `Animated`
  evaluates `Timing::curve()`. The wire format is unchanged.
- `platform::PresentChange` and `platform::LayerChange` hold a `Timing` where they hold a `LayerCurve` today.
  `LayerChange::through` stays: samples are values a program computes from named motions, not a curve.
- The catalogue's entries today: the six of the timing table (`ease`, `linear`, `ease-in`, `ease-out`, `ease-in-out`,
  `spring`), the cosine ease, and the theme data's named springs (`AnimationPresets`), which no program reads.
- Four crates build motions from numbers at 17 places: `controlcenter` 2 (`lifecycle.rs`), `dock` 5 (`ease.rs`,
  `fan.rs`, `tile_drag.rs`, `overview_ui.rs`), `launcher` 7 (`motion.rs`, `panel.rs`, `view/layers.rs`),
  `notifications` 3 (`center.rs`, `view.rs`). Two of the dock's repeat `ease-in-out` and `ease-in`.
- Step 2 adds the three entries its own change needs, with the numbers they have today:
  `SpringPreset::CONTROL_PANEL_OPEN` and `Timing::CONTROL_PANEL_CLOSE` (`controlcenter/src/lifecycle.rs:23-33`) and
  `SpringPreset::LAUNCHER_OPEN` (`launcher/src/view/layers.rs:736`, the spring `motion.rs:363` builds too). The other
  entries come with each component's own task, which leaves no duration, curve or spring literal in the crate.

### The `test` feature

`cupertino-kit`'s one feature, `test`, adds `kit::test`; nothing else in Kit changes with it. A `shell` or `apps` crate
enables it only under `[dev-dependencies]`.

| In `kit::test` | Items | Callers |
|---|---|---|
| The headless backend (`cupertino-ui-test`) | `TestDriver`, `ManualClock`, `HeadlessWindowSystem` and the records its accessors return | the capture examples of `files`, `login` and `kit`, `login`'s benches |
| What a driver is built from and reads | `Runtime`, `preferences`, `Capabilities`, `WindowEvent`, `WindowStates`, `PlaceholderIcons`, `FontIndex`, `each_turn` | the same, and Kit's `tests/children.rs` |
| `kit::test::capture` (W4.11) | `App`, `Look`, `Request`, `State`, `run`, `assets` | the three capture examples |

- The rest of `cupertino-capture` (`Options`, `USAGE`, `OUT`, `parse`, `capture`, `sheet`, `file_name`, `sheet_name`)
  is private to the module: `run` is their only caller. The backend's `Inspector`, `Trace`, semantic checks,
  `AssistiveTechnology` and `repaint` meter have no caller outside `cupertino-ui-test` and are not named.
- A capture is a `render::ImageResource`: W4.11 changes `capture::App::capture` to return it, so an example names no
  `tiny_skia` type. `tiny-skia` as a dev-dependency of `cupertino-files` is a line of `remaining.txt` today.
- `options`, `RuntimeOptions`, `CpuRenderer` and `CpuConfig` join `kit::test` when they leave `app`, `core` and `render`
  ([Step 2](#step-2)).
- Kit's own `tests/children.rs` and `examples/kit_capture.rs` are built with the feature (`required-features`).
- `cupertino-ui-abi` is a `ui` crate, below the facade rule. It depends on `cupertino-ui-test` for its headless
  backend and names `Application`, `Command`, `Runtime` and `Window` from `cupertino-ui-core`; from Kit it calls
  `app::options`. Its C interface does not change (D3).

**Release builds.** `cargo build-programs` (`.cargo/config.toml:16`) and both package recipes
(`packaging/arch/cupertino/PKGBUILD:56`, `packaging/arch/cupertino-git/PKGBUILD:49`) build `--workspace --all-features
--bins` today, which would turn `cupertino-kit/test` on in every packaged program. In step 2:

- The alias names the features instead of `--all-features`: every feature a workspace package has, as
  `--features PACKAGE/FEATURE`, except `cupertino-kit/test`. The packaged programs are built with the features they
  have today. The list is in the alias, the one definition of "the feature set the packages use", not in
  `.config/validation.toml`, which Cargo's alias cannot read, and not as default features, which would change 14
  manifests and what each crate's tests build.
- Both recipes run `cargo build-programs --frozen`, so there is one list. Their `cupertino-ui-abi --lib` line loses
  `--all-features`; that crate has no features.
- `cupertino-layers` refuses three things: a normal or build dependency that enables `cupertino-kit/test`; a feature of
  any package that enables it; and an alias whose list is not the workspace's features without that one, so a feature
  added to a program is never left out of the packages unnoticed.
- `docs/src/building.md:24-28` and the Tracy build line of `cupertino-diagnostic-events`' README name the old flags and
  change with them. `xtask`'s licence graph keeps `--all-features`: `test` brings in `cupertino-ui-test` and
  `tiny-skia`, both in that graph already.

### What Kit exposes today that the target refuses

| Today | Why it goes | Fate |
|---|---|---|
| `pub use cupertino_ui_core as core`, `cupertino_ui_controls as controls`, `cupertino_ui_render as render`, `cupertino_ui_text as text`, `cupertino_ui_core::{layout, platform}`, `cupertino_theme::ui as theme`, `cupertino_geometry as geometry` | a lower crate, or a module of one, as a Kit module | Kit modules of the same names with the lists above |
| `render::gpu`, `render::render_thread` (reached through `render`) | GPU objects | not named; no program uses them |
| `platform::{Lent, DrawInto, PresentOutcome, Configure}` (through `platform`) | raw surfaces and buffers | not named; no program uses them |
| `platform::WindowSystem` | the window system's own interface | leaves with its last caller |
| `platform::semantics` (through `platform`) | `cupertino-app-protocols`' module whole | not named |
| `platform::{CubicBezier, LayerCurve}`, `theme::{EASE_CURVE, LINEAR_CURVE, EASE_IN_CURVE, EASE_OUT_CURVE, EASE_IN_OUT_CURVE, DEFAULT_SPRING}` | built from numbers (D7) | `kit::motion` |
| `core::{Runtime, RendererFactory, EachTurn, MonotonicClock, AccessibilityAdapter, set_personal}`, `layout::{layout, layout_cached, LayoutCache, LayoutTree}` (through `core`, `layout`) | the runtime's wiring, which `app::run` does | `Runtime` in `kit::test`; the others are not named |
| `app::run_on`, `run_on_with`, `run_lock_screen_on_with`, `options`, `core::RuntimeOptions` | take a `WaylandWindowSystem`, or carry the runtime's wiring | leave with their last caller |
| `app::run_on_with_appearance`, `start_preferences`, `Timed`, `frame_timing_json`, `scroll_timing_json`, `SCROLL_TIMING_ENV`, `FRAME_TIMING_ENV` | no caller outside Kit | private to Kit; the two variable names stay in this README |
| `app::each_turn`, `preferences` | serve only a driver | `kit::test` |

### Step 2

**Owns** `kit`; the `animation` module of `cupertino-theme` and its tests; the `curve` fields of `PresentChange`
(`ui-core/src/platform.rs:81`) and `LayerChange` (`platform/surface_layers.rs:47`); `cupertino-layers`; the
`build-programs` alias; the build lines of both package recipes.

**Touches.** Each edit puts a Kit name where another name stands; no number and no behaviour changes.

| Crate | Edit |
|---|---|
| `ui-wayland`, `ui-test` | read `Timing::layer_curve()` where they read the field |
| `ui-abi` | `Application`, `Command`, `Runtime`, `Window` from `cupertino-ui-core` (`src/registry.rs:12`), with its dependency line |
| `controlcenter` | `lifecycle.rs:23-33` names the two catalogue entries; `app.rs:716-728` passes them |
| `launcher` | `view/layers.rs:653,736,740,849` and `motion.rs`: `Easing` and the catalogue spring where they name the curve constants and `LayerCurve` (6 uses) |
| `dock`, `login`, `screenshot` | `Easing::EASE_IN_OUT` where `dock.rs`, `switcher.rs`, `picker.rs` and `thumbnail.rs` name the curve constant |
| `settings` | `about_app.rs:76`: `app::environment` |
| `files`, `login`, `kit` | the capture examples and `login`'s benches name `kit::test`; their `Cargo.toml` takes `cupertino-kit` with `test` as a dev-dependency |
| every `shell` and `apps` crate | `use` paths of items the lists above place elsewhere; none for a path that stays |

**Leaves with its last caller.** These stay nameable after step 2, because replacing a caller is that caller's own
task, not a mechanical edit. The decision log records the list as a temporary exposure.

| Items | Callers | Each caller's task | After |
|---|---|---|---|
| `app::run_on`, `run_on_with`, `run_lock_screen_on_with`, `platform::WindowSystem`, `core::RuntimeOptions` | nine programs; `cupertino-kit-shell` from W5.10 | each program's W6 task; the desktop items (`files/src/kit/desktop/mod.rs:1196`) W6.10; `login` (`app.rs:443`, `greeter.rs:390`) W6.13 | a component starts through `cupertino-kit-shell` and gets a `core::Environment` |
| `app::options` | `cupertino-ui-abi`, `notifications` (`app.rs:653`, which shares `metrics` with its `run_on_with`), the drivers | `notifications` W6.3 | `app::environment` in a program; `kit::test::options` for a driver; the start-up below Kit for `cupertino-ui-abi` |
| renderers and glyph rasters (16 paths) | `controlcenter` (`view.rs:31-36,839`), `dock` (`view/paint.rs:14`), `login` (`view/widgets.rs:23`), `login`'s bench | W6.5, W6.8, W6.13 | `render::picture`; `kit::test::{CpuRenderer, CpuConfig}` for the bench |

One step added to W6.16, after the last of the tasks above (W6.10 and W6.13 among them), removes them: its Owns gains
`kit` (`app.rs`, the lists), `ui-wayland` and `kit-shell`, and it touches `ui-abi`. It puts Kit's start-up (the theme,
the fonts, the options, following the appearance, the timing reports) in `cupertino-ui-wayland`, where `cupertino-kit`,
`cupertino-kit-shell` and `cupertino-ui-abi` call it, and deletes the items above from Kit. W6.close checks that Kit
names none of them.

**Other tasks.**

- W4.8: Touches gains `kit`, for the file views' entries in `kit::controls`.
- W4.9: `cupertino-kit-shell`'s API also gives the menu bar's height, selection radius and fonts; the outputs (id,
  geometry, transform, description); starting an application on a connection with roles, from a `core::Environment`;
  the lock notice; the idle notice; the consent authority of `cupertino-prompts`; and the test clock
  (`motion_settled`, `display_unix_seconds`).
- W4.11: `capture::App::capture` returns `render::ImageResource`.
- `cupertino_assets::theme::load` joins the calls `cupertino-layers` writes to `clippy.toml` in step 2; no crate holds
  that file yet. Each caller drops the call in its own task: `notifications` W6.3, `controlcenter` W6.5, `menubar`
  W6.7, the desktop items W6.10, `polkit-agent` W6.14, `files`' tags W7.1.

### Open questions

Each has the default this proposal recommends; approving the proposal approves the defaults.

1. **The catalogue's types are outside W4.7's Owns.** `PresentChange` and `LayerChange` are `cupertino-ui-core` types,
   so a type they hold is defined at or below that crate. Default: as [Motion](#motion) and [Step 2](#step-2) say.
   Rejected: wrappers in Kit alone, which leave `LayerCurve` as the type of a public field Kit names.
2. **A duration with a named easing.** The dock's and the launcher's durations depend on distances, delays and
   settings, so they cannot all be constants. Default: `Easing::over(seconds)` is allowed, since the shape is named and
   the duration is not a curve parameter; a spring never takes a number.
3. **Where Kit's start-up is for `cupertino-kit-shell` and `cupertino-ui-abi`.** It is private to Kit's `app.rs` today
   and the `run_on` functions and `options` are the only ways in. Default: the step added to W6.16 above, with
   `cupertino-ui-wayland` as its place unless W4.9 names another `ui` crate.
4. **Renderers and glyph rasters.** The must-not-expose list does not name them. Three programs draw pictures of their
   own with them (the control panel's shadow bands, the dock's turned labels, the login screen's glass letters).
   Default: Kit stops naming them once those three use `render::picture(env, ops, size, scale, done) -> Command<M>`,
   which draws paint operations, text included, on the CPU pool and gives the `ImageResource` to `done` as a message
   (D4, D6). No pixel is drawn on a UI thread; a caller paints the picture from the turn it arrives.
5. **`kit::geometry`.** `cupertino-geometry` is a `foundation` crate outside the facade table, so a program may depend
   on it directly, and eight crates do (58 uses, against 197 through Kit). Default: `kit::geometry` stays, as the four
   types Kit's own signatures name.
6. **The shell's design values.** The notifications, the control panel and the desktop items read the menu bar's
   height, and the menu bar its fonts, from the theme data. Default: `cupertino-kit-shell` gives them (W4.9); Kit's
   `theme` stays what a control reads, with `system_color` for a colour the theme names.
7. **Pictures from files.** `tiny-skia` and `image` are not allowed in `shell` and `apps` (seven and two lines of
   `remaining.txt`), and W4.7 names only Kit-private crates. Default: `kit::image`, built in step 2, with the
   operations that have callers, all on `render::ImageResource` and all plain functions a caller runs as a
   `task::cpu`: `decode` (PNG and JPEG within limits the caller gives), `resize` and `encode_jpeg` for the login
   wallpaper; `crop`, `orient`, `resize` and `encode_png` for the screenshots. The dock and the menu bar only hold
   pixels in a `tiny_skia::Pixmap` and hold an `ImageResource` instead.
