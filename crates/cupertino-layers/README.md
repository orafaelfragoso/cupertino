# cupertino-layers

Checks that dependencies between workspace crates follow the layer rules below, today's and the target's. A crate's
layer is declared in its manifest, `[package.metadata.cupertino] layer = "<layer>"`; in this tree crates live in
`crates/<crate>`. Layer: `tools`; the only dependency is `serde_json`.

## Usage

```sh
cargo run -p cupertino-layers                          # from anywhere inside the repository
cargo run -p cupertino-layers -- --graph current       # DOT graph of the declared layers
cargo run -p cupertino-layers -- --graph target        # DOT graph of the target layers
```

The binary runs `cargo metadata --no-deps`, collects every dependency between workspace crates (normal, build and
dev), and prints one line per problem to stderr; target problems start with `target:`. Without `--graph` the last line
is a summary:

```
50 crates, 155 internal dependencies, 0 pending, 6 target exceptions, 0 problems
```

It exits with failure on any problem, in both modes; lists it among the pre-finish checks.

`--graph` prints a DOT graph with one cluster per layer. Comment lines above it give the crate count per layer, every
forbidden edge with its exception reason or `violation`, stale exceptions and denied paths. Exceptions are drawn
orange and dashed, violations red. `graph/current.dot` and `graph/target.dot` are committed snapshots; regenerate them
after a dependency or classification change:

```sh
cargo run -q -p cupertino-layers -- --graph current > crates/cupertino-layers/graph/current.dot
cargo run -q -p cupertino-layers -- --graph target > crates/cupertino-layers/graph/target.dot
```

## Design

All rules are constants in `src/lib.rs`; nothing is read from a configuration file.

`RULES` is what the manifests follow today:

| Layer | May depend on |
|---|---|
| `foundation` | `foundation` |
| `models` | `foundation`, `models` |
| `platform` | `foundation`, `platform` |
| `render` | `foundation`, `platform`, `render` |
| `core`, `shell`, `services`, `apps`, `tools` | `foundation`, `models`, `platform`, `render` |

`TARGET_RULES` is the twelve-layer target. `TARGET` classifies every workspace crate into one of its layers (names below without
the `cupertino-` prefix):

| Layer | May depend on | Crates |
|---|---|---|
| `foundation` | `foundation` | animation, apps, assets, defaults, filepicker, geometry, theme, thumbnails, typeahead, xkbcommon |
| `models` | `foundation`, `models` | displays, effects, input, keyboard, login-ipc, overview, stage, text-field, wm |
| `platform` | `foundation`, `platform` | alsa, bluetooth, devices, evdev, netlink, seat |
| `protocol` | `foundation`, `protocol` | protocols |
| `transport` | `foundation`, `platform`, `protocol`, `transport` | client |
| `render` | `foundation`, `platform`, `render` | cursor, decorations, glass, icons |
| `ui` | `foundation`, `protocol`, `transport`, `ui` | none yet |
| `core` | `foundation`, `models`, `platform`, `protocol`, `render` | server |
| `shell` | `foundation`, `models`, `platform`, `protocol`, `transport`, `render`, `ui` | controlcenter, dock, launcher, login, menubar, notifications, osd, screenshot, windowmanager |
| `services` | `foundation`, `models`, `platform`, `protocol`, `services` | btd, netd, powerd, sessiond |
| `apps` | `foundation`, `models`, `protocol`, `transport`, `ui` | files, settings |
| `tools` | all but `core`, `shell`, `services`, `apps` | gtk-theme, layers, testclient |

`DENIED_PATHS` also forbids any chain of dependencies from `ui` to `core`, `shell` or `services`.

Two lists excuse known violations as `(from, to, reason)`: `PENDING` for `RULES` (empty) and `TARGET_EXCEPTIONS` for
`TARGET_RULES` (the six edges from the apps to the renderer and the mixer, and the server's dev-dependency on the
client). An entry that no longer matches an edge is an error, so both lists only shrink. A crate missing from `TARGET`,
or a `TARGET` entry that names no crate, is an error too.

Library: `Layer` (`ALL`, `name`, `from_name`), `allows(rules, from, to)`, `Edge`, `edges(layers, deps)`,
`classify(crates, target)`, `check(rules, edges, exceptions)`, `exception(exceptions, edge)`, `Problem`,
`denied_paths(edges, denied)`, `dot(…)`.

## Tests

```sh
cargo test -p cupertino-layers
```
