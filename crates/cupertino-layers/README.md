# cupertino-layers

Checks that dependencies between workspace crates follow the layer rules below. A crate's
layer is declared in its manifest, `[package.metadata.cupertino] layer = "<layer>"`; in this tree crates live in
`crates/<crate>`. Layer: `tools`.

## Usage

```sh
cargo run -p cupertino-layers               # from anywhere inside the repository
cargo run -p cupertino-layers -- --graph    # DOT graph of the declared layers
```

The binary runs `cargo metadata`, collects every dependency between workspace crates (normal, build and
dev, one edge per kind), and prints one line per problem to stderr. Without `--graph` the last line is a summary:

```
71 crates, 291 internal dependencies, 3 exceptions, 0 problems
```

It exits with failure on any problem, in both modes; lists it among the pre-finish checks.

`--graph` prints a DOT graph with one cluster per layer. Comment lines above it give the crate count per layer, every
forbidden edge with its kind and its exception (tracking, area, reason) or `violation`, stale exceptions and denied
paths. Build and dev edges are labelled, exceptions drawn orange and dashed, violations red. `graph/layers.dot` is a committed
snapshot, and `tests/graph-snapshots.rs` fails when it differs from the workspace; regenerate it after a dependency or
layer change:

```sh
cargo run -q -p cupertino-layers -- --graph > crates/cupertino-layers/graph/layers.dot
```

The test leaves out the two count lines (`// N crates, M internal dependencies` and `// per layer: ...`). A branch
keeps main's count lines as they are, and the merge queue regenerates the snapshot after each merge, so two branches
that each add a crate do not conflict on them.

## Design

All rules are constants in `src/lib.rs`; nothing is read from a configuration file.

`RULES` has twelve layers. The crates that declare each layer (names without the `cupertino-` prefix):

| Layer | May depend on | Crates |
|---|---|---|
| `foundation` | `foundation` | animation, apps, assets, build-info, defaults, filepicker, geometry, glyph, icons, key-bindings, keyboard, menu-model, modifiers, test-bus, theme, thumbnails, typeahead, xkbcommon |
| `models` | `foundation`, `models` | displays, effects, input, overview, render-api, scene, stage, wm |
| `platform` | `foundation`, `platform` | alsa, bluetooth, devices, evdev, netlink, peer, seat |
| `protocol` | `foundation`, `protocol` | app-protocols, login-ipc, protocols |
| `transport` | `foundation`, `platform`, `protocol`, `transport` | protocol-app, shell-client, wayland-client |
| `render` | `foundation`, `platform`, `render` | cursor, decorations, glass |
| `ui` | `foundation`, `protocol`, `transport`, `ui` | kit, ui-controls, ui-core, ui-layout, ui-menus, ui-platform, ui-render, ui-test, ui-text, ui-theme, ui-wayland |
| `core` | `foundation`, `models`, `platform`, `protocol`, `render` | server |
| `shell` | `foundation`, `models`, `platform`, `protocol`, `transport`, `render`, `ui` | controlcenter, dock, launcher, login, menubar, notifications, osd, screenshot, windowmanager |
| `services` | `foundation`, `models`, `platform`, `protocol`, `services` | btd, netd, powerd, sessiond |
| `apps` | `foundation`, `models`, `protocol`, `transport`, `ui` | files, settings |
| `tools` | all but `core`, `shell`, `services`, `apps` | cli, diagnostics, layers, testclient |

Pure toolkit-facing models (filepicker, typeahead, keyboard bindings) and the icon loader are `foundation`, so the toolkit can use them.

`DENIED_PATHS` forbids any chain of dependencies from `ui` to `core`, `shell` or `services`; a denied path fails the
check even when exceptions excuse its edges. The licence check requires a crate whose `license` starts with `MPL-2.0`
to have a `LICENSE` file with the MPL-2.0 text, and rejects normal dependencies on workspace crates under GPL or AGPL.
It also follows resolved normal and build dependencies through external crates and requires an accepted SPDX licence expression.
An `OR` expression passes if at least one option is accepted; every term of an `AND` expression must be accepted.
Missing or unrecognised external licences fail, except for the vendored xkbcommon licence text identified by its hash.
Dev dependencies do not count in these checks. Required toolkit and transport roots must be present and licensed under MPL-2.0.

`DENIED_CRATES` forbids any chain of normal dependencies, external crates
included, from `cupertino-wayland-client` to `cupertino-protocols` or another compositor's protocol crates: the public
transport speaks standard protocols only. Every toolkit crate is also denied `cupertino-protocols` and
`cupertino-shell-client`.

`EXCEPTIONS` holds `Exception` values: `from`, `to`, the
dependency `kind` it excuses, an owner `area` (compositor, rendering, design, apps, services, packaging), a `reason`
and a `tracking`:

- `Permanent`: a reviewed edge that stays, such as a dev-only edge for tests or an approved render primitive. Today:
  none.
- `Issue(n)`: migration debt the issue removes. Today: #21 settings → alsa.

An entry that no longer matches an edge of its kind is an error, so the list only shrinks. A crate without a declared
layer, or with a layer that has no row in `RULES`, stops the check.

Library: `Layer` (`ALL`, `name`, `from_name`), `Kind`, `allows(rules, from, to)`, `Edge`, `edges(layers, deps)`,
`Exception`, `Area`, `Tracking`, `check(rules, edges, exceptions, denied)`,
`exception(exceptions, edge)`, `Problem`, `denied_paths(edges, denied)`, `denied_crate_paths(deps, denied)`, `licence_problems(licences, deps, has_text)`, `is_mpl`, `is_gpl`, `dot(…)`.

## Tests

```sh
cargo test -p cupertino-layers
```
