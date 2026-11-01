# CupertinoKit: getting started

CupertinoKit is the UI toolkit for Cupertino applications. An application depends on one crate, `cupertino-kit`
(MPL-2.0), which re-exports the `cupertino-ui-*` crates as modules and has a `prelude`. Every command below runs from a
checkout and was run on the commit this page was written against.

## What an application needs

- A Wayland display. `cupertino_kit::app::run::<A>()` connects to the session's display. Without one it returns
  `cannot connect to the Wayland display` and the example exits with a failure:

  ```bash
  env -u WAYLAND_DISPLAY cargo run -p cupertino-kit --example counter
  ```

- No Cupertino desktop. The kit reads theme data from `$CUPERTINO_SPEC_DIR` (a default directory otherwise). When the
  data is missing or unusable, the application starts on the built-in neutral theme and prints one line on stderr
  saying why. Any Wayland compositor works. To see it, point the variable at an empty directory:

  ```bash
  CUPERTINO_SPEC_DIR="$(mktemp -d)" cargo run -p cupertino-kit --example counter
  ```

  The window opens and stderr shows `cupertino-kit: no Cupertino theme (…); using the neutral theme`. The `examples`
  test starts the Counter this way and compares it with `tests/goldens/counter-neutral.txt`:

  ```bash
  cargo test -p cupertino-kit --test examples the_counter_starts_on_the_neutral_theme
  ```

- Fonts and icons come from an asset pack when one is installed (`CUPERTINO_ASSET_DIR`, else the user or system
  pack). The kit falls back to its built-in Inter font. Headless captures of the controls sheet need a pack
  (`ci/capture.sh` says so in its header).
- A Rust toolchain. Nothing else is installed or enabled by these steps.

## Build and run the examples

The three examples live in `crates/cupertino-kit/examples`. Each opens a window on the session's display and runs until
you close it.

| Example | What it shows | Command |
|---|---|---|
| Counter | A number and two buttons that change it | `cargo run -p cupertino-kit --example counter` |
| Forms | A sign-in form: a name, a password and a choice | `cargo run -p cupertino-kit --example forms` |
| Virtual List | 50,000 two-line rows in a scrolling list with multiple selection, a status line, and a button that reverses the order and keeps the selection | `cargo run -p cupertino-kit --example virtual-list` |

Build all three without running them:

```bash
cargo build -p cupertino-kit --examples
```

Run them in a nested Cupertino session, on another compositor, or on the host's Wayland session; see
[Building and running](building.md) for the nested server. On a Wayland session, Counter and Forms run until you close
them.

## Check the examples without a display

`cargo test -p cupertino-kit` drives the examples with input on the headless window system and compares their traces and
tree dumps with files in `crates/cupertino-kit/tests/goldens/`. `CUPERTINO_UPDATE_GOLDENS=1` rewrites them. At this
commit it passes: 8 unit tests, 2 in `assistive-technology`, 6 in `examples` (1 timing matrix is ignored; run it with
`--release --ignored --nocapture`), and 1 in `virtual-list-memory`.

## Accessibility

`app::run` publishes every window's accessibility tree over AT-SPI (`cupertino-ui-atspi`, on AccessKit's Unix
adapter). Nothing is exported until an assistive technology turns accessibility on (`org.a11y.Status` `IsEnabled`).

Works:

- Roles, names, descriptions, states, bounds, relations, children and values (text, number with range and step, toggle).
- Actions from the assistive technology: click, focus, increment, decrement, expand, collapse, show menu, scroll into
  view, set value. An action reaches the application only if its target advertised it.
- Password and protected values carry no value and cannot be set from outside.
- A text field's caret and selection, in characters, and a selection set from the assistive technology. A protected
  field publishes neither.

Not done:

- Bounds are window-relative, because a Wayland client does not know its window position.
- A closed window's AT-SPI object stays until the application exits.

Commands behind these statements:

```bash
cargo test -p cupertino-ui-atspi
cargo test -p cupertino-kit --test assistive-technology
```

The first runs the mapping and filter tests, a test of the live-region rules while the session is locked, and, when
`dbus-daemon` is installed, tests on a private bus with a fake AT-SPI registry, one of them following a text field's
caret and selection (without `dbus-daemon` the bus tests print `SKIPPED`).
The second changes a form value and moves focus through assistive-technology requests. No screen reader was
run against the examples for this page, so how a screen reader reads them is not documented here.

## Right to left

What exists:

- Layout mirrors children inside their parent in right-to-left mode. A horizontal scroll view starts at its right edge.
  Insets are physical and do not mirror.
- Text follows UAX 9: the base direction comes from the paragraph style or the first strong character, runs are shaped in
  their direction, lines are reordered for display, and carets, hit testing and selection rectangles work across
  direction changes. Tests cover Latin, Hebrew, Arabic, Devanagari, emoji and digits inside right-to-left text.
- In the toolkit's text editing (`cupertino-ui-text`), the arrow keys move on screen through right-to-left text.
- The toolkit's controls sheet draws mirrored:

  ```bash
  ci/capture.sh kit/controls --direction rtl
  ```

  This writes light and dark PNGs under the build directory's `captures/`. `kit/icons` takes the same flag; chevrons
  follow the reading direction.

Limits:

- `app::run` always lays out left to right; it has no option to change that yet. The kit does not choose the direction
  from the locale or the session.

Commands behind these statements:

```bash
cargo test -p cupertino-ui-layout --test layout right_to_left
cargo test -p cupertino-ui-text
cargo test -p cupertino-ui-layout --test properties
```
