# cupertino-kit

CupertinoKit, the Cupertino UI toolkit: the one crate applications depend on. It re-exports the `cupertino-ui-*` crates
as modules and has a `prelude`. MPL-2.0.

`app::run` and `app::options` start on the Cupertino theme from the installed theme data, and on
`ThemeTokens::neutral` when there is none or it is unusable (one line on stderr says why), so an application runs on any
compositor. `app::run_on` and `app::run_on_with` run on a window system a desktop shell set up with its surface roles;
`app::run_lock_screen_on_with` does the same for the lock screen, whose AT-SPI adapter the session's lock never mutes
(`AtspiAdapter::lock_screen`). The `examples` test starts the Counter that way and compares it with `tests/goldens/counter-neutral.txt`.

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

`ci/capture.sh kit/controls` and `kit/icons` draw the row 167a controls sheet and a grid of system symbols (`--direction rtl` mirrors them)
headless, in light and dark at any scale (`examples/kit_capture.rs`, `crates/cupertino-capture`). `kit/counter`, `kit/forms`,
`kit/virtual-list` and `kit/editor` draw the examples' windows as they open.

The `control-fixtures` test renders every control in each state it has (normal, hover, pressed, disabled, focused,
selected) in Light, Dark and both high-contrast appearances, left to right and right to left, through the CPU renderer,
and compares a sheet of the cells and each cell's accessibility tree with `tests/fixtures/controls`. A state a control
does not have is an empty cell and an `n/a` block. `CUPERTINO_UPDATE_GOLDENS=1` re-records and fails; a new control is
one more fixture.
