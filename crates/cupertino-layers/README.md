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
dev, one edge per kind), and prints one line per problem to stderr; target problems start with `target:`. Without `--graph` the last line
is a summary:

```
51 crates, 160 internal dependencies, 0 pending, 7 target exceptions, 0 problems
```

It exits with failure on any problem, in both modes; lists it among the pre-finish checks.

`--graph` prints a DOT graph with one cluster per layer. Comment lines above it give the crate count per layer, every
forbidden edge with its kind and its exception (tracking, area, reason) or `violation`, stale exceptions and denied
paths. Build and dev edges are labelled, exceptions drawn orange and dashed, violations red. `graph/current.dot` and
`graph/target.dot` are committed snapshots, and `tests/graph-snapshots.rs` fails when they differ from the workspace;
regenerate them after a dependency or classification change:

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

`TARGET_RULES` is the twelve-layer target. `TARGET` classifies every workspace crate into one of its layers (names
without the `cupertino-` prefix):

| Layer | May depend on | Crates |
|---|---|---|
| `foundation` | `foundation` | animation, apps, assets, defaults, filepicker, geometry, text-field, theme, thumbnails, typeahead, xkbcommon |
| `models` | `foundation`, `models` | displays, effects, input, keyboard, overview, stage, wm |
| `platform` | `foundation`, `platform` | alsa, bluetooth, devices, evdev, netlink, peer, seat |
| `protocol` | `foundation`, `protocol` | login-ipc, protocols |
| `transport` | `foundation`, `platform`, `protocol`, `transport` | client, wayland-client |
| `render` | `foundation`, `platform`, `render` | cursor, decorations, glass, icons |
| `ui` | `foundation`, `protocol`, `transport`, `ui` | none yet |
| `core` | `foundation`, `models`, `platform`, `protocol`, `render` | server |
| `shell` | `foundation`, `models`, `platform`, `protocol`, `transport`, `render`, `ui` | controlcenter, dock, launcher, login, menubar, notifications, osd, screenshot, windowmanager |
| `services` | `foundation`, `models`, `platform`, `protocol`, `services` | btd, netd, powerd, sessiond |
| `apps` | `foundation`, `models`, `protocol`, `transport`, `ui` | files, settings |
| `tools` | all but `core`, `shell`, `services`, `apps` | layers, testclient |

Pure toolkit-facing models (filepicker, text-field, typeahead) are `foundation`, so the toolkit can use them.
`keyboard` stays in `models`: it is compositor keyboard policy.

`DENIED_PATHS` forbids any chain of dependencies from `ui` to `core`, `shell` or `services`; a denied path fails the
check even when exceptions excuse its edges. `licence_problems` checks the licence split: a crate whose `license` starts with `MPL-2.0` may not have a normal dependency on a workspace crate under a GPL or AGPL licence (build and dev dependencies do not count), and needs a `LICENSE` file with the MPL-2.0 text next to its manifest. Both fail the check.

`DENIED_CRATES` forbids any chain of normal dependencies, external crates
included, from `cupertino-wayland-client` to `cupertino-protocols` or another compositor's protocol crates: the public
transport speaks standard protocols only.

`PENDING` (for `RULES`, empty) and `TARGET_EXCEPTIONS` (for `TARGET_RULES`) hold `Exception` values: `from`, `to`, the
dependency `kind` it excuses, an owner `area` (compositor, rendering, design, apps, services, packaging), a `reason`
and a `tracking`:

- `Permanent`: a reviewed edge that stays, such as a dev-only edge for tests or an approved render primitive. Today:
  server → client (dev).
- `Issue(n)`: migration debt the issue removes. Today: #46 files and settings → decorations and icons, #21 settings →
  alsa, #47 text-field → keyboard.

An entry that no longer matches an edge of its kind is an error, so both lists only shrink. A crate missing from
`TARGET`, or a `TARGET` entry that names no crate, is an error too.

Library: `Layer` (`ALL`, `name`, `from_name`), `Kind`, `allows(rules, from, to)`, `Edge`, `edges(layers, deps)`,
`classify(crates, target)`, `Exception`, `Area`, `Tracking`, `check(rules, edges, exceptions, denied)`,
`exception(exceptions, edge)`, `Problem`, `denied_paths(edges, denied)`, `denied_crate_paths(deps, denied)`, `licence_problems(licences, deps, has_text)`, `is_mpl`, `is_gpl`, `dot(…)`.

## Tests

```sh
cargo test -p cupertino-layers
```
