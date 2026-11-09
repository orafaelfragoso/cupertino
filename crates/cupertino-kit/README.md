# cupertino-kit

CupertinoKit, the Cupertino UI toolkit: the one crate applications depend on. It re-exports the `cupertino-ui-*` crates
as modules and has a `prelude`. MPL-2.0.

A node is glass when its `paint` calls `PaintCx::glass` (`Glass` and `GlassStyle` are in the `prelude`): the compositor
draws the glass under it, and nodes in the same container merge into one shape (`cupertino-ui-core`, *Glass*):

```rust
use cupertino_kit::core::PaintCx;
use cupertino_kit::prelude::Glass;

fn paint(cx: &mut PaintCx<'_>) {
    cx.glass(Glass::regular().radius(12.0).container(1, 6.0));
}
```

`app::run` and `app::options` start on the Cupertino theme from the installed theme data, and on
`ThemeTokens::neutral` when there is none or it is unusable (one line on stderr says why), so an application runs on any
compositor. `app::run_on` and `app::run_on_with` run on a window system a desktop shell set up with its surface roles;
`app::run_lock_screen_on_with` does the same for the lock screen, whose AT-SPI adapter the session's lock never mutes
(`AtspiAdapter::lock_screen`), and `app::run_on_with_appearance` passes the session's appearance through a function first, for a
shell surface that keeps the dark appearance in a light session. Every one of them keeps the text caret from blinking
(`caret_phase_ms` 0) while the session asks for a steady caret, from the first frame. The `examples` test starts the Counter that way and compares it with `tests/goldens/counter-neutral.txt`.
Every one of them reports scroll timing when `CUPERTINO_SCROLL_TIMING` (`app::SCROLL_TIMING_ENV`) names an absolute path:
one JSON line a pointer scroll event, appended, with its stages in nanoseconds of `CLOCK_MONOTONIC` (`input`, the
window system's timestamp; `handled`; `updated`, the model update and view, or `null`; `submitted`, the frame that drew
it; `presented`, the compositor's time for that frame, or `null` when it was dropped). Without the variable no timing
is kept (`app::Timed`, on `Runtime::with_scroll_timing`).
`app::mono_family` names the asset packs' fixed-pitch family, which the text system loads with the interface face, for
text that lines up by character (a file's contents); `None` when no pack names one.

The widgets' icons are the asset packs' symbols. `Icons::application`, the icon an alert shows, is the icon named by
the `Icon=` of the desktop entry whose id is the running program's file name (`cupertino-files.desktop` for
`cupertino-files`), from the packs, else from the host icon themes or a file, else the packs' generic application icon.

`file_icons::FileIcons` gives apps that list files the icon of each item's type as image renditions (1× and 2×) for
`controls::Image`: the asset packs' and the host icon theme's icon of the type's kind or its nearest ancestor's (as
`cupertino_icons::document::document_icon_name` maps them), then the generic document icon, then the placeholder; a
special folder's own icon; and the alias badge of a symbolic link, drawn into the image. `discover` reads the installed
packs and the icon theme, `without_host` a given pack alone and `builtin` the built-in assets (placeholders), for tests
and captures. `pack_has(name)` says whether an asset pack ships a raster or SVG file of its own for an icon name, so an
app with its own art for a name can still let a pack's icon win. Files and the portal's file chooser draw their rows
with it.

Each example runs on Cupertino headless and on headless Mutter. One test per example and host asserts its app id,
title and buffer size, and that it presents a frame (on Cupertino it also exits when its window closes). Its behaviour
is checked by its golden: the trace of inputs, messages and models, then the Inspector dump.

| example | app id | size | Cupertino headless | Mutter headless | golden |
|---|---|---|---|---|---|
| Counter | `org.cupertino.examples.Counter` | 200×64 | `the_counter_example_maps_at_its_size_and_exits_on_close` | `the_counter_example_presents_frames_on_mutter` | `tests/goldens/counter.txt` |
| Forms | `org.cupertino.examples.Forms` | 280×180 | `the_forms_example_maps_at_its_size_and_exits_on_close` | `the_forms_example_presents_frames_on_mutter` | `tests/goldens/forms.txt` |
| Virtual List | `org.cupertino.examples.VirtualList` | 480×600 | `the_virtual_list_example_maps_at_its_size_and_exits_on_close` | `the_virtual_list_example_presents_frames_on_mutter` | `tests/goldens/virtual-list.txt` |
| Editor | `org.cupertino.examples.Editor` | 480×320 | `the_editor_example_maps_at_its_size_and_exits_on_close` | `the_editor_example_presents_frames_on_mutter` | `tests/goldens/editor.txt` |

The Cupertino tests are in `crates/cupertino-server/tests/standard-clients.rs`, the Mutter tests in
`crates/cupertino-wayland-client/tests/mutter.rs` (they skip when `mutter` is not installed), and the goldens in this
crate's `tests/examples.rs`.

Windows move by their toolbar. The toolbar row of a `NavigationStack` is a window drag region: a drag where no control
takes the press moves the window, and a double click there zooms it (maximizes it, or restores it). `WindowDragArea`
makes any other part of a window one; `EventCx::move_window` and `EventCx::zoom_window` are the requests underneath.
The keyboard has no move: the compositor's Zoom, Minimize and tile shortcuts and the menu bar's Window menu cover
it (see `cupertino-ui-controls`).

`ci/capture.sh kit/controls` and `kit/icons` draw the row 167a controls sheet and a grid of system symbols (`--direction rtl` mirrors them),
and `kit/scroll` a scroll view at rest, stretched and with its bar shown, fading and hidden (`--phase`), `kit/under-toolbar` a
page scrolling under a toolbar row drawn in a layer over it (`--phase rest|shown|hidden`), headless, in light and dark at any scale (`examples/kit_capture.rs`, `crates/cupertino-capture`). `kit/counter`, `kit/forms`,
`kit/virtual-list` and `kit/editor` draw the examples' windows as they open. `kit/gallery` draws every control family
side by side, `--state rest|held-button|held-switch|held-slider|held-segment|disabled` holding the pointer down on one
or disabling them all, at `--control-size mini|small|regular|large|extra-large` and `--compact on|off`, over
`--backdrop flat|busy`.

The `control-fixtures` test renders every control in each state it has (normal, hover, pressed, disabled, focused,
selected) in Light, Dark and both high-contrast appearances, left to right and right to left, through the CPU renderer,
and compares a sheet of the cells and each cell's accessibility tree with `tests/fixtures/controls`. A state a control
does not have is an empty cell and an `n/a` block. `CUPERTINO_UPDATE_GOLDENS=1` re-records and fails; a new control is
one more fixture.
