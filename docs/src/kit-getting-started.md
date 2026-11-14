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
  cargo test -p cupertino-kit --test suite examples::the_counter_starts_on_the_neutral_theme
  ```

- Fonts and icons come from an asset pack when one is installed (`CUPERTINO_ASSET_DIR`, else the user or system
  pack). The kit falls back to its built-in Inter font. Headless captures of the controls sheet need a pack
  (`ci/capture.sh` says so in its header).
- No GPU. The kit rasterizes each window on the CPU (`cupertino-ui-render`), repainting only what changed and keeping
  scrolled content as an image it moves, and hands it to the compositor in shared memory (`wl_shm`). Started with
  `CUPERTINO_RENDERER=gpu`, a process draws its first frames that way while its device warms up, then draws every
  window with the GPU renderer (`cupertino-ui-gpu`) on the device the compositor names and hands it over as a dmabuf;
  that is off by default until the GPU path is turned on, with the CPU kept as the fallback
  ([Roadmap](roadmap.md#planned-gpu-rendering-in-applications)).
  A long list or grid keeps scrolling cheap by
  building only the rows near its viewport, each as `Element::kept(key, mark, build)`: a row whose mark is the same
  as last time keeps its nodes and their layout and is not built again.
- A Rust toolchain. Nothing else is installed or enabled by these steps.

## Build and run the examples

The four examples live in `crates/cupertino-kit/examples`. Each opens a window on the session's display and runs until
you close it.

| Example | What it shows | Command |
|---|---|---|
| Counter | A number and two buttons that change it | `cargo run -p cupertino-kit --example counter` |
| Forms | A sign-in form: a name, a password and a choice | `cargo run -p cupertino-kit --example forms` |
| Virtual List | 50,000 two-line rows in a scrolling list with multiple selection, a status line, and a button that reverses the order and keeps the selection | `cargo run -p cupertino-kit --example virtual-list` |
| Editor | Text editing, selection and undo | `cargo run -p cupertino-kit --example editor` |

Build all four without running them:

```bash
cargo build -p cupertino-kit --examples
```

Run them in a nested Cupertino session, on another compositor, or on the host's Wayland session; see
[Building and running](building.md) for the nested server. On a Wayland session, Counter and Forms run until you close
them.

## Check the examples without a display

`cargo test -p cupertino-kit` drives the examples with input on the headless window system and compares their traces and
tree dumps with files in `crates/cupertino-kit/tests/goldens/`. `CUPERTINO_UPDATE_GOLDENS=1` rewrites them. The timing matrix is ignored in a normal run; run it with `--release --ignored --nocapture`. Test counts change as the toolkit grows; use the command output for the checked-out commit.

## From C

The C SDK is the `cupertino-kit` package: the library `libcupertinokit.so.1`, the header
`/usr/include/cupertinokit-1/cupertinokit.h` and `cupertinokit.pc`. A C program needs nothing else, and takes every
path from pkg-config. The ABI's rules (versions, handles, threads, errors) are at the top of the header and in
`crates/cupertino-ui-abi/README.md`. Today the library runs the headless backend only. With the package installed,
and `gcc`, `make` and `pkgconf`:

```bash
cupertino sdk info                 # what is installed, file by file
cupertino sdk check                # build and run a probe with /usr/bin/cc
cupertino sdk new --lang c demo    # main.c, a Makefile, demo.desktop and a README
make -C demo && ./demo/demo        # prints count: 1, count: 2, count: 3
```

Versions: the header defines `CUPERTINOKIT_ABI_MAJOR` and `CUPERTINOKIT_ABI_MINOR` for the compiler, and
`pkg-config --atleast-version=1.0.0 cupertinokit` checks the installed SDK in a build script. At run time
`ck_abi_version` reports the library's version; pass the minor version the program needs as `abi_minor` to
`ck_runtime_new`, which refuses an older library with `CK_ERROR_VERSION`, and ask `ck_has_capability` before calling a
function added after 1.0. A new major version gets a new soname and include directory. Threads: a runtime belongs to the
thread that created it; other threads only post to it through a waker (`ck_runtime_waker`, `ck_waker_post`), and the
runtime's thread reads the posts with `ck_poll_events`.

`cupertino sdk new --lang rust NAME` starts a Rust project on the `cupertino-kit` crate of the same release instead
(`--source CHECKOUT` takes it from a Cupertino checkout, offline or before the release is tagged). The samples in `crates/cupertino-ui-abi/examples/c` build with `make` the same way. From a checkout, `python3
ci/c-sdk.py` builds the SDK into a staging directory and runs all of the above against it.

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
- A window root without a name of its own is named by the window's title. A list item, tree item, cell or tab without
  a name is labelled by the text inside it. Menu items, tabs and page dots say their place and how many there are.
- Tests can check a whole window with `cupertino-ui-test`'s `accessibility_audit` (unnamed controls, missing values
  and states, focus that does not agree with the tree, masked text, items without place or count) and drive it as a
  screen reader would with its `AssistiveTechnology` stand-in, which also records what live regions announce.
- While the session is locked, and until an application learns it is not, each of its windows reads as an empty frame
  without a name and takes no action from the assistive technology; nothing is announced. The unlock publishes the
  windows whole again. The lock screen is what an assistive technology reads behind the lock.

Not done:

- Bounds are window-relative, because a Wayland client does not know its window position.
- A closed window's AT-SPI object stays until the application exits.
- An application on another toolkit keeps publishing its tree while the session is locked: only Kit applications
  hide theirs.

Commands behind these statements:

```bash
cargo test -p cupertino-ui-atspi
cargo test -p cupertino-kit --test suite assistive_technology::
cargo test -p cupertino-kit --test suite control_fixtures::
```

The first runs the mapping and filter tests, tests of the live-region rules and of the hidden windows while the session is locked, and, when
`dbus-daemon` is installed, tests on a private bus with a fake AT-SPI registry, one of them following a text field's
caret and selection (without `dbus-daemon` the bus tests print `SKIPPED`).
The second changes a form value and moves focus through assistive-technology requests. The third audits every
control in each state, appearance and direction, checks that the keyboard focus shows in each appearance, high contrast
included, and asks for every action each control offers, which must reach the application or change what a screen
reader reads. No screen reader was
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
- The controls gallery shows every control family together. Hold one down, disable them all, change their size or
  draw them over a busy backdrop:

  ```bash
  ci/capture.sh kit/gallery --state held-switch --control-size large --backdrop busy --scale 2
  ci/capture.sh kit/gallery --sheet
  ```

  The sheet puts the resting, held and disabled states side by side.

Limits:

- `app::run` always lays out left to right; it has no option to change that yet. The kit does not choose the direction
  from the locale or the session.

Commands behind these statements:

```bash
cargo test -p cupertino-ui-layout --test suite layout::right_to_left
cargo test -p cupertino-ui-text
cargo test -p cupertino-ui-layout --test suite properties::
```
